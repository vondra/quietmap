//! Full per-pair energy: the five-bucket fan quadrature of one line source at one
//! receiver, each bucket on its own ray, wide buckets arc-clipped (CPU
//! seg_sampling::sampled_gob_bands_with_ground under scatter_band's evaluate_exact_pair).

#pragma once

#include "relevant_source_arc.cuh"

__device__ __forceinline__ float ground_or_barrier_attenuation_db(
    float ground_db,
    float terrain_db,
    float screening_db
) {
    const float barrier = terrain_db + screening_db;
    return barrier > 0.0f ? fmaxf(ground_db, barrier) : ground_db;
}

struct FanPoint {
    bool valid;
    float x_m;
    float y_m;
    float distance_m;
};

__device__ __forceinline__ FanPoint fan_point(
    const DeviceLineSource& source,
    float receiver_x_m,
    float receiver_y_m,
    float azimuth
) {
    FanPoint point;
    point.valid = segment_point_at_azimuth(source, receiver_x_m, receiver_y_m, azimuth,
                                           point.x_m, point.y_m, point.distance_m);
    return point;
}

/// Energy-mean of max(A_ground, A_terrain + A_screen) over the fan's buckets;
/// returns the number of buckets used (0 when the receiver sits on the segment).
__device__ __forceinline__ int fan_ground_or_barrier_energy(
    const DeviceScenePointers& scene,
    const DeviceLineSource& source,
    float receiver_x_m,
    float receiver_y_m,
    float receiver_altitude_m,
    const float ground_db[QUIETMAP_BAND_COUNT],
    PathProfile& profile,
    float energy[QUIETMAP_BAND_COUNT]
) {
    const float start_x = source.start_x_m - receiver_x_m;
    const float start_y = source.start_y_m - receiver_y_m;
    const float end_x = source.end_x_m - receiver_x_m;
    const float end_y = source.end_y_m - receiver_y_m;
    const float base_azimuth = atan2f(start_y, start_x);
    const float span = wrap_to_pi(atan2f(end_y, end_x) - base_azimuth);
    const float inverse_count = 1.0f / static_cast<float>(QUIETMAP_LINE_DIRECTION_COUNT);
    const float cosine_bucket_width = cosf(fabsf(span) * inverse_count);
    int used = 0;
    FanPoint low = fan_point(source, receiver_x_m, receiver_y_m, base_azimuth);
    for (int bucket = 0; bucket < QUIETMAP_LINE_DIRECTION_COUNT; ++bucket) {
        const FanPoint high = fan_point(
            source, receiver_x_m, receiver_y_m,
            fmaf(static_cast<float>(bucket + 1) * inverse_count, span, base_azimuth));
        const float centre_azimuth = fmaf(
            (static_cast<float>(bucket) + 0.5f) * inverse_count, span, base_azimuth);
        const FanPoint centre = fan_point(source, receiver_x_m, receiver_y_m, centre_azimuth);
        if (!centre.valid) {
            low = high;
            continue;
        }
        build_path_profile(scene, centre.x_m, centre.y_m, receiver_x_m, receiver_y_m,
                           centre.distance_m, source_is_bridge(source), profile);
        float terrain_db[QUIETMAP_BAND_COUNT];
        float screening_db[QUIETMAP_BAND_COUNT];
        ray_terrain_and_screening_bands(
            scene, centre.x_m, centre.y_m, receiver_x_m, receiver_y_m,
            profile.elevation_m[0] + source.source_height_m, receiver_altitude_m, true, 0.0f,
            profile, terrain_db, screening_db);
        if (low.valid && high.valid) {
            const float chord_m = sqrtf(fmaxf(
                fmaf(low.distance_m, low.distance_m, high.distance_m * high.distance_m)
                    - 2.0f * low.distance_m * high.distance_m * cosine_bucket_width,
                0.0f));
            const float sub_distance_m = fminf(fminf(low.distance_m, high.distance_m),
                                               centre.distance_m);
            if (chord_m > QUIETMAP_SEG_ARC_MIN_SPAN_RAD * sub_distance_m) {
                arc_screened_bucket_increment(
                    scene, source, receiver_x_m, receiver_y_m, receiver_altitude_m,
                    low.x_m, low.y_m, high.x_m, high.y_m,
                    atan2f(centre.y_m - receiver_y_m, centre.x_m - receiver_x_m),
                    sub_distance_m + chord_m, ground_db, terrain_db, profile, screening_db);
            }
        }
        for (int band = 0; band < QUIETMAP_BAND_COUNT; ++band) {
            energy[band] += quietmap_energy_from_db(-ground_or_barrier_attenuation_db(
                ground_db[band], terrain_db[band], screening_db[band]));
        }
        ++used;
        low = high;
    }
    return used;
}

__device__ __forceinline__ bool evaluate_source_receiver_energy(
    const DeviceScenePointers& scene,
    uint32_t source_index,
    float receiver_x_m,
    float receiver_y_m,
    float receiver_altitude_m,
    float receiver_reflection_db,
    float output_energy[QUIETMAP_PERIOD_COUNT]
) {
    const DeviceLineSource source = scene.sources[source_index];
    LineReceiverGeometry geometry;
    if (!line_receiver_geometry(scene, source, receiver_x_m, receiver_y_m,
                                receiver_altitude_m, receiver_reflection_db, geometry)) {
        return false;
    }
    PathProfile profile;
    build_path_profile(scene, geometry.closest_x_m, geometry.closest_y_m,
                       receiver_x_m, receiver_y_m, geometry.endpoint_distance_m,
                       source_is_bridge(source), profile);
    float ground_db[QUIETMAP_BAND_COUNT];
    ground_attenuation_bands(profile, geometry.source_altitude_m, receiver_altitude_m, ground_db);
    const float characteristic_forest_depth_m = profile.forest_depth_m;
    float ground_or_barrier_energy[QUIETMAP_BAND_COUNT] = {};
    int used_directions = 0;
    const float segment_x = source.end_x_m - source.start_x_m;
    const float segment_y = source.end_y_m - source.start_y_m;
    const bool fan_exists = fmaf(segment_x, segment_x, segment_y * segment_y) >= 1.0e-6f;
    if (scene.obstacle_grid_count > 0 && fan_exists) {
        used_directions = fan_ground_or_barrier_energy(
            scene, source, receiver_x_m, receiver_y_m, receiver_altitude_m, ground_db,
            profile, ground_or_barrier_energy);
        if (used_directions == 0) {
            // Every bucket degenerate: the receiver sits on the segment, where the
            // characteristic ray resolves to no screening — ground alone over its terrain.
            build_path_profile(scene, geometry.closest_x_m, geometry.closest_y_m,
                               receiver_x_m, receiver_y_m, geometry.endpoint_distance_m,
                               source_is_bridge(source), profile);
            float terrain_db[QUIETMAP_BAND_COUNT];
            float screening_db[QUIETMAP_BAND_COUNT];
            ray_terrain_and_screening_bands(
                scene, geometry.closest_x_m, geometry.closest_y_m, receiver_x_m, receiver_y_m,
                geometry.source_altitude_m, receiver_altitude_m, false, 0.0f, profile,
                terrain_db, screening_db);
            for (int band = 0; band < QUIETMAP_BAND_COUNT; ++band) {
                ground_or_barrier_energy[band] = quietmap_energy_from_db(
                    -ground_or_barrier_attenuation_db(ground_db[band], terrain_db[band], 0.0f));
            }
            used_directions = 1;
        }
    } else {
        float terrain_db[QUIETMAP_BAND_COUNT];
        float screening_db[QUIETMAP_BAND_COUNT];
        ray_terrain_and_screening_bands(
            scene, geometry.closest_x_m, geometry.closest_y_m, receiver_x_m, receiver_y_m,
            geometry.source_altitude_m, receiver_altitude_m, true,
            source_exclusion_radius_m(source), profile, terrain_db, screening_db);
        for (int band = 0; band < QUIETMAP_BAND_COUNT; ++band) {
            ground_or_barrier_energy[band] = quietmap_energy_from_db(
                -ground_or_barrier_attenuation_db(
                    ground_db[band], terrain_db[band], screening_db[band]));
        }
        used_directions = 1;
    }
    for (int period = 0; period < QUIETMAP_PERIOD_COUNT; ++period) {
        float period_energy = 0.0f;
        for (int band = 0; band < QUIETMAP_BAND_COUNT; ++band) {
            const float path_level = geometry.base_level_db
                - QUIETMAP_ATMOSPHERIC_DB_PER_KM[band] * geometry.slant_distance_m * 0.001f
                + QUIETMAP_A_WEIGHTING[band]
                - fminf(QUIETMAP_VEGETATION_DB_PER_M[band] * characteristic_forest_depth_m,
                        QUIETMAP_VEGETATION_CAP_DB[band]);
            const float transfer = quietmap_energy_from_db(path_level)
                * ground_or_barrier_energy[band] / static_cast<float>(used_directions);
            period_energy = fmaf(
                source.emission_linear[period * QUIETMAP_BAND_COUNT + band],
                transfer, period_energy);
        }
        output_energy[period] = isfinite(period_energy) && period_energy > 0.0f
            ? period_energy : 0.0f;
    }
    return true;
}

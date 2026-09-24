//! Full per-pair energy: a line piece's CNOSSOS point sum as quadrature nodes uniform in the
//! in-plane angle, each node on its own ray, wide buckets on geometry-placed nodes (CPU
//! noise-compute line_quadrature + compute/line_piece); a point source on one ray.

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

/// Linear transfer `10^(-A/10)` of the ray from the source point `(x, y)` (its ground plus the
/// source height) to the receiver: everything but divergence and reflection (CPU
/// ray_transfer::evaluate_ray_transfer, full variant).
__device__ void ray_transfer(
    const DeviceScenePointers& scene,
    const DeviceLineSource& source,
    float source_x_m,
    float source_y_m,
    float receiver_x_m,
    float receiver_y_m,
    float receiver_altitude_m,
    bool obstacles_on_ray,
    PathProfile& profile,
    float transfer[QUIETMAP_BAND_COUNT]
) {
    const float horizontal_m = fmaxf(
        hypotf(receiver_x_m - source_x_m, receiver_y_m - source_y_m), 1.0f);
    build_path_profile(scene, source_x_m, source_y_m, receiver_x_m, receiver_y_m, horizontal_m,
                       source_is_bridge(source), profile);
    const float source_altitude_m = profile.elevation_m[0] + source.source_height_m;
    const float slant_m = fmaxf(hypotf(horizontal_m, receiver_altitude_m - source_altitude_m), 1.0f);
    float ground_db[QUIETMAP_BAND_COUNT];
    ground_attenuation_bands(profile, source_altitude_m, receiver_altitude_m, ground_db);
    const float forest_depth_m = profile.forest_depth_m;
    float terrain_db[QUIETMAP_BAND_COUNT];
    float screening_db[QUIETMAP_BAND_COUNT];
    ray_terrain_and_screening_bands(
        scene, source_x_m, source_y_m, receiver_x_m, receiver_y_m, source_altitude_m,
        receiver_altitude_m, obstacles_on_ray, source_exclusion_radius_m(source), profile,
        terrain_db, screening_db);
    for (int band = 0; band < QUIETMAP_BAND_COUNT; ++band) {
        const float attenuation =
            ground_or_barrier_attenuation_db(ground_db[band], terrain_db[band], screening_db[band])
            + fminf(QUIETMAP_VEGETATION_DB_PER_M[band] * forest_depth_m,
                    QUIETMAP_VEGETATION_CAP_DB[band])
            + QUIETMAP_ATMOSPHERIC_DB_PER_KM[band] * slant_m * 0.001f;
        transfer[band] = quietmap_energy_from_db(-attenuation);
    }
}

/// The source point of the line at `along_m` in the scene frame.
__device__ __forceinline__ void line_source_point(
    const LinePieceGeometry& geometry,
    float receiver_x_m,
    float receiver_y_m,
    float along_m,
    float& x_m,
    float& y_m
) {
    float point[3];
    line_point_at(geometry, along_m, point);
    x_m = receiver_x_m + point[0];
    y_m = receiver_y_m + point[1];
}

/// Σ Δφ·T over the piece's quadrature nodes (CPU line_quadrature_nodes).
__device__ void line_quadrature_transfer(
    const DeviceScenePointers& scene,
    const DeviceLineSource& source,
    const LinePieceGeometry& geometry,
    float receiver_x_m,
    float receiver_y_m,
    float receiver_altitude_m,
    PathProfile& profile,
    float sum[QUIETMAP_BAND_COUNT]
) {
    const float bucket_angle = line_subtended_angle(geometry) / QUIETMAP_LINE_BUCKET_COUNT;
    float transfer[QUIETMAP_BAND_COUNT];
    for (int bucket = 0; bucket < QUIETMAP_LINE_BUCKET_COUNT; ++bucket) {
        const float angle_lo = fmaf(static_cast<float>(bucket), bucket_angle, geometry.start_angle_rad);
        const float angle_hi = angle_lo + bucket_angle;
        WideBucketNodes wide;
        if (wide_bucket_nodes(scene, source, geometry, receiver_x_m, receiver_y_m, angle_lo, angle_hi,
                              wide)) {
            int bin = 0;
            while (bin < QUIETMAP_WIDE_BUCKET_MASK_BINS) {
                bool run_blocked;
                const int run_end = wide_bucket_run_end(wide, bin, run_blocked);
                const float run_lo = fmaf(static_cast<float>(bin), wide.bin_width, wide.span_lo);
                const float run_hi = run_end == QUIETMAP_WIDE_BUCKET_MASK_BINS
                    ? wide.span_hi : fmaf(static_cast<float>(run_end), wide.bin_width, wide.span_lo);
                const int parts = min(max(static_cast<int>(ceilf(
                    (run_hi - run_lo) / QUIETMAP_WIDE_BUCKET_PART_MAX_SPAN_RAD)), 1),
                    QUIETMAP_WIDE_BUCKET_MAX_PARTS_PER_RUN);
                const float step = (run_hi - run_lo) / static_cast<float>(parts);
                for (int part = 0; part < parts; ++part) {
                    const bool last_part = run_end == QUIETMAP_WIDE_BUCKET_MASK_BINS && part + 1 == parts;
                    const float part_lo = fmaf(static_cast<float>(part), step, run_lo);
                    const float part_hi = last_part ? wide.span_hi : part_lo + step;
                    const float edge_lo = bin == 0 && part == 0
                        ? wide.angle_at_span_lo
                        : wide_bucket_angle_at_azimuth(geometry, wide, part_lo, wide.angle_at_span_lo);
                    const float edge_hi = last_part
                        ? wide.angle_at_span_hi
                        : wide_bucket_angle_at_azimuth(geometry, wide, part_hi, wide.angle_at_span_hi);
                    float along_m;
                    if (!line_along_at_azimuth(geometry, 0.5f * (part_lo + part_hi), along_m)) {
                        along_m = line_along_at_angle(geometry, 0.5f * (edge_lo + edge_hi));
                    }
                    float x_m;
                    float y_m;
                    line_source_point(geometry, receiver_x_m, receiver_y_m, along_m, x_m, y_m);
                    ray_transfer(scene, source, x_m, y_m, receiver_x_m, receiver_y_m,
                                 receiver_altitude_m, run_blocked, profile, transfer);
                    const float weight = fabsf(edge_hi - edge_lo);
                    for (int band = 0; band < QUIETMAP_BAND_COUNT; ++band) {
                        sum[band] = fmaf(weight, transfer[band], sum[band]);
                    }
                }
                bin = run_end;
            }
            continue;
        }
        float x_m;
        float y_m;
        line_source_point(geometry, receiver_x_m, receiver_y_m,
                          line_along_at_angle(geometry, angle_lo + 0.5f * bucket_angle), x_m, y_m);
        ray_transfer(scene, source, x_m, y_m, receiver_x_m, receiver_y_m, receiver_altitude_m, true,
                     profile, transfer);
        for (int band = 0; band < QUIETMAP_BAND_COUNT; ++band) {
            sum[band] = fmaf(bucket_angle, transfer[band], sum[band]);
        }
    }
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
    float transfer[QUIETMAP_BAND_COUNT];
    float divergence_linear;
    PathProfile profile;
    if (source_is_ground_ops(source)) {
        // Ground ops keep their closest-point ray and band-mean ground (W5 carve-out).
        LineReceiverGeometry geometry;
        if (!ground_ops_receiver_geometry(scene, source, receiver_x_m, receiver_y_m,
                                          receiver_reflection_db, geometry)) {
            return false;
        }
        build_path_profile(scene, geometry.closest_x_m, geometry.closest_y_m,
                           receiver_x_m, receiver_y_m, geometry.endpoint_distance_m,
                           source_is_bridge(source), profile);
        float ground_db[QUIETMAP_BAND_COUNT];
        ground_ops_ground_bands(profile.ground_path_g, ground_db);
        const float forest_depth_m = profile.forest_depth_m;
        float terrain_db[QUIETMAP_BAND_COUNT];
        float screening_db[QUIETMAP_BAND_COUNT];
        ray_terrain_and_screening_bands(
            scene, geometry.closest_x_m, geometry.closest_y_m, receiver_x_m, receiver_y_m,
            geometry.source_altitude_m, receiver_altitude_m, true, 0.0f, profile,
            terrain_db, screening_db);
        for (int band = 0; band < QUIETMAP_BAND_COUNT; ++band) {
            transfer[band] = quietmap_energy_from_db(
                geometry.base_level_db
                - ground_or_barrier_attenuation_db(ground_db[band], terrain_db[band], screening_db[band])
                - fminf(QUIETMAP_VEGETATION_DB_PER_M[band] * forest_depth_m,
                        QUIETMAP_VEGETATION_CAP_DB[band])
                - QUIETMAP_ATMOSPHERIC_DB_PER_KM[band] * geometry.slant_distance_m * 0.001f);
        }
        divergence_linear = 1.0f;
    } else if (source_is_point(source)) {
        const float distance_m = hypotf(receiver_x_m - source.start_x_m,
                                        receiver_y_m - source.start_y_m);
        if (distance_m > source.max_distance_m
            || pair_is_inaudible(source, false, distance_m)) {
            return false;
        }
        ray_transfer(scene, source, source.start_x_m, source.start_y_m, receiver_x_m, receiver_y_m,
                     receiver_altitude_m, true, profile, transfer);
        // CPU scatter_point: the divergence distance never falls inside the footprint.
        const float source_altitude_m =
            sample_scene_raster(scene, source.start_x_m, source.start_y_m).elevation_m
            + source.source_height_m;
        const float slant_m = fmaxf(
            hypotf(fmaxf(distance_m, source.extent_m), source_altitude_m - receiver_altitude_m), 1.0f);
        divergence_linear = quietmap_energy_from_db(
            receiver_reflection_db - (8.685889638065036f * __logf(slant_m) + 11.0f));
    } else {
        float closest_distance_m;
        if (!line_closest_horizontal_distance(source, receiver_x_m, receiver_y_m, closest_distance_m)
            || closest_distance_m > source.max_distance_m
            || pair_is_inaudible(source, true, closest_distance_m)) {
            return false;
        }
        LinePieceGeometry geometry;
        if (!line_piece_geometry(scene, source, receiver_x_m, receiver_y_m, receiver_altitude_m,
                                 geometry)) {
            return false;
        }
        for (int band = 0; band < QUIETMAP_BAND_COUNT; ++band) {
            transfer[band] = 0.0f;
        }
        line_quadrature_transfer(scene, source, geometry, receiver_x_m, receiver_y_m,
                                 receiver_altitude_m, profile, transfer);
        divergence_linear = quietmap_energy_from_db(receiver_reflection_db)
            / (QUIETMAP_POINT_DIVERGENCE_LINEAR * geometry.perpendicular_m);
    }
    for (int period = 0; period < QUIETMAP_PERIOD_COUNT; ++period) {
        float period_energy = 0.0f;
        for (int band = 0; band < QUIETMAP_BAND_COUNT; ++band) {
            period_energy = fmaf(
                source.emission_linear[period * QUIETMAP_BAND_COUNT + band],
                transfer[band] * QUIETMAP_A_WEIGHTING_LINEAR[band], period_energy);
        }
        output_energy[period] = period_energy * divergence_linear;
    }
    return true;
}

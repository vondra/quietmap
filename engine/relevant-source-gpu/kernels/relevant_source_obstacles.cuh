//! Exact vector crossings, five-direction line quadrature, and full per-pair energy.

#pragma once

#include "relevant_source_attenuation.cuh"

__device__ __forceinline__ bool segment_crossing_fraction(
    float ray_start_x,
    float ray_start_y,
    float ray_dx,
    float ray_dy,
    float edge_start_x,
    float edge_start_y,
    float edge_end_x,
    float edge_end_y,
    float& crossing_t
) {
    const float edge_dx = edge_end_x - edge_start_x;
    const float edge_dy = edge_end_y - edge_start_y;
    const float denominator = ray_dx * edge_dy - ray_dy * edge_dx;
    if (fabsf(denominator) < 1.0e-8f) {
        return false;
    }
    const float from_ray_x = edge_start_x - ray_start_x;
    const float from_ray_y = edge_start_y - ray_start_y;
    const float ray_fraction = (from_ray_x * edge_dy - from_ray_y * edge_dx) / denominator;
    const float edge_fraction = (from_ray_x * ray_dy - from_ray_y * ray_dx) / denominator;
    if (ray_fraction <= 1.0e-7f || ray_fraction >= 1.0f - 1.0e-7f
        || edge_fraction < 0.0f || edge_fraction > 1.0f) {
        return false;
    }
    crossing_t = ray_fraction;
    return true;
}

__device__ __forceinline__ float signed_candidate_delta(
    const PathProfile& profile,
    float source_altitude_m,
    float receiver_altitude_m,
    float t,
    float top_m
) {
    const float source_elevation = profile.elevation_m[0]
        + fmaxf(
            source_altitude_m - profile.elevation_m[0],
            QUIETMAP_MINIMUM_SOURCE_HEIGHT_M);
    const float receiver_elevation = profile.elevation_m[profile.count - 1]
        + fmaxf(receiver_altitude_m - profile.elevation_m[profile.count - 1], 0.5f);
    const float direct = hypotf(profile.distance_m, receiver_elevation - source_elevation);
    const float sight = fmaf(t, receiver_elevation - source_elevation, source_elevation);
    const float detour = hypotf(t * profile.distance_m, top_m - source_elevation)
        + hypotf((1.0f - t) * profile.distance_m, top_m - receiver_elevation) - direct;
    return top_m >= sight ? detour : -detour;
}

__device__ __forceinline__ void consider_crossing_candidate(
    const PathProfile& profile,
    float source_altitude_m,
    float receiver_altitude_m,
    float t,
    float obstacle_height_m,
    DiffractionEdge& best
) {
    const float terrain_m = profile_elevation_at(profile, t);
    const float top_m = terrain_m + obstacle_height_m;
    const float delta_m = signed_candidate_delta(
        profile, source_altitude_m, receiver_altitude_m, t, top_m);
    if (!best.present || delta_m > best.delta_m) {
        best.present = true;
        best.explicit_obstacle = true;
        best.t = t;
        best.top_m = top_m;
        best.delta_m = delta_m;
        best.terrain_sample_index = 0;
    }
}

__device__ __forceinline__ bool ray_may_enter_grid(
    float start_x,
    float start_y,
    float end_x,
    float end_y,
    const DeviceObstacleGrid& grid
) {
    const float maximum_x = grid.minimum_x_m + grid.columns * grid.cell_m;
    const float maximum_y = grid.minimum_y_m + grid.rows * grid.cell_m;
    if (fmaxf(start_x, end_x) < grid.minimum_x_m
        || fminf(start_x, end_x) > maximum_x
        || fmaxf(start_y, end_y) < grid.minimum_y_m
        || fminf(start_y, end_y) > maximum_y) {
        return false;
    }
    const float dx = end_x - start_x;
    const float dy = end_y - start_y;
    float minimum_t = 0.0f;
    float maximum_t = 1.0f;
    const float starts[2] = {start_x, start_y};
    const float directions[2] = {dx, dy};
    const float minima[2] = {grid.minimum_x_m, grid.minimum_y_m};
    const float maxima[2] = {maximum_x, maximum_y};
    for (int axis = 0; axis < 2; ++axis) {
        if (fabsf(directions[axis]) < 1.0e-8f) {
            if (starts[axis] < minima[axis] || starts[axis] > maxima[axis]) {
                return false;
            }
        } else {
            float first = (minima[axis] - starts[axis]) / directions[axis];
            float second = (maxima[axis] - starts[axis]) / directions[axis];
            if (first > second) {
                const float swap = first;
                first = second;
                second = swap;
            }
            minimum_t = fmaxf(minimum_t, first);
            maximum_t = fminf(maximum_t, second);
            if (minimum_t > maximum_t) {
                return false;
            }
        }
    }
    return true;
}

#include "relevant_source_grid_scan.cuh"

__device__ __forceinline__ void scan_vector_crossings(
    const DeviceScenePointers& scene,
    float source_x_m,
    float source_y_m,
    float receiver_x_m,
    float receiver_y_m,
    float source_altitude_m,
    float receiver_altitude_m,
    const PathProfile& profile,
    DiffractionEdge& best
) {
    for (uint32_t grid_index = 0; grid_index < scene.obstacle_grid_count; ++grid_index) {
        scan_obstacle_grid(scene, scene.obstacle_grids[grid_index], source_x_m, source_y_m,
                           receiver_x_m, receiver_y_m, source_altitude_m,
                           receiver_altitude_m, profile, best);
    }
    const float ray_dx = receiver_x_m - source_x_m;
    const float ray_dy = receiver_y_m - source_y_m;
    for (uint32_t barrier_index = 0; barrier_index < scene.barrier_count; ++barrier_index) {
        const DeviceBarrier barrier = scene.barriers[barrier_index];
        if (barrier.receiver_distance_lower_bound_m > profile.distance_m + 125.0f) {
            break;
        }
        float crossing_t;
        if (segment_crossing_fraction(source_x_m, source_y_m, ray_dx, ray_dy,
                                      barrier.start_x_m, barrier.start_y_m,
                                      barrier.end_x_m, barrier.end_y_m, crossing_t)) {
            consider_crossing_candidate(profile, source_altitude_m, receiver_altitude_m,
                                        crossing_t, barrier.height_m, best);
        }
    }
}

__device__ __forceinline__ void path_diffraction_bands(
    const DeviceScenePointers& scene,
    float source_x_m,
    float source_y_m,
    float receiver_x_m,
    float receiver_y_m,
    float source_altitude_m,
    float receiver_altitude_m,
    PathProfile& profile,
    float attenuation_db[QUIETMAP_BAND_COUNT]
) {
    const float raw_receiver_ground_m = profile.elevation_m[profile.count - 1];
    clamp_source_platform_profile(profile);
    DiffractionEdge obstacle = {};
    scan_vector_crossings(scene, source_x_m, source_y_m, receiver_x_m, receiver_y_m,
                          source_altitude_m, receiver_altitude_m, profile, obstacle);
    DiffractionEdge winner = terrain_diffraction_edge(
        profile, source_altitude_m, receiver_altitude_m, raw_receiver_ground_m);
    if (obstacle.present && (!winner.present || obstacle.delta_m > winner.delta_m)) {
        complete_explicit_edge_geometry(
            profile, source_altitude_m, receiver_altitude_m, obstacle);
        winner = obstacle;
    }
    diffraction_attenuation_bands(winner, attenuation_db);
}

__device__ __forceinline__ bool point_on_segment_at_angular_fraction(
    const DeviceLineSource& source,
    float receiver_x_m,
    float receiver_y_m,
    float angular_fraction,
    float& source_x_m,
    float& source_y_m,
    float& distance_m
) {
    const float start_x = source.start_x_m - receiver_x_m;
    const float start_y = source.start_y_m - receiver_y_m;
    const float end_x = source.end_x_m - receiver_x_m;
    const float end_y = source.end_y_m - receiver_y_m;
    const float segment_x = end_x - start_x;
    const float segment_y = end_y - start_y;
    if (fmaf(segment_x, segment_x, segment_y * segment_y) < 1.0e-6f) {
        return false;
    }
    const float start_angle = atan2f(start_y, start_x);
    float span = atan2f(end_y, end_x) - start_angle;
    if (span > CUDART_PI_F) {
        span -= 2.0f * CUDART_PI_F;
    } else if (span < -CUDART_PI_F) {
        span += 2.0f * CUDART_PI_F;
    }
    const float angle = fmaf(angular_fraction, span, start_angle);
    const float direction_x = cosf(angle);
    const float direction_y = sinf(angle);
    const float denominator = direction_x * segment_y - direction_y * segment_x;
    const float numerator = direction_x * start_y - direction_y * start_x;
    const float segment_fraction = fabsf(denominator) > 1.0e-8f
        ? quietmap_clamp(-numerator / denominator, 0.0f, 1.0f) : 0.5f;
    const float local_x = fmaf(segment_fraction, segment_x, start_x);
    const float local_y = fmaf(segment_fraction, segment_y, start_y);
    distance_m = hypotf(local_x, local_y);
    source_x_m = receiver_x_m + local_x;
    source_y_m = receiver_y_m + local_y;
    return isfinite(distance_m) && distance_m >= 1.0f;
}

__device__ __forceinline__ float ground_or_barrier_attenuation_db(
    float ground_db,
    float barrier_db
) {
    return barrier_db > 0.0f ? fmaxf(ground_db, barrier_db) : ground_db;
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
    PathProfile characteristic_profile;
    build_path_profile(scene, geometry.closest_x_m, geometry.closest_y_m,
                       receiver_x_m, receiver_y_m, geometry.endpoint_distance_m,
                       source.bridge != 0, characteristic_profile);
    float ground_db[QUIETMAP_BAND_COUNT];
    ground_attenuation_bands(characteristic_profile, geometry.source_altitude_m,
                             receiver_altitude_m, ground_db);
    const float characteristic_forest_depth_m = characteristic_profile.forest_depth_m;
    float ground_or_barrier_energy[QUIETMAP_BAND_COUNT] = {};
    int used_directions = 0;
    if (scene.obstacle_grid_count > 0) {
        for (int direction = 0; direction < QUIETMAP_LINE_DIRECTION_COUNT; ++direction) {
            float direction_source_x;
            float direction_source_y;
            float direction_distance;
            if (!point_on_segment_at_angular_fraction(
                    source, receiver_x_m, receiver_y_m,
                    (static_cast<float>(direction) + 0.5f)
                        / static_cast<float>(QUIETMAP_LINE_DIRECTION_COUNT),
                    direction_source_x, direction_source_y, direction_distance)) {
                continue;
            }
            build_path_profile(scene, direction_source_x, direction_source_y,
                               receiver_x_m, receiver_y_m, direction_distance,
                               source.bridge != 0, characteristic_profile);
            const float direction_source_altitude = characteristic_profile.elevation_m[0]
                + source.source_height_m;
            float diffraction_db[QUIETMAP_BAND_COUNT];
            path_diffraction_bands(scene, direction_source_x, direction_source_y,
                                   receiver_x_m, receiver_y_m, direction_source_altitude,
                                   receiver_altitude_m, characteristic_profile,
                                   diffraction_db);
            for (int band = 0; band < QUIETMAP_BAND_COUNT; ++band) {
                ground_or_barrier_energy[band] += quietmap_energy_from_db(
                    -ground_or_barrier_attenuation_db(ground_db[band], diffraction_db[band]));
            }
            ++used_directions;
        }
    }
    if (used_directions == 0) {
        float diffraction_db[QUIETMAP_BAND_COUNT];
        path_diffraction_bands(scene, geometry.closest_x_m, geometry.closest_y_m,
                               receiver_x_m, receiver_y_m, geometry.source_altitude_m,
                               receiver_altitude_m, characteristic_profile, diffraction_db);
        for (int band = 0; band < QUIETMAP_BAND_COUNT; ++band) {
            ground_or_barrier_energy[band] = quietmap_energy_from_db(
                -ground_or_barrier_attenuation_db(ground_db[band], diffraction_db[band]));
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

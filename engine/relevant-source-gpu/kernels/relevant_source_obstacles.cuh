//! Exact vector crossings and the terrain-plus-screening verdict of one ray.

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
    float exclusion_radius_m,
    const PathProfile& profile,
    DiffractionEdge& best
) {
    for (uint32_t grid_index = 0; grid_index < scene.obstacle_grid_count; ++grid_index) {
        scan_obstacle_grid(scene, scene.obstacle_grids[grid_index], source_x_m, source_y_m,
                           receiver_x_m, receiver_y_m, source_altitude_m,
                           receiver_altitude_m, exclusion_radius_m, profile, best);
    }
    const float ray_dx = receiver_x_m - source_x_m;
    const float ray_dy = receiver_y_m - source_y_m;
    for (uint32_t barrier_index = 0; barrier_index < scene.barrier_count; ++barrier_index) {
        const DeviceBarrier barrier = scene.barriers[barrier_index];
        if (barrier.receiver_distance_lower_bound_m
            > profile.distance_m + QUIETMAP_BARRIER_PATH_HORIZON_M) {
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

/// Terrain bands and the screening increment over them on one built profile: the
/// bare-earth max-delta edge, then the exact-crossing race against it (CPU
/// terrain_attenuation + screening_attenuation, A_screen = max(A_combined - A_terrain, 0)).
/// Rays under 30 m or with fewer than three samples carry neither term; building
/// crossings inside `exclusion_radius_m` of the source are its own footprint.
__device__ __forceinline__ void ray_terrain_and_screening_bands(
    const DeviceScenePointers& scene,
    float source_x_m,
    float source_y_m,
    float receiver_x_m,
    float receiver_y_m,
    float source_altitude_m,
    float receiver_altitude_m,
    bool with_obstacles,
    float exclusion_radius_m,
    PathProfile& profile,
    float terrain_db[QUIETMAP_BAND_COUNT],
    float screening_db[QUIETMAP_BAND_COUNT]
) {
    for (int band = 0; band < QUIETMAP_BAND_COUNT; ++band) {
        terrain_db[band] = 0.0f;
        screening_db[band] = 0.0f;
    }
    if (profile.count < 3 || profile.distance_m < 30.0f) {
        return;
    }
    const float raw_receiver_ground_m = profile.elevation_m[profile.count - 1];
    clamp_source_platform_profile(profile);
    const DiffractionEdge terrain = terrain_diffraction_edge(
        profile, source_altitude_m, receiver_altitude_m, raw_receiver_ground_m);
    diffraction_attenuation_bands(terrain, terrain_db);
    if (!with_obstacles) {
        return;
    }
    DiffractionEdge obstacle = {};
    scan_vector_crossings(scene, source_x_m, source_y_m, receiver_x_m, receiver_y_m,
                          source_altitude_m, receiver_altitude_m, exclusion_radius_m,
                          profile, obstacle);
    if (obstacle.present && (!terrain.present || obstacle.delta_m > terrain.delta_m)) {
        complete_explicit_edge_geometry(
            profile, source_altitude_m, receiver_altitude_m, obstacle);
        float combined_db[QUIETMAP_BAND_COUNT];
        diffraction_attenuation_bands(obstacle, combined_db);
        for (int band = 0; band < QUIETMAP_BAND_COUNT; ++band) {
            screening_db[band] = fmaxf(combined_db[band] - terrain_db[band], 0.0f);
        }
    }
}

//! Height-bounded obstacle-cell traversal for one full-physics source/receiver ray.

#pragma once

__device__ __forceinline__ float signed_delta_for_bounded_top(
    const PathProfile& profile,
    float source_altitude_m,
    float receiver_altitude_m,
    float top_m,
    float t
) {
    const float sight_m = fmaf(t, receiver_altitude_m - source_altitude_m,
                               source_altitude_m);
    const float direct_m = hypotf(profile.distance_m,
                                  receiver_altitude_m - source_altitude_m);
    const float detour_m = hypotf(t * profile.distance_m, top_m - source_altitude_m)
        + hypotf((1.0f - t) * profile.distance_m, top_m - receiver_altitude_m)
        - direct_m;
    return top_m >= sight_m ? detour_m : -detour_m;
}

__device__ __forceinline__ float maximum_cell_candidate_delta(
    const PathProfile& profile,
    float source_altitude_m,
    float receiver_altitude_m,
    float top_m,
    float minimum_t,
    float maximum_t
) {
    const float source_height_difference = fabsf(top_m - source_altitude_m);
    const float receiver_height_difference = fabsf(top_m - receiver_altitude_m);
    const float height_sum = source_height_difference + receiver_height_difference;
    const float reflection_t = height_sum > 0.0f
        ? quietmap_clamp(source_height_difference / height_sum, minimum_t, maximum_t)
        : minimum_t;
    return fmaxf(
        fmaxf(
            signed_delta_for_bounded_top(
                profile, source_altitude_m, receiver_altitude_m, top_m, minimum_t),
            signed_delta_for_bounded_top(
                profile, source_altitude_m, receiver_altitude_m, top_m, maximum_t)),
        signed_delta_for_bounded_top(
            profile, source_altitude_m, receiver_altitude_m, top_m, reflection_t));
}

/// Every obstacle crossing of one ray, keeping the one with the largest delta.
///
/// `must_exceed_m` is the delta a crossing has to beat for the caller to read it at all;
/// a cell whose own bound cannot reach it is never opened.
__device__ __forceinline__ void scan_obstacle_grid(
    const DeviceScenePointers& scene,
    const DeviceObstacleGrid& grid,
    float source_x_m,
    float source_y_m,
    float receiver_x_m,
    float receiver_y_m,
    float source_altitude_m,
    float receiver_altitude_m,
    float exclusion_radius_m,
    float must_exceed_m,
    const PathProfile& profile,
    DiffractionEdge& best
) {
    const float start_x = fmaf(source_x_m, grid.query_x_scale, grid.query_x_offset_m);
    const float start_y = source_y_m + grid.query_y_offset_m;
    const float end_x = fmaf(receiver_x_m, grid.query_x_scale, grid.query_x_offset_m);
    const float end_y = receiver_y_m + grid.query_y_offset_m;
    if (!ray_may_enter_grid(start_x, start_y, end_x, end_y, grid)) {
        return;
    }
    const float dx = end_x - start_x;
    const float dy = end_y - start_y;
    const float inverse_cell = 1.0f / grid.cell_m;
    int cell_x = max(0, min(static_cast<int>(floorf((start_x - grid.minimum_x_m) * inverse_cell)),
                            static_cast<int>(grid.columns) - 1));
    int cell_y = max(0, min(static_cast<int>(floorf((start_y - grid.minimum_y_m) * inverse_cell)),
                            static_cast<int>(grid.rows) - 1));
    const int end_cell_x = max(0, min(
        static_cast<int>(floorf((end_x - grid.minimum_x_m) * inverse_cell)),
        static_cast<int>(grid.columns) - 1));
    const int end_cell_y = max(0, min(
        static_cast<int>(floorf((end_y - grid.minimum_y_m) * inverse_cell)),
        static_cast<int>(grid.rows) - 1));
    const int step_x = dx >= 0.0f ? 1 : -1;
    const int step_y = dy >= 0.0f ? 1 : -1;
    const float delta_t_x = dx != 0.0f ? fabsf(grid.cell_m / dx) : CUDART_INF_F;
    const float delta_t_y = dy != 0.0f ? fabsf(grid.cell_m / dy) : CUDART_INF_F;
    const float next_x = grid.minimum_x_m + (cell_x + (dx >= 0.0f ? 1 : 0)) * grid.cell_m;
    const float next_y = grid.minimum_y_m + (cell_y + (dy >= 0.0f ? 1 : 0)) * grid.cell_m;
    float maximum_t_x = dx != 0.0f ? fabsf((next_x - start_x) / dx) : CUDART_INF_F;
    float maximum_t_y = dy != 0.0f ? fabsf((next_y - start_y) / dy) : CUDART_INF_F;
    int guard = static_cast<int>(grid.columns + grid.rows) + 4;
    float entry_t = 0.0f;
    int profile_window_start = 0;
    while (guard-- > 0) {
        const uint32_t cell = static_cast<uint32_t>(cell_y) * grid.columns + cell_x;
        const uint32_t first = scene.obstacle_cell_starts[grid.cell_starts_offset + cell];
        const uint32_t end = scene.obstacle_cell_starts[grid.cell_starts_offset + cell + 1];
        if (end > first) {
            const float exit_t = fminf(fminf(maximum_t_x, maximum_t_y), 1.0f);
            const float cell_minimum_t = quietmap_clamp(entry_t, 0.0f, 1.0f);
            const float cell_maximum_t = quietmap_clamp(exit_t, 0.0f, 1.0f);
            while (profile_window_start + 1 < profile.count
                   && profile.t[profile_window_start + 1] <= cell_minimum_t) {
                ++profile_window_start;
            }
            float terrain_maximum_m = profile.elevation_m[profile_window_start];
            int profile_window_end = profile_window_start;
            while (profile_window_end + 1 < profile.count
                   && profile.t[profile_window_end] < cell_maximum_t) {
                ++profile_window_end;
                terrain_maximum_m = fmaxf(
                    terrain_maximum_m, profile.elevation_m[profile_window_end]);
            }
            const float top_bound_m = terrain_maximum_m
                + scene.obstacle_cell_maximum_heights[
                    grid.cell_maximum_height_offset + cell];
            // The loop below is where a dense metro ray spends nearly all of its GPU
            // seconds — on the kbench downtown window, skipping it takes the paint from 84
            // to 5 GPU seconds — so the cell bound gates on both floors it can.
            // `maximum_cell_candidate_delta` bounds the delta of every crossing this cell
            // holds from above: the delta rises with the obstacle top, and `top_bound_m` is
            // the cell's tallest edge over the highest terrain the ray meets inside it;
            // the detour is convex in `t`, so over the cell's own stretch of the ray its
            // extremes are the two ends and the reflection point, the three the bound
            // evaluates. A cell that cannot reach the penumbra floor diffracts nothing in
            // any band, and a cell that cannot out-diffract the terrain edge produces
            // nothing the caller reads: `ray_terrain_and_screening_bands` takes the
            // obstacle edge only where it beats the terrain one.
            const float cell_bound_m = maximum_cell_candidate_delta(
                profile, source_altitude_m, receiver_altitude_m, top_bound_m,
                cell_minimum_t, cell_maximum_t);
            if (cell_bound_m >= QUIETMAP_PENUMBRA_DELTA_FLOOR_M
                && cell_bound_m > must_exceed_m) {
                for (uint32_t position = first; position < end; ++position) {
                    const uint32_t local_edge = scene.obstacle_edge_references[
                        grid.edge_references_offset + position];
                    const uint32_t edge = grid.edge_values_offset + local_edge;
                    const float* values = scene.obstacle_edge_values_xyxyh + edge * 5;
                    float crossing_t;
                    // The exclusion radius shields only the source's own BUILDING
                    // footprint; a barrier edge is an explicit wall and always
                    // admits (CPU path_effects.rs §5b kind rule).
                    if (segment_crossing_fraction(
                            start_x, start_y, dx, dy, values[0], values[1],
                            values[2], values[3], crossing_t)
                        && (scene.obstacle_edge_is_building[edge] == 0u
                            || crossing_t * profile.distance_m >= exclusion_radius_m)) {
                        // The cell's own window already brackets `crossing_t` whenever the
                        // crossing lies inside the cell, which is where the walk finds
                        // nearly all of them; a crossing further along the edge than the
                        // cell simply falls back on the whole profile.
                        consider_crossing_candidate(
                            profile, source_altitude_m, receiver_altitude_m,
                            crossing_t, values[4], profile_window_start, best);
                    }
                }
            }
        }
        if (cell_x == end_cell_x && cell_y == end_cell_y) {
            break;
        }
        if (maximum_t_x < maximum_t_y) {
            entry_t = maximum_t_x;
            maximum_t_x += delta_t_x;
            cell_x += step_x;
        } else {
            entry_t = maximum_t_y;
            maximum_t_y += delta_t_y;
            cell_y += step_y;
        }
        if (cell_x < 0 || cell_y < 0
            || cell_x >= static_cast<int>(grid.columns)
            || cell_y >= static_cast<int>(grid.rows)) {
            break;
        }
    }
}

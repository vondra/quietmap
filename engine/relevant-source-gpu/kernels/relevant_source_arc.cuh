//! Arc-clipped screening of one wide fan bucket: the CUDA form of noise-compute
//! arc_screening.rs `arc_screened_eval` under the etalon's exact bounds.
//!
//! The receiver's skyline is the set of obstacle edges and noise walls standing in
//! front of the sub-segment; their azimuth arcs, clipped to the bucket's span, are
//! unioned into a fixed-resolution blocked mask over the span (its bins are never
//! wider than the CPU's own ARC_QUADRATURE_MIN_RAD coalescing floor for a bucket).
//! Every blocked run and every clear gap is evaluated by one ray per part of at
//! most ESCALATE_SPAN_RAD, energy-averaged on max(A_ground, A_terrain + A_screen),
//! and handed back as the non-negative increment over the bucket ray's terrain.

#pragma once

#include "relevant_source_obstacles.cuh"

constexpr int QUIETMAP_ARC_MASK_BINS = 128;
constexpr int QUIETMAP_ARC_MASK_WORDS = QUIETMAP_ARC_MASK_BINS / 32;
// A bucket spans at most pi / bucket count, so a bin is never wider than the
// window the CPU itself coalesces away.
static_assert(
    QUIETMAP_ARC_MASK_BINS * QUIETMAP_ARC_QUADRATURE_MIN_RAD
        >= CUDART_PI_F / QUIETMAP_LINE_DIRECTION_COUNT,
    "arc mask bins coarser than the CPU quadrature floor");

struct ArcMask {
    uint32_t bits[QUIETMAP_ARC_MASK_WORDS];
};

__device__ __forceinline__ float wrap_to_pi(float angle) {
    while (angle > CUDART_PI_F) {
        angle -= 2.0f * CUDART_PI_F;
    }
    while (angle <= -CUDART_PI_F) {
        angle += 2.0f * CUDART_PI_F;
    }
    return angle;
}

/// The point on the segment seen from the receiver at `azimuth`: ray x line, the
/// solved fraction clamped to the segment (CPU `SegFan::at` / `source_point_at`).
__device__ __forceinline__ bool segment_point_at_azimuth(
    const DeviceLineSource& source,
    float receiver_x_m,
    float receiver_y_m,
    float azimuth,
    float& point_x_m,
    float& point_y_m,
    float& distance_m
) {
    const float start_x = source.start_x_m - receiver_x_m;
    const float start_y = source.start_y_m - receiver_y_m;
    const float segment_x = source.end_x_m - source.start_x_m;
    const float segment_y = source.end_y_m - source.start_y_m;
    const float direction_x = cosf(azimuth);
    const float direction_y = sinf(azimuth);
    const float denominator = direction_x * segment_y - direction_y * segment_x;
    if (fabsf(denominator) < 1.0e-12f) {
        return false;
    }
    const float fraction = quietmap_clamp(
        (direction_y * start_x - direction_x * start_y) / denominator, 0.0f, 1.0f);
    const float local_x = fmaf(fraction, segment_x, start_x);
    const float local_y = fmaf(fraction, segment_y, start_y);
    distance_m = hypotf(local_x, local_y);
    point_x_m = receiver_x_m + local_x;
    point_y_m = receiver_y_m + local_y;
    return isfinite(distance_m) && distance_m >= 1.0f;
}

__device__ __forceinline__ float origin_to_segment_distance(
    float x0, float y0, float x1, float y1
) {
    const float edge_x = x1 - x0;
    const float edge_y = y1 - y0;
    const float length_squared = fmaf(edge_x, edge_x, edge_y * edge_y);
    const float t = length_squared > 0.0f
        ? quietmap_clamp(-(x0 * edge_x + y0 * edge_y) / length_squared, 0.0f, 1.0f)
        : 0.0f;
    return hypotf(fmaf(t, edge_x, x0), fmaf(t, edge_y, y0));
}

/// Mark the bins of `[piece_lo, piece_hi]` (absolute azimuths inside the span).
__device__ __forceinline__ void mark_arc_bins(
    ArcMask& mask,
    float span_lo,
    float bin_width,
    float piece_lo,
    float piece_hi
) {
    int first = static_cast<int>(floorf((piece_lo - span_lo) / bin_width));
    int last = static_cast<int>(ceilf((piece_hi - span_lo) / bin_width)) - 1;
    first = max(first, 0);
    last = min(max(last, first), QUIETMAP_ARC_MASK_BINS - 1);
    for (int bin = first; bin <= last; ++bin) {
        mask.bits[bin >> 5] |= 1u << (bin & 31);
    }
}

/// One skyline arc (an edge or wall in the receiver frame) clipped to the span:
/// every piece whose obstacle stands in front of the source point seen at the
/// piece's centre and not under the receiver's feet is marked blocked
/// (CPU `arc_screened_eval` step 1: geometry only, no delta prefilter).
__device__ __forceinline__ void admit_skyline_arc(
    const DeviceLineSource& source,
    float receiver_x_m,
    float receiver_y_m,
    float edge_x0,
    float edge_y0,
    float edge_x1,
    float edge_y1,
    float need_radius_m,
    float span_lo,
    float span_hi,
    float bin_width,
    ArcMask& mask
) {
    const float nearest_m = origin_to_segment_distance(edge_x0, edge_y0, edge_x1, edge_y1);
    if (nearest_m > need_radius_m || nearest_m < 1.0e-6f) {
        return;
    }
    const float azimuth0 = atan2f(edge_y0, edge_x0);
    const float azimuth1 = azimuth0 + wrap_to_pi(atan2f(edge_y1, edge_x1) - azimuth0);
    const float arc_lo = fminf(azimuth0, azimuth1);
    const float arc_hi = fmaxf(azimuth0, azimuth1);
    for (int shift = 0; shift < 3; ++shift) {
        const float offset = shift == 0 ? 0.0f
            : (shift == 1 ? 2.0f * CUDART_PI_F : -2.0f * CUDART_PI_F);
        const float piece_lo = fmaxf(arc_lo + offset, span_lo);
        const float piece_hi = fminf(arc_hi + offset, span_hi);
        if (piece_hi <= piece_lo) {
            continue;
        }
        float point_x;
        float point_y;
        float source_distance_m;
        if (!segment_point_at_azimuth(source, receiver_x_m, receiver_y_m,
                                      0.5f * (piece_lo + piece_hi), point_x, point_y,
                                      source_distance_m)) {
            continue;
        }
        if (source_distance_m - nearest_m <= 1.0f || nearest_m < 1.0f) {
            continue;
        }
        mark_arc_bins(mask, span_lo, bin_width, piece_lo, piece_hi);
    }
}

/// The blocked mask of the bucket span from every obstacle grid and wall within
/// `need_radius_m` (CPU `skyline_arcs_within` + the wall half of `ensure`): a cell
/// is pruned on its tallest edge against the sight-line floor, an edge is not.
__device__ void gather_blocked_mask(
    const DeviceScenePointers& scene,
    const DeviceLineSource& source,
    float receiver_x_m,
    float receiver_y_m,
    float need_radius_m,
    float sight_line_floor_m,
    float span_lo,
    float span_hi,
    float bin_width,
    ArcMask& mask
) {
    for (int word = 0; word < QUIETMAP_ARC_MASK_WORDS; ++word) {
        mask.bits[word] = 0u;
    }
    const float low_x = cosf(span_lo);
    const float low_y = sinf(span_lo);
    const float high_x = cosf(span_hi);
    const float high_y = sinf(span_hi);
    const float need_squared = need_radius_m * need_radius_m;
    for (uint32_t grid_index = 0; grid_index < scene.obstacle_grid_count; ++grid_index) {
        const DeviceObstacleGrid grid = scene.obstacle_grids[grid_index];
        const float inverse_scale = 1.0f / grid.query_x_scale;
        const float receiver_grid_x = fmaf(receiver_x_m, grid.query_x_scale, grid.query_x_offset_m);
        const float receiver_grid_y = receiver_y_m + grid.query_y_offset_m;
        const float radius_grid = need_radius_m * fmaxf(grid.query_x_scale, 1.0f);
        const float inverse_cell = 1.0f / grid.cell_m;
        const int column_first = max(
            static_cast<int>(floorf((receiver_grid_x - radius_grid - grid.minimum_x_m) * inverse_cell)), 0);
        const int column_last = min(
            static_cast<int>(floorf((receiver_grid_x + radius_grid - grid.minimum_x_m) * inverse_cell)),
            static_cast<int>(grid.columns) - 1);
        const int row_first = max(
            static_cast<int>(floorf((receiver_grid_y - radius_grid - grid.minimum_y_m) * inverse_cell)), 0);
        const int row_last = min(
            static_cast<int>(floorf((receiver_grid_y + radius_grid - grid.minimum_y_m) * inverse_cell)),
            static_cast<int>(grid.rows) - 1);
        for (int row = row_first; row <= row_last; ++row) {
            const float cell_south = grid.minimum_y_m + row * grid.cell_m - receiver_grid_y;
            const float cell_north = cell_south + grid.cell_m;
            const float dy = fmaxf(fmaxf(cell_south, -cell_north), 0.0f);
            for (int column = column_first; column <= column_last; ++column) {
                const uint32_t cell = static_cast<uint32_t>(row) * grid.columns + column;
                const uint32_t first = scene.obstacle_cell_starts[grid.cell_starts_offset + cell];
                const uint32_t end = scene.obstacle_cell_starts[grid.cell_starts_offset + cell + 1];
                if (first == end) {
                    continue;
                }
                const float cell_west = (grid.minimum_x_m + column * grid.cell_m - receiver_grid_x)
                    * inverse_scale;
                const float cell_east = cell_west + grid.cell_m * inverse_scale;
                const float dx = fmaxf(fmaxf(cell_west, -cell_east), 0.0f);
                if (fmaf(dx, dx, dy * dy) > need_squared) {
                    continue;
                }
                bool all_below = true;
                bool all_above = true;
                const float corner_x[4] = {cell_west, cell_east, cell_west, cell_east};
                const float corner_y[4] = {cell_south, cell_south, cell_north, cell_north};
                for (int corner = 0; corner < 4; ++corner) {
                    if (low_x * corner_y[corner] - low_y * corner_x[corner] >= 0.0f) {
                        all_below = false;
                    }
                    if (corner_x[corner] * high_y - corner_y[corner] * high_x >= 0.0f) {
                        all_above = false;
                    }
                }
                if (all_below || all_above) {
                    continue;
                }
                if (scene.obstacle_cell_maximum_heights[grid.cell_maximum_height_offset + cell]
                    <= sight_line_floor_m) {
                    continue;
                }
                for (uint32_t position = first; position < end; ++position) {
                    const uint32_t local_edge = scene.obstacle_edge_references[
                        grid.edge_references_offset + position];
                    const float* values = scene.obstacle_edge_values_xyxyh
                        + (grid.edge_values_offset + local_edge) * 5;
                    admit_skyline_arc(
                        source, receiver_x_m, receiver_y_m,
                        (values[0] - receiver_grid_x) * inverse_scale, values[1] - receiver_grid_y,
                        (values[2] - receiver_grid_x) * inverse_scale, values[3] - receiver_grid_y,
                        need_radius_m, span_lo, span_hi, bin_width, mask);
                }
            }
        }
    }
    for (uint32_t barrier_index = 0; barrier_index < scene.barrier_count; ++barrier_index) {
        const DeviceBarrier barrier = scene.barriers[barrier_index];
        if (barrier.receiver_distance_lower_bound_m
            > need_radius_m + QUIETMAP_BARRIER_PATH_HORIZON_M) {
            break;
        }
        if (barrier.height_m <= sight_line_floor_m) {
            continue;
        }
        admit_skyline_arc(
            source, receiver_x_m, receiver_y_m,
            barrier.start_x_m - receiver_x_m, barrier.start_y_m - receiver_y_m,
            barrier.end_x_m - receiver_x_m, barrier.end_y_m - receiver_y_m,
            need_radius_m, span_lo, span_hi, bin_width, mask);
    }
}

/// Terrain and screening increment on the ray to the source point at `azimuth`
/// (CPU `interval_screening`; `interval_terrain` when `with_obstacles` is false).
/// False when the ray is degenerate, where the caller keeps the bucket ray's values.
__device__ __forceinline__ bool azimuth_ray_bands(
    const DeviceScenePointers& scene,
    const DeviceLineSource& source,
    float receiver_x_m,
    float receiver_y_m,
    float receiver_altitude_m,
    float azimuth,
    bool with_obstacles,
    PathProfile& profile,
    float terrain_db[QUIETMAP_BAND_COUNT],
    float screening_db[QUIETMAP_BAND_COUNT]
) {
    float point_x;
    float point_y;
    float distance_m;
    if (!segment_point_at_azimuth(source, receiver_x_m, receiver_y_m, azimuth,
                                  point_x, point_y, distance_m)) {
        return false;
    }
    build_path_profile(scene, point_x, point_y, receiver_x_m, receiver_y_m, distance_m,
                       source.bridge != 0, profile);
    ray_terrain_and_screening_bands(
        scene, point_x, point_y, receiver_x_m, receiver_y_m,
        profile.elevation_m[0] + source.source_height_m, receiver_altitude_m,
        with_obstacles, profile, terrain_db, screening_db);
    return true;
}

/// Energy of one part of the fan: max(A_ground, A_terrain + A_screen) on the
/// part's own ray, weighted by its share of the span.
__device__ __forceinline__ void accumulate_fan_part(
    const float ground_db[QUIETMAP_BAND_COUNT],
    const float terrain_db[QUIETMAP_BAND_COUNT],
    const float screening_db[QUIETMAP_BAND_COUNT],
    float fraction,
    float energy[QUIETMAP_BAND_COUNT]
) {
    for (int band = 0; band < QUIETMAP_BAND_COUNT; ++band) {
        const float barrier = terrain_db[band] + screening_db[band];
        const float composite = barrier > 0.0f ? fmaxf(ground_db[band], barrier) : ground_db[band];
        energy[band] += fraction * quietmap_energy_from_db(-composite);
    }
}

__device__ __forceinline__ int fan_part_count(float width) {
    return min(max(static_cast<int>(ceilf(width / QUIETMAP_ARC_ESCALATE_SPAN_RAD)), 1),
               QUIETMAP_ARC_ESCALATE_MAX_PARTS);
}

/// The arc-clipped screening increment of one bucket over the terrain of its
/// centre ray, or the centre ray's own increment when the sub-span is degenerate,
/// under the 3 degree gate, or nothing blocks it (CPU `arc_screened_eval`).
__device__ void arc_screened_bucket_increment(
    const DeviceScenePointers& scene,
    const DeviceLineSource& source,
    float receiver_x_m,
    float receiver_y_m,
    float receiver_altitude_m,
    float bucket_start_x_m,
    float bucket_start_y_m,
    float bucket_end_x_m,
    float bucket_end_y_m,
    float centre_azimuth,
    float need_radius_m,
    const float ground_db[QUIETMAP_BAND_COUNT],
    const float centre_terrain_db[QUIETMAP_BAND_COUNT],
    PathProfile& profile,
    float screening_db[QUIETMAP_BAND_COUNT]
) {
    const float base = atan2f(bucket_start_y_m - receiver_y_m, bucket_start_x_m - receiver_x_m);
    const float delta = wrap_to_pi(
        atan2f(bucket_end_y_m - receiver_y_m, bucket_end_x_m - receiver_x_m) - base);
    const float span = fabsf(delta);
    if (span < QUIETMAP_ARC_DEGENERATE_SPAN_RAD || span < QUIETMAP_SEG_ARC_MIN_SPAN_RAD) {
        return;
    }
    const float span_lo = delta < 0.0f ? base + delta : base;
    const float span_hi = delta < 0.0f ? base : base + delta;
    const float bin_width = span / QUIETMAP_ARC_MASK_BINS;
    ArcMask mask;
    gather_blocked_mask(scene, source, receiver_x_m, receiver_y_m, need_radius_m,
                        fmaxf(source.source_height_m, 0.0f), span_lo, span_hi, bin_width, mask);
    bool blocked = false;
    for (int word = 0; word < QUIETMAP_ARC_MASK_WORDS; ++word) {
        blocked |= mask.bits[word] != 0u;
    }
    if (!blocked) {
        return;
    }
    float cp_azimuth = centre_azimuth;
    for (int shift = 0; shift < 3; ++shift) {
        const float shifted = centre_azimuth
            + (shift == 0 ? 0.0f : (shift == 1 ? 2.0f * CUDART_PI_F : -2.0f * CUDART_PI_F));
        if (shifted >= span_lo - QUIETMAP_ARC_CP_AZIMUTH_EPS
            && shifted <= span_hi + QUIETMAP_ARC_CP_AZIMUTH_EPS) {
            cp_azimuth = shifted;
            break;
        }
    }
    cp_azimuth = quietmap_clamp(cp_azimuth, span_lo, span_hi);

    float centre_screening_db[QUIETMAP_BAND_COUNT];
    for (int band = 0; band < QUIETMAP_BAND_COUNT; ++band) {
        centre_screening_db[band] = screening_db[band];
    }
    const float zero_db[QUIETMAP_BAND_COUNT] = {};
    float energy[QUIETMAP_BAND_COUNT] = {};
    float covered = 0.0f;
    float terrain_db[QUIETMAP_BAND_COUNT];
    float part_screening_db[QUIETMAP_BAND_COUNT];
    int bin = 0;
    while (bin < QUIETMAP_ARC_MASK_BINS) {
        const bool run_blocked = (mask.bits[bin >> 5] >> (bin & 31)) & 1u;
        int run_end = bin;
        while (run_end < QUIETMAP_ARC_MASK_BINS
               && (((mask.bits[run_end >> 5] >> (run_end & 31)) & 1u) != 0u) == run_blocked) {
            ++run_end;
        }
        const float run_lo = fmaf(static_cast<float>(bin), bin_width, span_lo);
        const float run_hi = run_end == QUIETMAP_ARC_MASK_BINS
            ? span_hi : fmaf(static_cast<float>(run_end), bin_width, span_lo);
        const float width = run_hi - run_lo;
        const int parts = fan_part_count(width);
        const float step = width / static_cast<float>(parts);
        for (int part = 0; part < parts; ++part) {
            const float part_lo = fmaf(static_cast<float>(part), step, run_lo);
            const float part_hi = part_lo + step;
            const float fraction = step / span;
            covered += fraction;
            if (run_blocked
                && cp_azimuth >= part_lo - QUIETMAP_ARC_CP_AZIMUTH_EPS
                && cp_azimuth <= part_hi + QUIETMAP_ARC_CP_AZIMUTH_EPS) {
                accumulate_fan_part(ground_db, centre_terrain_db, centre_screening_db, fraction, energy);
                continue;
            }
            if (azimuth_ray_bands(scene, source, receiver_x_m, receiver_y_m, receiver_altitude_m,
                                  0.5f * (part_lo + part_hi), run_blocked, profile,
                                  terrain_db, part_screening_db)) {
                accumulate_fan_part(ground_db, terrain_db,
                                    run_blocked ? part_screening_db : zero_db, fraction, energy);
            } else {
                accumulate_fan_part(ground_db, centre_terrain_db,
                                    run_blocked ? centre_screening_db : zero_db, fraction, energy);
            }
        }
        bin = run_end;
    }
    const float residual = fmaxf(1.0f - covered, 0.0f);
    for (int band = 0; band < QUIETMAP_BAND_COUNT; ++band) {
        const float clear = centre_terrain_db[band] > 0.0f
            ? fmaxf(ground_db[band], centre_terrain_db[band]) : ground_db[band];
        const float mean_db = -4.342944819032518f * __logf(
            fmaxf(energy[band] + residual * quietmap_energy_from_db(-clear), 1.0e-12f));
        screening_db[band] = fmaxf(mean_db - centre_terrain_db[band], 0.0f);
    }
}

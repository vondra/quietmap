//! The line piece as a 3D line and the geometry-placed nodes of a wide quadrature bucket: the
//! CUDA form of noise-compute line_quadrature.rs.
//!
//! Obstacle edges standing in front of the piece mark a blocked mask over a wide bucket's
//! horizontal azimuths; every blocked run and clear gap is split into parts of at most
//! WIDE_BUCKET_PART_MAX_SPAN_RAD, each part one node weighted by its own in-plane angle.

#pragma once

#include "relevant_source_obstacles.cuh"

static_assert(QUIETMAP_WIDE_BUCKET_MASK_BINS % 32 == 0, "mask words");
constexpr int QUIETMAP_WIDE_BUCKET_MASK_WORDS = QUIETMAP_WIDE_BUCKET_MASK_BINS / 32;

struct ArcMask {
    uint32_t bits[QUIETMAP_WIDE_BUCKET_MASK_WORDS];
};

__device__ __forceinline__ bool arc_mask_bin(const ArcMask& mask, int bin) {
    return ((mask.bits[bin >> 5] >> (bin & 31)) & 1u) != 0u;
}

__device__ __forceinline__ float wrap_to_pi(float angle) {
    while (angle > CUDART_PI_F) {
        angle -= 2.0f * CUDART_PI_F;
    }
    while (angle <= -CUDART_PI_F) {
        angle += 2.0f * CUDART_PI_F;
    }
    return angle;
}

/// A straight piece relative to the receiver: x east, y north, z above the receiver.
struct LinePieceGeometry {
    float start[3];
    float unit[3];
    float length_m;
    float foot_along_m;
    float perpendicular_m;
    float start_angle_rad;
    float end_angle_rad;
};

/// The piece from its endpoints' ground plus the source height (CPU LinePieceGeometry::new);
/// false for a piece shorter than a millimetre.
__device__ __forceinline__ bool line_piece_geometry(
    const DeviceScenePointers& scene,
    const DeviceLineSource& source,
    float receiver_x_m,
    float receiver_y_m,
    float receiver_altitude_m,
    LinePieceGeometry& g
) {
    g.start[0] = source.start_x_m - receiver_x_m;
    g.start[1] = source.start_y_m - receiver_y_m;
    g.start[2] = sample_scene_raster(scene, source.start_x_m, source.start_y_m).elevation_m
                 + source.source_height_m - receiver_altitude_m;
    const float end_z = sample_scene_raster(scene, source.end_x_m, source.end_y_m).elevation_m
                        + source.source_height_m - receiver_altitude_m;
    const float along[3] = {source.end_x_m - source.start_x_m, source.end_y_m - source.start_y_m,
                            end_z - g.start[2]};
    g.length_m = sqrtf(fmaf(along[0], along[0], fmaf(along[1], along[1], along[2] * along[2])));
    if (g.length_m < 1.0e-3f) {
        return false;
    }
    for (int axis = 0; axis < 3; ++axis) {
        g.unit[axis] = along[axis] / g.length_m;
    }
    g.foot_along_m = -(g.start[0] * g.unit[0] + g.start[1] * g.unit[1] + g.start[2] * g.unit[2]);
    float foot_sq = 0.0f;
    for (int axis = 0; axis < 3; ++axis) {
        const float foot = fmaf(g.foot_along_m, g.unit[axis], g.start[axis]);
        foot_sq = fmaf(foot, foot, foot_sq);
    }
    g.perpendicular_m = fmaxf(sqrtf(foot_sq), QUIETMAP_LINE_PERPENDICULAR_FLOOR_M);
    g.start_angle_rad = atanf(-g.foot_along_m / g.perpendicular_m);
    g.end_angle_rad = atanf((g.length_m - g.foot_along_m) / g.perpendicular_m);
    return true;
}

__device__ __forceinline__ float line_subtended_angle(const LinePieceGeometry& g) {
    return g.end_angle_rad - g.start_angle_rad;
}

__device__ __forceinline__ float line_in_plane_angle_at(const LinePieceGeometry& g, float along_m) {
    return atanf((along_m - g.foot_along_m) / g.perpendicular_m);
}

__device__ __forceinline__ float line_along_at_angle(const LinePieceGeometry& g, float angle_rad) {
    return quietmap_clamp(fmaf(g.perpendicular_m, tanf(angle_rad), g.foot_along_m), 0.0f, g.length_m);
}

__device__ __forceinline__ void line_point_at(const LinePieceGeometry& g, float along_m, float point[3]) {
    for (int axis = 0; axis < 3; ++axis) {
        point[axis] = fmaf(along_m, g.unit[axis], g.start[axis]);
    }
}

/// Where the horizontal ray at `azimuth` meets the piece's ground track, clamped to the piece.
__device__ __forceinline__ bool line_along_at_azimuth(
    const LinePieceGeometry& g,
    float azimuth,
    float& along_m
) {
    const float direction_x = cosf(azimuth);
    const float direction_y = sinf(azimuth);
    const float track_x = g.unit[0] * g.length_m;
    const float track_y = g.unit[1] * g.length_m;
    const float denominator = direction_x * track_y - direction_y * track_x;
    if (fabsf(denominator) < 1.0e-12f) {
        return false;
    }
    const float fraction = (direction_y * g.start[0] - direction_x * g.start[1]) / denominator;
    along_m = quietmap_clamp(fraction, 0.0f, 1.0f) * g.length_m;
    return true;
}

__device__ __forceinline__ float line_azimuth_at(const LinePieceGeometry& g, float along_m) {
    float point[3];
    line_point_at(g, along_m, point);
    return atan2f(point[1], point[0]);
}

__device__ __forceinline__ float line_horizontal_range_at(const LinePieceGeometry& g, float along_m) {
    float point[3];
    line_point_at(g, along_m, point);
    return hypotf(point[0], point[1]);
}

/// Horizontal distance from the receiver to the nearest point of the piece.
__device__ __forceinline__ bool line_closest_horizontal_distance(
    const DeviceLineSource& source,
    float receiver_x_m,
    float receiver_y_m,
    float& distance_m
) {
    const float segment_x = source.end_x_m - source.start_x_m;
    const float segment_y = source.end_y_m - source.start_y_m;
    const float length_squared = fmaf(segment_x, segment_x, segment_y * segment_y);
    const float fraction = length_squared > 1.0e-10f
        ? quietmap_clamp(((receiver_x_m - source.start_x_m) * segment_x
                          + (receiver_y_m - source.start_y_m) * segment_y) / length_squared,
                         0.0f, 1.0f)
        : 0.0f;
    distance_m = hypotf(receiver_x_m - fmaf(fraction, segment_x, source.start_x_m),
                        receiver_y_m - fmaf(fraction, segment_y, source.start_y_m));
    return isfinite(distance_m);
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
    last = min(max(last, first), QUIETMAP_WIDE_BUCKET_MASK_BINS - 1);
    for (int bin = first; bin <= last; ++bin) {
        mask.bits[bin >> 5] |= 1u << (bin & 31);
    }
}

/// One edge's arc clipped to the span: every piece whose edge stands at least a metre in front
/// of the source point seen at the piece's centre marks its bins (CPU mark_blocked_bins).
__device__ __forceinline__ void admit_skyline_arc(
    const LinePieceGeometry& geometry,
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
    const float edge_x = edge_x1 - edge_x0;
    const float edge_y = edge_y1 - edge_y0;
    const float length_squared = fmaf(edge_x, edge_x, edge_y * edge_y);
    const float t = length_squared > 0.0f
        ? quietmap_clamp(-(edge_x0 * edge_x + edge_y0 * edge_y) / length_squared, 0.0f, 1.0f)
        : 0.0f;
    const float nearest_m = hypotf(fmaf(t, edge_x, edge_x0), fmaf(t, edge_y, edge_y0));
    if (nearest_m > need_radius_m || nearest_m < 1.0f) {
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
        float along_m;
        if (!line_along_at_azimuth(geometry, 0.5f * (piece_lo + piece_hi), along_m)) {
            continue;
        }
        if (line_horizontal_range_at(geometry, along_m) - nearest_m <= 1.0f) {
            continue;
        }
        mark_arc_bins(mask, span_lo, bin_width, piece_lo, piece_hi);
    }
}

/// The blocked mask of a bucket span from every obstacle edge within `need_radius_m` (CPU
/// ObstacleSet::skyline_arcs_within with no grazing prune): a building cell is pruned on its
/// tallest edge against the source height, a wall on its own height.
__device__ void gather_blocked_mask(
    const DeviceScenePointers& scene,
    const LinePieceGeometry& geometry,
    float receiver_x_m,
    float receiver_y_m,
    float need_radius_m,
    float sight_line_floor_m,
    float span_lo,
    float span_hi,
    float bin_width,
    ArcMask& mask
) {
    for (int word = 0; word < QUIETMAP_WIDE_BUCKET_MASK_WORDS; ++word) {
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
                    const uint32_t edge = grid.edge_index_offset + local_edge;
                    if (scene.obstacle_edge_is_building[edge] == 0u
                        && scene.obstacle_edge_height_m[edge] <= sight_line_floor_m) {
                        continue;
                    }
                    const float4 ends = load_obstacle_edge_endpoints(scene, edge);
                    admit_skyline_arc(
                        geometry,
                        (ends.x - receiver_grid_x) * inverse_scale, ends.y - receiver_grid_y,
                        (ends.z - receiver_grid_x) * inverse_scale, ends.w - receiver_grid_y,
                        need_radius_m, span_lo, span_hi, bin_width, mask);
                }
            }
        }
    }
}

/// A wide bucket's mask and the in-plane angles its azimuth span is pinned to.
struct WideBucketNodes {
    ArcMask mask;
    float span_lo;
    float span_hi;
    float bin_width;
    float angle_at_span_lo;
    float angle_at_span_hi;
    float along_min_m;
    float along_max_m;
};

/// True when the bucket `[angle_lo, angle_hi]` spans at least WIDE_BUCKET_MIN_AZIMUTH_SPAN_RAD of
/// azimuth and something stands in front of it; `wide` then holds its mask.
__device__ bool wide_bucket_nodes(
    const DeviceScenePointers& scene,
    const DeviceLineSource& source,
    const LinePieceGeometry& geometry,
    float receiver_x_m,
    float receiver_y_m,
    float angle_lo,
    float angle_hi,
    WideBucketNodes& wide
) {
    const float along_lo = line_along_at_angle(geometry, angle_lo);
    const float along_hi = line_along_at_angle(geometry, angle_hi);
    const float azimuth_a = line_azimuth_at(geometry, along_lo);
    const float turn = wrap_to_pi(line_azimuth_at(geometry, along_hi) - azimuth_a);
    const float span = fabsf(turn);
    if (span < QUIETMAP_WIDE_BUCKET_MIN_AZIMUTH_SPAN_RAD) {
        return false;
    }
    wide.span_lo = turn < 0.0f ? azimuth_a + turn : azimuth_a;
    wide.span_hi = turn < 0.0f ? azimuth_a : azimuth_a + turn;
    wide.angle_at_span_lo = turn < 0.0f ? angle_hi : angle_lo;
    wide.angle_at_span_hi = turn < 0.0f ? angle_lo : angle_hi;
    wide.along_min_m = fminf(along_lo, along_hi);
    wide.along_max_m = fmaxf(along_lo, along_hi);
    float point_lo[3];
    float point_hi[3];
    line_point_at(geometry, along_lo, point_lo);
    line_point_at(geometry, along_hi, point_hi);
    const float chord = hypotf(point_lo[0] - point_hi[0], point_lo[1] - point_hi[1]);
    const float centre_range = line_horizontal_range_at(
        geometry, line_along_at_angle(geometry, 0.5f * (angle_lo + angle_hi)));
    const float need_radius = fminf(fminf(hypotf(point_lo[0], point_lo[1]),
                                          hypotf(point_hi[0], point_hi[1])), centre_range) + chord;
    wide.bin_width = span / QUIETMAP_WIDE_BUCKET_MASK_BINS;
    gather_blocked_mask(scene, geometry, receiver_x_m, receiver_y_m, need_radius,
                        fmaxf(source.source_height_m, 0.0f), wide.span_lo, wide.span_hi,
                        wide.bin_width, wide.mask);
    bool blocked = false;
    for (int word = 0; word < QUIETMAP_WIDE_BUCKET_MASK_WORDS; ++word) {
        blocked |= wide.mask.bits[word] != 0u;
    }
    return blocked;
}

/// The run of equal mask bins starting at `bin`: its end (exclusive) and its state.
__device__ __forceinline__ int wide_bucket_run_end(const WideBucketNodes& wide, int bin, bool& blocked) {
    blocked = arc_mask_bin(wide.mask, bin);
    int end = bin;
    while (end < QUIETMAP_WIDE_BUCKET_MASK_BINS && arc_mask_bin(wide.mask, end) == blocked) {
        ++end;
    }
    return end;
}

/// In-plane angle at a mask azimuth, clamped to the bucket's stretch.
__device__ __forceinline__ float wide_bucket_angle_at_azimuth(
    const LinePieceGeometry& geometry,
    const WideBucketNodes& wide,
    float azimuth,
    float fallback
) {
    float along_m;
    if (!line_along_at_azimuth(geometry, azimuth, along_m)) {
        return fallback;
    }
    return line_in_plane_angle_at(geometry, quietmap_clamp(along_m, wide.along_min_m, wide.along_max_m));
}

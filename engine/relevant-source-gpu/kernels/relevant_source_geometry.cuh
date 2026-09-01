//! Fixed CUDA ABI, raster sampling, and finite-line relevant-source geometry.

#pragma once

#include <cuda_runtime.h>
#include <stdint.h>
#include <math_constants.h>

constexpr int QUIETMAP_PERIOD_COUNT = 3;
constexpr int QUIETMAP_BAND_COUNT = 8;
constexpr int QUIETMAP_TILE_PIXEL_SIDE = 512;

#include "relevant_source_physics_constants.cuh"

constexpr int QUIETMAP_BLOCKS_PER_TILE_SIDE = QUIETMAP_TILE_PIXEL_SIDE / QUIETMAP_BLOCK_PIXEL_SIDE;
constexpr int QUIETMAP_CORNER_COUNT =
    (QUIETMAP_BLOCKS_PER_TILE_SIDE + 1) * (QUIETMAP_BLOCKS_PER_TILE_SIDE + 1);
static_assert(QUIETMAP_TILE_PIXEL_SIDE % QUIETMAP_BLOCK_PIXEL_SIDE == 0, "block tiles the tile");

struct DeviceLineSource {
    float start_x_m;
    float start_y_m;
    float end_x_m;
    float end_y_m;
    float length_m;
    float max_distance_m;
    float source_height_m;
    uint32_t bridge;
    float emission_linear[QUIETMAP_PERIOD_COUNT * QUIETMAP_BAND_COUNT];
};

struct FusedPixel {
    float elevation;
    uint8_t forest;
    uint8_t imd;
    uint8_t padding;
};

struct DeviceRasterGeometry {
    float row_scale_per_metre;
    float row_offset;
    float column_scale_per_metre;
    float column_offset;
    uint32_t rows;
    uint32_t columns;
};

struct DeviceObstacleGrid {
    float query_x_scale;
    float query_x_offset_m;
    float query_y_offset_m;
    float cell_m;
    float minimum_x_m;
    float minimum_y_m;
    uint32_t columns;
    uint32_t rows;
    uint32_t cell_starts_offset;
    uint32_t edge_references_offset;
    uint32_t edge_values_offset;
    uint32_t cell_maximum_height_offset;
};

struct DeviceBarrier {
    float start_x_m;
    float start_y_m;
    float end_x_m;
    float end_y_m;
    float height_m;
    float receiver_distance_lower_bound_m;
};

struct DeviceScenePointers {
    const DeviceLineSource* sources;
    const FusedPixel* raster_pixels;
    const DeviceObstacleGrid* obstacle_grids;
    const uint32_t* obstacle_cell_starts;
    const uint32_t* obstacle_edge_references;
    const float* obstacle_edge_values_xyxyh;
    const float* obstacle_cell_maximum_heights;
    const DeviceBarrier* barriers;
    uint32_t source_count;
    uint32_t obstacle_grid_count;
    uint32_t barrier_count;
    /// Non-zero: long rays take the surface heatmap's coarse-middle cadence.
    uint32_t coarse_middle_cadence;
    DeviceRasterGeometry raster_geometry;
};

struct SampledRasterPoint {
    float elevation_m;
    uint8_t forest;
    uint8_t imd;
};

struct LineReceiverGeometry {
    float closest_x_m;
    float closest_y_m;
    float endpoint_distance_m;
    float perpendicular_distance_m;
    float signed_fraction;
    float slant_distance_m;
    float source_altitude_m;
    float base_level_db;
};

static_assert(sizeof(DeviceLineSource) == 128, "source ABI");
static_assert(sizeof(FusedPixel) == 8, "raster pixel ABI");
static_assert(sizeof(DeviceRasterGeometry) == 24, "raster geometry ABI");
static_assert(sizeof(DeviceObstacleGrid) == 48, "obstacle grid ABI");
static_assert(sizeof(DeviceBarrier) == 24, "barrier ABI");
static_assert(sizeof(DeviceScenePointers) == 104, "scene ABI");

__device__ __forceinline__ float quietmap_clamp(float value, float minimum, float maximum) {
    return fminf(fmaxf(value, minimum), maximum);
}

__device__ __forceinline__ float quietmap_energy_from_db(float decibels) {
    return __expf(decibels * 0.23025850929940458f);
}

__device__ __forceinline__ float quietmap_attenuation_from_energy(float energy) {
    return -4.342944819032518f * __logf(fmaxf(energy, 1.0e-20f));
}

__device__ __forceinline__ SampledRasterPoint sample_scene_raster(
    const DeviceScenePointers& scene,
    float x_m,
    float y_m
) {
    const DeviceRasterGeometry geometry = scene.raster_geometry;
    float row = fmaf(y_m, geometry.row_scale_per_metre, geometry.row_offset);
    float column = fmaf(x_m, geometry.column_scale_per_metre, geometry.column_offset);
    row = quietmap_clamp(row, 0.0f, static_cast<float>(geometry.rows - 1));
    column = quietmap_clamp(column, 0.0f, static_cast<float>(geometry.columns - 1));
    const uint32_t row0 = min(static_cast<uint32_t>(floorf(row)), geometry.rows - 2);
    const uint32_t column0 = min(static_cast<uint32_t>(floorf(column)), geometry.columns - 2);
    const float row_fraction = row - static_cast<float>(row0);
    const float column_fraction = column - static_cast<float>(column0);
    const uint32_t base = row0 * geometry.columns + column0;
    const FusedPixel point00 = scene.raster_pixels[base];
    const FusedPixel point01 = scene.raster_pixels[base + 1];
    const FusedPixel point10 = scene.raster_pixels[base + geometry.columns];
    const FusedPixel point11 = scene.raster_pixels[base + geometry.columns + 1];
    const float elevation0 = fmaf(column_fraction, point01.elevation - point00.elevation,
                                  point00.elevation);
    const float elevation1 = fmaf(column_fraction, point11.elevation - point10.elevation,
                                  point10.elevation);
    const float imd0 = fmaf(column_fraction, static_cast<float>(point01.imd) - point00.imd,
                            static_cast<float>(point00.imd));
    const float imd1 = fmaf(column_fraction, static_cast<float>(point11.imd) - point10.imd,
                            static_cast<float>(point10.imd));
    const uint32_t nearest = base + (row_fraction >= 0.5f ? geometry.columns : 0)
                            + (column_fraction >= 0.5f ? 1 : 0);
    SampledRasterPoint result;
    result.elevation_m = fmaf(row_fraction, elevation1 - elevation0, elevation0);
    result.forest = scene.raster_pixels[nearest].forest;
    result.imd = static_cast<uint8_t>(quietmap_clamp(
        roundf(fmaf(row_fraction, imd1 - imd0, imd0)), 0.0f, 255.0f));
    return result;
}

__device__ __forceinline__ float finite_line_correction_db(
    float segment_length_m,
    float perpendicular_distance_m,
    float signed_fraction,
    float divergence_distance_m
) {
    if (segment_length_m < 0.1f) {
        return 0.0f;
    }
    const float perpendicular = fmaxf(perpendicular_distance_m, 0.5f);
    const float first = signed_fraction * segment_length_m / perpendicular;
    const float second = (1.0f - signed_fraction) * segment_length_m / perpendicular;
    const float product = first * second;
    const float angle = product < 0.98f
        ? atanf((first + second) / (1.0f - product))
        : atanf(first) + atanf(second);
    const float finite_correction = fminf(
        4.342944819032518f * __logf(fmaxf(angle / CUDART_PI_F, 1.0e-20f)), 0.0f);
    return finite_correction
        + 4.342944819032518f
            * __logf(fmaxf(divergence_distance_m, perpendicular) / perpendicular);
}

__device__ __forceinline__ bool line_receiver_geometry(
    const DeviceScenePointers& scene,
    const DeviceLineSource& source,
    float receiver_x_m,
    float receiver_y_m,
    float receiver_altitude_m,
    float receiver_reflection_db,
    LineReceiverGeometry& result
) {
    const float segment_x = source.end_x_m - source.start_x_m;
    const float segment_y = source.end_y_m - source.start_y_m;
    const float receiver_from_start_x = receiver_x_m - source.start_x_m;
    const float receiver_from_start_y = receiver_y_m - source.start_y_m;
    const float segment_length_squared = fmaf(segment_x, segment_x, segment_y * segment_y);
    const float signed_fraction = segment_length_squared > 1.0e-10f
        ? (receiver_from_start_x * segment_x + receiver_from_start_y * segment_y)
            / segment_length_squared
        : 0.0f;
    const float clamped_fraction = quietmap_clamp(signed_fraction, 0.0f, 1.0f);
    const float closest_x = fmaf(clamped_fraction, segment_x, source.start_x_m);
    const float closest_y = fmaf(clamped_fraction, segment_y, source.start_y_m);
    const float endpoint_dx = receiver_x_m - closest_x;
    const float endpoint_dy = receiver_y_m - closest_y;
    const float endpoint_distance = hypotf(endpoint_dx, endpoint_dy);
    if (endpoint_distance > source.max_distance_m) {
        return false;
    }
    const float perpendicular_x = receiver_from_start_x - signed_fraction * segment_x;
    const float perpendicular_y = receiver_from_start_y - signed_fraction * segment_y;
    const float perpendicular_distance = hypotf(perpendicular_x, perpendicular_y);
    const float source_altitude = sample_scene_raster(scene, closest_x, closest_y).elevation_m
                                  + source.source_height_m;
    const float slant_distance = fmaxf(
        hypotf(endpoint_distance, source_altitude - receiver_altitude_m), 1.0f);
    const float finite_correction = finite_line_correction_db(
        source.length_m, perpendicular_distance, signed_fraction, endpoint_distance);
    result.closest_x_m = closest_x;
    result.closest_y_m = closest_y;
    result.endpoint_distance_m = endpoint_distance;
    result.perpendicular_distance_m = perpendicular_distance;
    result.signed_fraction = signed_fraction;
    result.slant_distance_m = slant_distance;
    result.source_altitude_m = source_altitude;
    result.base_level_db = receiver_reflection_db + finite_correction
                           - 4.342944819032518f * __logf(2.0f * CUDART_PI_F * slant_distance);
    return true;
}

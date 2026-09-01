//! Bilateral raster profile, raw-ground fit, and vegetation depth on one CUDA ray.

#pragma once

#include "relevant_source_geometry.cuh"

constexpr float QUIETMAP_EXACT_CADENCE_MAX_DISTANCE_M = 400.0f;
constexpr float QUIETMAP_FULL_RESOLUTION_END_ZONE_M = 600.0f;
constexpr int QUIETMAP_MIDDLE_CADENCE_STRIDE = 3;
constexpr int QUIETMAP_MAXIMUM_PROFILE_POINTS = 96;

struct PathProfile {
    int count;
    float distance_m;
    float t[QUIETMAP_MAXIMUM_PROFILE_POINTS];
    float elevation_m[QUIETMAP_MAXIMUM_PROFILE_POINTS];
    float ground_path_g;
    float source_ground_g;
    float forest_depth_m;
    float mean_ground_slope;
    float mean_ground_intercept_m;
};

struct PlaneFitSums {
    float count;
    float sum_x;
    float sum_z;
    float sum_x_squared;
    float sum_xz;
    float reference_z;
};

__device__ __forceinline__ void reset_plane_fit(PlaneFitSums& sums, float reference_z) {
    sums.count = 0.0f;
    sums.sum_x = 0.0f;
    sums.sum_z = 0.0f;
    sums.sum_x_squared = 0.0f;
    sums.sum_xz = 0.0f;
    sums.reference_z = reference_z;
}

__device__ __forceinline__ void add_plane_fit_point(
    PlaneFitSums& sums,
    float x_m,
    float elevation_m
) {
    const float relative_z = elevation_m - sums.reference_z;
    sums.count += 1.0f;
    sums.sum_x += x_m;
    sums.sum_z += relative_z;
    sums.sum_x_squared = fmaf(x_m, x_m, sums.sum_x_squared);
    sums.sum_xz = fmaf(x_m, relative_z, sums.sum_xz);
}

__device__ __forceinline__ void finish_plane_fit(
    const PlaneFitSums& sums,
    float& slope,
    float& intercept
) {
    if (sums.count < 1.0f) {
        slope = 0.0f;
        intercept = 0.0f;
        return;
    }
    const float denominator = sums.count * sums.sum_x_squared - sums.sum_x * sums.sum_x;
    if (fabsf(denominator) < 1.0e-6f) {
        slope = 0.0f;
        intercept = sums.reference_z + sums.sum_z / sums.count;
        return;
    }
    slope = (sums.count * sums.sum_xz - sums.sum_x * sums.sum_z) / denominator;
    intercept = sums.reference_z + (sums.sum_z - slope * sums.sum_x) / sums.count;
}

__device__ __forceinline__ void append_profile_t(PathProfile& profile, float value) {
    if (profile.count > 0 && fabsf(profile.t[profile.count - 1] - value) < 1.0e-8f) {
        return;
    }
    if (profile.count < QUIETMAP_MAXIMUM_PROFILE_POINTS) {
        profile.t[profile.count++] = value;
    }
}

__device__ __forceinline__ void fill_profile_chainages(PathProfile& profile, float distance_m) {
    profile.count = 0;
    profile.distance_m = distance_m;
    const bool emit_near = distance_m >= 3.0f * QUIETMAP_NEAR_SAMPLE_M;
    const float near_t = QUIETMAP_NEAR_SAMPLE_M / distance_m;
    if (distance_m <= QUIETMAP_RASTER_CELL_M * 10.0f) {
        const int intervals = max(static_cast<int>(ceilf(distance_m / QUIETMAP_RASTER_CELL_M)), 3);
        append_profile_t(profile, 0.0f);
        if (emit_near) {
            append_profile_t(profile, near_t);
        }
        for (int index = 1; index < intervals - 1; ++index) {
            const float t = static_cast<float>(index) / static_cast<float>(intervals - 1);
            if (emit_near
                && (fabsf(t - near_t) * distance_m < 3.0f
                    || fabsf((1.0f - t) - near_t) * distance_m < 3.0f)) {
                continue;
            }
            append_profile_t(profile, t);
        }
        if (emit_near) {
            append_profile_t(profile, 1.0f - near_t);
        }
        append_profile_t(profile, 1.0f);
        return;
    }

    append_profile_t(profile, 0.0f);
    if (emit_near) {
        append_profile_t(profile, near_t);
    }
    const float levels[4] = {
        QUIETMAP_RASTER_CELL_M,
        QUIETMAP_RASTER_CELL_M * 2.0f,
        QUIETMAP_RASTER_CELL_M * 4.0f,
        QUIETMAP_RASTER_CELL_M * 8.0f,
    };
    const bool use_coarse_middle = distance_m > QUIETMAP_EXACT_CADENCE_MAX_DISTANCE_M;
    float position_m = emit_near ? QUIETMAP_NEAR_SAMPLE_M : 0.0f;
    float last_forward_position_m = position_m;
    bool forward_done = false;
    for (int level = 0; level < 4 && !forward_done; ++level) {
        for (int repetition = 0; repetition < 3; ++repetition) {
            position_m += levels[level];
            if (position_m >= distance_m * 0.5f
                || (use_coarse_middle
                    && position_m > QUIETMAP_FULL_RESOLUTION_END_ZONE_M)) {
                forward_done = true;
                break;
            }
            append_profile_t(profile, position_m / distance_m);
            last_forward_position_m = position_m;
        }
    }
    const float forward_end = (use_coarse_middle
        ? last_forward_position_m : fminf(position_m, distance_m * 0.5f)) / distance_m;

    float backward_position_m = emit_near ? QUIETMAP_NEAR_SAMPLE_M : 0.0f;
    bool backward_done = false;
    for (int level = 0; level < 4 && !backward_done; ++level) {
        for (int repetition = 0; repetition < 3; ++repetition) {
            const float next = backward_position_m + levels[level];
            if (next >= distance_m * 0.5f
                || (use_coarse_middle
                    && next > QUIETMAP_FULL_RESOLUTION_END_ZONE_M)) {
                backward_done = true;
                break;
            }
            backward_position_m = next;
        }
    }
    const float backward_start = fmaxf(1.0f - backward_position_m / distance_m, 0.5f);
    const float coarse_step_m = fminf(
        levels[3] * (use_coarse_middle ? QUIETMAP_MIDDLE_CADENCE_STRIDE : 1),
        distance_m * 0.25f);
    float middle = forward_end;
    while (middle < backward_start - 0.0001f) {
        middle += coarse_step_m / distance_m;
        if (middle < backward_start - 1.0e-8f) {
            append_profile_t(profile, middle);
        }
    }

    float backward_values[12];
    int backward_count = 0;
    position_m = emit_near ? QUIETMAP_NEAR_SAMPLE_M : 0.0f;
    backward_done = false;
    for (int level = 0; level < 4 && !backward_done; ++level) {
        for (int repetition = 0; repetition < 3; ++repetition) {
            position_m += levels[level];
            if (position_m >= distance_m * 0.5f
                || (use_coarse_middle
                    && position_m > QUIETMAP_FULL_RESOLUTION_END_ZONE_M)) {
                backward_done = true;
                break;
            }
            backward_values[backward_count++] = 1.0f - position_m / distance_m;
        }
    }
    for (int index = backward_count - 1; index >= 0; --index) {
        append_profile_t(profile, backward_values[index]);
    }
    if (emit_near) {
        append_profile_t(profile, 1.0f - near_t);
    }
    append_profile_t(profile, 1.0f);
}

__device__ __forceinline__ void build_path_profile(
    const DeviceScenePointers& scene,
    float source_x_m,
    float source_y_m,
    float receiver_x_m,
    float receiver_y_m,
    float distance_m,
    bool force_hard_ground,
    PathProfile& profile
) {
    fill_profile_chainages(profile, distance_m);
    PlaneFitSums ground_fit;
    float imd_integral = 0.0f;
    float forest_total = 0.0f;
    float forest_run_physical = 0.0f;
    float forest_run_weighted = 0.0f;
    uint8_t previous_imd = 0;
    for (int index = 0; index < profile.count; ++index) {
        const float t = profile.t[index];
        const SampledRasterPoint sample = sample_scene_raster(
            scene,
            fmaf(t, receiver_x_m - source_x_m, source_x_m),
            fmaf(t, receiver_y_m - source_y_m, source_y_m));
        profile.elevation_m[index] = sample.elevation_m;
        if (index == 0) {
            reset_plane_fit(ground_fit, sample.elevation_m);
            previous_imd = sample.imd;
        }
        add_plane_fit_point(ground_fit, t * distance_m, sample.elevation_m);
        if (index > 0) {
            const float interval_m = (t - profile.t[index - 1]) * distance_m;
            imd_integral += 0.5f * (static_cast<float>(previous_imd) + sample.imd) * interval_m;
            if (sample.forest > 0) {
                forest_run_physical += interval_m;
                forest_run_weighted += interval_m * static_cast<float>(sample.forest) * 0.01f;
            } else {
                if (forest_run_physical >= QUIETMAP_MINIMUM_FOREST_RUN_M) {
                    forest_total += forest_run_weighted;
                }
                forest_run_physical = 0.0f;
                forest_run_weighted = 0.0f;
            }
            previous_imd = sample.imd;
        }
    }
    if (forest_run_physical >= QUIETMAP_MINIMUM_FOREST_RUN_M) {
        forest_total += forest_run_weighted;
    }
    finish_plane_fit(ground_fit, profile.mean_ground_slope, profile.mean_ground_intercept_m);
    const float mean_imd = distance_m > 1.0e-6f ? imd_integral / distance_m : previous_imd;
    profile.ground_path_g = force_hard_ground ? 0.0f
        : quietmap_clamp(1.0f - mean_imd * 0.01f, 0.0f, 1.0f);
    profile.source_ground_g = force_hard_ground ? 0.0f
        : quietmap_clamp(1.0f - static_cast<float>(
            sample_scene_raster(scene, source_x_m, source_y_m).imd) * 0.01f, 0.0f, 1.0f);
    profile.forest_depth_m = forest_total;
}

__device__ __forceinline__ float profile_elevation_at(const PathProfile& profile, float t) {
    int upper = 1;
    while (upper < profile.count - 1 && profile.t[upper] < t) {
        ++upper;
    }
    const float t0 = profile.t[upper - 1];
    const float t1 = profile.t[upper];
    const float fraction = t1 > t0 ? quietmap_clamp((t - t0) / (t1 - t0), 0.0f, 1.0f) : 0.0f;
    return fmaf(fraction, profile.elevation_m[upper] - profile.elevation_m[upper - 1],
                profile.elevation_m[upper - 1]);
}

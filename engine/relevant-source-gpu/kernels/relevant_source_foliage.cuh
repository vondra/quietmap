//! Foliage attenuation from the ray's metres in canopy (vegetation.rs).
//!
//! Each profile interval contributes its slant length times the fraction of its ends inside
//! the canopy volume (above the bare-earth ground, at or below ground plus canopy height)
//! times the mean forest cover; the attenuation is ISO 9613-2:2024 Table A.1 literally.
//! The samples re-read the scene at the profile chainages (bilinear ground, nearest canopy
//! and cover — the same rule as the CPU profile). Callers guarantee canopy ≤ 250 m (the
//! scene build fails on a 255 sample before any kernel runs).

#pragma once

#include "relevant_source_cnossos_boundary.cuh"
#include "relevant_source_path.cuh"

/// Cover-weighted ray metres inside the canopy volume per state (homogeneous, favourable).
__device__ inline void canopy_foliage_depths(
    const DeviceScenePointers& scene, float source_x_m, float source_y_m, float receiver_x_m,
    float receiver_y_m, float source_altitude_m, float receiver_altitude_m, float length_m,
    float gamma_m, const PathProfile& profile, float* depth_h_m, float* depth_f_m
) {
    const PlanePoint source{0.0f, source_altitude_m};
    const PlanePoint receiver{length_m, receiver_altitude_m};
    float homogeneous = 0.0f;
    float favourable = 0.0f;
    for (int index = 1; index < profile.count; ++index) {
        const float t0 = profile.t[index - 1];
        const float t1 = profile.t[index];
        const float x0 = t0 * length_m;
        const float x1 = t1 * length_m;
        const SampledRasterPoint s0 = sample_scene_raster(
            scene, fmaf(t0, receiver_x_m - source_x_m, source_x_m),
            fmaf(t0, receiver_y_m - source_y_m, source_y_m));
        const SampledRasterPoint s1 = sample_scene_raster(
            scene, fmaf(t1, receiver_x_m - source_x_m, source_x_m),
            fmaf(t1, receiver_y_m - source_y_m, source_y_m));
        const float chord0 = source_altitude_m + (receiver_altitude_m - source_altitude_m) * t0;
        const float chord1 = source_altitude_m + (receiver_altitude_m - source_altitude_m) * t1;
        const float r0_h = chord0;
        const float r1_h = chord1;
        const float r0_f = chord0
            + ray_height_above_chord(
                length_m, source, receiver, gamma_m, QUIETMAP_STATE_FAVOURABLE, x0);
        const float r1_f = chord1
            + ray_height_above_chord(
                length_m, source, receiver, gamma_m, QUIETMAP_STATE_FAVOURABLE, x1);
        const float top0 = s0.elevation_m + s0.canopy_m;
        const float top1 = s1.elevation_m + s1.canopy_m;
        const float in0_h =
            (s0.elevation_m < r0_h && r0_h <= top0) ? 1.0f : 0.0f;
        const float in1_h =
            (s1.elevation_m < r1_h && r1_h <= top1) ? 1.0f : 0.0f;
        const float in0_f =
            (s0.elevation_m < r0_f && r0_f <= top0) ? 1.0f : 0.0f;
        const float in1_f =
            (s1.elevation_m < r1_f && r1_f <= top1) ? 1.0f : 0.0f;
        const float cover = (fminf(s0.forest, 100.0f) + fminf(s1.forest, 100.0f)) * 0.005f;
        homogeneous += 0.5f * (in0_h + in1_h) * cover * hypotf(x1 - x0, r1_h - r0_h);
        favourable += 0.5f * (in0_f + in1_f) * cover * hypotf(x1 - x0, r1_f - r0_f);
    }
    *depth_h_m = homogeneous;
    *depth_f_m = favourable;
}

/// ISO Table A.1 for one band: nothing below 10 m, the short row below 20 m, the rate
/// times the depth (capped at 200 m) above.
__device__ __forceinline__ float foliage_band_db(float depth_m, int band) {
    if (depth_m < QUIETMAP_FOLIAGE_MIN_DEPTH_M) {
        return 0.0f;
    }
    if (depth_m < QUIETMAP_FOLIAGE_RATE_DEPTH_M) {
        return QUIETMAP_FOLIAGE_SHORT_DB[band];
    }
    return QUIETMAP_FOLIAGE_DB_PER_M[band] * fminf(depth_m, QUIETMAP_FOLIAGE_MAX_DEPTH_M);
}

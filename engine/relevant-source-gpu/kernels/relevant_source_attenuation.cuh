//! Airport ground operations' single dominant edge (terrain, then the exact-crossing race) and
//! band-mean ground over a sampled CUDA path (the W5 carve-out; surface sources use
//! relevant_source_cnossos_*).

#pragma once

#include "relevant_source_path.cuh"

struct DiffractionEdge {
    bool present;
    bool explicit_obstacle;
    float t;
    float top_m;
    float delta_m;
    float delta_favourable_m;
    float delta_star_m;
    int terrain_sample_index;
};

__device__ __forceinline__ void clamp_source_platform_profile(PathProfile& profile) {
    const float source_ground_m = profile.elevation_m[0];
    for (int index = 1; index < profile.count; ++index) {
        if (profile.t[index] * profile.distance_m >= QUIETMAP_RASTER_CELL_M) {
            break;
        }
        profile.elevation_m[index] = fminf(profile.elevation_m[index], source_ground_m);
    }
}

__device__ __forceinline__ float circular_arc_length(float chord_m, float radius_m) {
    return 2.0f * radius_m * asinf(chord_m / (2.0f * radius_m));
}

__device__ __forceinline__ void fit_profile_range(
    const PathProfile& profile,
    int begin,
    int end,
    float t_offset,
    bool include_extra,
    float extra_t,
    float extra_elevation,
    float& slope,
    float& intercept
) {
    const float reference = include_extra ? extra_elevation : profile.elevation_m[begin];
    PlaneFitSums sums;
    reset_plane_fit(sums, reference);
    for (int index = begin; index < end; ++index) {
        add_plane_fit_point(
            sums, (profile.t[index] - t_offset) * profile.distance_m,
            profile.elevation_m[index]);
    }
    if (include_extra) {
        add_plane_fit_point(
            sums, (extra_t - t_offset) * profile.distance_m, extra_elevation);
    }
    finish_plane_fit(sums, slope, intercept);
}

__device__ __forceinline__ float edge_delta_star(
    const PathProfile& profile,
    const DiffractionEdge& edge,
    float source_height_m,
    float receiver_height_m
) {
    float source_slope;
    float source_intercept;
    float receiver_slope;
    float receiver_intercept;
    const float bare_top = edge.explicit_obstacle
        ? profile_elevation_at(profile, edge.t, 1)
        : profile.elevation_m[edge.terrain_sample_index];
    if (edge.explicit_obstacle) {
        int lower_end = 0;
        while (lower_end < profile.count && profile.t[lower_end] < edge.t) {
            ++lower_end;
        }
        int upper_begin = lower_end;
        while (upper_begin < profile.count && profile.t[upper_begin] <= edge.t) {
            ++upper_begin;
        }
        fit_profile_range(profile, 0, lower_end, 0.0f, true, edge.t, bare_top,
                          source_slope, source_intercept);
        fit_profile_range(profile, upper_begin, profile.count, edge.t, true, edge.t, bare_top,
                          receiver_slope, receiver_intercept);
    } else {
        fit_profile_range(profile, 0, edge.terrain_sample_index + 1, 0.0f, false, 0.0f, 0.0f,
                          source_slope, source_intercept);
        fit_profile_range(profile, edge.terrain_sample_index, profile.count, edge.t, false,
                          0.0f, 0.0f, receiver_slope, receiver_intercept);
    }
    const float source_to_edge_m = edge.t * profile.distance_m;
    const float edge_to_receiver_m = (1.0f - edge.t) * profile.distance_m;
    const float receiver_plane_at_end = fmaf(receiver_slope, edge_to_receiver_m,
                                             receiver_intercept);
    const float mirror_source_z = 2.0f * source_intercept
                                  - (profile.elevation_m[0] + source_height_m);
    const float mirror_receiver_z = 2.0f * receiver_plane_at_end
                                    - (profile.elevation_m[profile.count - 1]
                                       + receiver_height_m);
    const float source_detour = hypotf(source_to_edge_m, bare_top - mirror_source_z);
    const float receiver_detour = hypotf(edge_to_receiver_m, mirror_receiver_z - bare_top);
    const float direct = hypotf(profile.distance_m, mirror_receiver_z - mirror_source_z);
    return fmaxf(source_detour + receiver_detour - direct, 0.0f);
}

__device__ __forceinline__ DiffractionEdge terrain_diffraction_edge(
    PathProfile& profile,
    float source_altitude_m,
    float receiver_altitude_m,
    float raw_receiver_ground_m
) {
    DiffractionEdge best = {};
    if (profile.count < 3 || profile.distance_m < QUIETMAP_SCREENING_MIN_PATH_M) {
        return best;
    }
    const float source_ground = profile.elevation_m[0];
    const float source_height = fmaxf(
        source_altitude_m - source_ground, QUIETMAP_SOURCE_HEIGHT_FLOOR_M);
    const float receiver_height = fmaxf(
        receiver_altitude_m - raw_receiver_ground_m, QUIETMAP_RECEIVER_HEIGHT_FLOOR_M);
    const float source_elevation = source_ground + source_height;
    const float receiver_elevation = profile.elevation_m[profile.count - 1] + receiver_height;
    const float direct_distance = hypotf(
        profile.distance_m, receiver_elevation - source_elevation);
    for (int index = 1; index < profile.count - 1; ++index) {
        const float t = profile.t[index];
        const float top = profile.elevation_m[index];
        const float sight_line = fmaf(t, receiver_elevation - source_elevation, source_elevation);
        if (top <= sight_line) {
            continue;
        }
        const float first = hypotf(t * profile.distance_m, top - source_elevation);
        const float second = hypotf((1.0f - t) * profile.distance_m, top - receiver_elevation);
        const float delta = first + second - direct_distance;
        if (!best.present || delta > best.delta_m) {
            best.present = true;
            best.explicit_obstacle = false;
            best.t = t;
            best.top_m = top;
            best.delta_m = delta;
            best.terrain_sample_index = index;
        }
    }
    if (best.present) {
        const float first = hypotf(best.t * profile.distance_m,
                                   best.top_m - source_elevation);
        const float second = hypotf((1.0f - best.t) * profile.distance_m,
                                    best.top_m - receiver_elevation);
        const float radius = fmaxf(
            QUIETMAP_FAVOURABLE_CURVATURE_MINIMUM_M,
            QUIETMAP_FAVOURABLE_CURVATURE_PER_DISTANCE * direct_distance);
        best.delta_favourable_m = circular_arc_length(first, radius)
                                   + circular_arc_length(second, radius)
                                   - circular_arc_length(direct_distance, radius);
        best.delta_star_m = edge_delta_star(profile, best, source_height, receiver_height);
    }
    return best;
}

__device__ __forceinline__ void complete_explicit_edge_geometry(
    const PathProfile& profile,
    float source_altitude_m,
    float receiver_altitude_m,
    DiffractionEdge& edge
) {
    const float source_height = fmaxf(
        source_altitude_m - profile.elevation_m[0], QUIETMAP_SOURCE_HEIGHT_FLOOR_M);
    const float receiver_height = fmaxf(
        receiver_altitude_m - profile.elevation_m[profile.count - 1],
        QUIETMAP_RECEIVER_HEIGHT_FLOOR_M);
    const float source_elevation = profile.elevation_m[0] + source_height;
    const float receiver_elevation = profile.elevation_m[profile.count - 1] + receiver_height;
    const float direct_distance = hypotf(profile.distance_m,
                                         receiver_elevation - source_elevation);
    const float source_to_edge = edge.t * profile.distance_m;
    const float edge_to_receiver = (1.0f - edge.t) * profile.distance_m;
    const float first = hypotf(source_to_edge, edge.top_m - source_elevation);
    const float second = hypotf(edge_to_receiver, edge.top_m - receiver_elevation);
    const float sight_line = fmaf(edge.t, receiver_elevation - source_elevation, source_elevation);
    const float sign = edge.top_m >= sight_line ? 1.0f : -1.0f;
    edge.delta_m = sign * (first + second - direct_distance);
    edge.delta_star_m = edge_delta_star(profile, edge, source_height, receiver_height);
    const float radius = fmaxf(
        QUIETMAP_FAVOURABLE_CURVATURE_MINIMUM_M,
        QUIETMAP_FAVOURABLE_CURVATURE_PER_DISTANCE * direct_distance);
    if (sign > 0.0f) {
        edge.delta_favourable_m = circular_arc_length(first, radius)
                                   + circular_arc_length(second, radius)
                                   - circular_arc_length(direct_distance, radius);
    } else {
        const float sight_first = hypotf(source_to_edge, sight_line - source_elevation);
        const float sight_second = hypotf(edge_to_receiver, receiver_elevation - sight_line);
        edge.delta_favourable_m = 2.0f * circular_arc_length(sight_first, radius)
            + 2.0f * circular_arc_length(sight_second, radius)
            - circular_arc_length(first, radius) - circular_arc_length(second, radius)
            - circular_arc_length(direct_distance, radius);
    }
}

__device__ __forceinline__ float maekawa_attenuation_db(
    float delta_m,
    int band,
    bool admitted
) {
    const float wavelength = QUIETMAP_SPEED_OF_SOUND_M_PER_S
        / QUIETMAP_BAND_FREQUENCIES[band];
    if (!admitted) {
        return 0.0f;
    }
    const float shape = delta_m < 0.0f
        ? 3.0f + 40.0f * delta_m / wavelength
        : 3.0f + 20.0f * delta_m * QUIETMAP_BAND_FREQUENCIES[band]
            / QUIETMAP_SPEED_OF_SOUND_M_PER_S;
    return shape > 1.0f
        ? fminf(4.342944819032518f * __logf(shape), QUIETMAP_SINGLE_DIFFRACTION_CAP_DB)
        : 0.0f;
}

__device__ __forceinline__ void diffraction_attenuation_bands(
    const DiffractionEdge& edge,
    float attenuation_db[QUIETMAP_BAND_COUNT]
) {
    if (!edge.present) {
        for (int band = 0; band < QUIETMAP_BAND_COUNT; ++band) {
            attenuation_db[band] = 0.0f;
        }
        return;
    }
    for (int band = 0; band < QUIETMAP_BAND_COUNT; ++band) {
        const float wavelength = QUIETMAP_SPEED_OF_SOUND_M_PER_S
            / QUIETMAP_BAND_FREQUENCIES[band];
        const bool admitted = edge.delta_m >= 0.0f
            || edge.delta_m > wavelength * 0.25f - edge.delta_star_m;
        const float homogeneous = maekawa_attenuation_db(edge.delta_m, band, admitted);
        const float favourable = maekawa_attenuation_db(
            edge.delta_favourable_m, band, admitted);
        attenuation_db[band] = quietmap_attenuation_from_energy(
            (1.0f - QUIETMAP_FAVOURABLE_PROBABILITY)
                * quietmap_energy_from_db(-homogeneous)
            + QUIETMAP_FAVOURABLE_PROBABILITY * quietmap_energy_from_db(-favourable));
    }
}

/// The band-mean ground surrogate airport ground ops keep (iso9613
/// aircraft_ground_atten_db): GROUND_CF * G floored at 0 plus the hard floor's share.
__device__ __forceinline__ void ground_ops_ground_bands(
    float ground_g,
    float attenuation_db[QUIETMAP_BAND_COUNT]
) {
    for (int band = 0; band < QUIETMAP_BAND_COUNT; ++band) {
        attenuation_db[band] = fmaxf(QUIETMAP_GROUND_BAND_MEAN_CF[band] * ground_g, 0.0f)
                               + QUIETMAP_GROUND_HARD_FLOOR_DB * (1.0f - ground_g);
    }
}


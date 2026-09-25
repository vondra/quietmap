//! Full per-pair energy: a line piece's CNOSSOS point sum as quadrature nodes uniform in the
//! in-plane angle, each node on its own CNOSSOS ray, wide buckets on geometry-placed nodes (CPU
//! noise-compute line_quadrature + compute/line_piece); a point source on one ray; airport ground
//! operations on their closest-point single-edge ray (W5 carve-out).

#pragma once

#include "relevant_source_arc.cuh"
#include "relevant_source_cnossos_stream.cuh"

__device__ __forceinline__ float ground_or_barrier_attenuation_db(
    float ground_db,
    float terrain_db,
    float screening_db
) {
    const float barrier = terrain_db + screening_db;
    return barrier > 0.0f ? fmaxf(ground_db, barrier) : ground_db;
}

/// The ray terms of a line source (fixed Gs, its platform) or a point (Gs sampled under it, its
/// footprint radius).
__device__ __forceinline__ RaySourceTerms ray_source_terms(const DeviceLineSource& source) {
    RaySourceTerms terms;
    terms.height_m = source.source_height_m;
    if (source_is_point(source)) {
        terms.ground_factor = -1.0f;
        terms.platform_half_width_m = 0.0f;
        terms.exclusion_radius_m = source.extent_m;
    } else {
        terms.ground_factor = source.source_ground_factor;
        terms.platform_half_width_m = source.platform_half_width_m;
        terms.exclusion_radius_m = 0.0f;
    }
    return terms;
}

/// A node's weight over the in-plane angles between `a` and `b`: Δφ, or the integral of the
/// track dipole 0.01 + 0.99·cos²φ (noise-compute LineDirectivity::weight).
__device__ __forceinline__ float line_node_weight(const DeviceLineSource& source, float a, float b) {
    const float lo = fminf(a, b);
    const float hi = fmaxf(a, b);
    if ((source.flags & QUIETMAP_SOURCE_FLAG_TRACK_DIPOLE) == 0u) {
        return hi - lo;
    }
    return 0.01f * (hi - lo) + 0.99f * (0.5f * (hi - lo) + 0.25f * (sinf(2.0f * hi) - sinf(2.0f * lo)));
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

/// Σ Δφ·T over the piece's quadrature nodes (CPU line_quadrature_nodes), per period and band.
__device__ void line_quadrature_transfer(
    const DeviceScenePointers& scene,
    const DeviceLineSource& source,
    const LinePieceGeometry& geometry,
    float receiver_x_m,
    float receiver_y_m,
    float receiver_altitude_m,
    PathProfile& profile,
    float sum[QUIETMAP_PERIOD_COUNT][QUIETMAP_BAND_COUNT]
) {
    const float bucket_angle = line_subtended_angle(geometry) / QUIETMAP_LINE_BUCKET_COUNT;
    const RaySourceTerms terms = ray_source_terms(source);
    float transfer[QUIETMAP_PERIOD_COUNT][QUIETMAP_BAND_COUNT];
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
                    cnossos_ray_transfer(scene, terms, x_m, y_m, receiver_x_m, receiver_y_m,
                                         receiver_altitude_m, run_blocked, profile, transfer);
                    const float weight = line_node_weight(source, edge_lo, edge_hi);
                    for (int period = 0; period < QUIETMAP_PERIOD_COUNT; ++period) {
                        for (int band = 0; band < QUIETMAP_BAND_COUNT; ++band) {
                            sum[period][band] = fmaf(weight, transfer[period][band],
                                                     sum[period][band]);
                        }
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
        cnossos_ray_transfer(scene, terms, x_m, y_m, receiver_x_m, receiver_y_m, receiver_altitude_m,
                             true, profile, transfer);
        const float weight = line_node_weight(source, angle_lo, angle_hi);
        for (int period = 0; period < QUIETMAP_PERIOD_COUNT; ++period) {
            for (int band = 0; band < QUIETMAP_BAND_COUNT; ++band) {
                sum[period][band] = fmaf(weight, transfer[period][band], sum[period][band]);
            }
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
    float transfer[QUIETMAP_PERIOD_COUNT][QUIETMAP_BAND_COUNT];
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
            const float level = quietmap_energy_from_db(
                geometry.base_level_db
                - ground_or_barrier_attenuation_db(ground_db[band], terrain_db[band], screening_db[band])
                - fminf(QUIETMAP_VEGETATION_DB_PER_M[band] * forest_depth_m,
                        QUIETMAP_VEGETATION_CAP_DB[band])
                - QUIETMAP_ATMOSPHERIC_DB_PER_KM[band] * geometry.slant_distance_m * 0.001f);
            for (int period = 0; period < QUIETMAP_PERIOD_COUNT; ++period) {
                transfer[period][band] = level;
            }
        }
        divergence_linear = 1.0f;
    } else if (source_is_point(source)) {
        const float distance_m = hypotf(receiver_x_m - source.start_x_m,
                                        receiver_y_m - source.start_y_m);
        if (distance_m > source.max_distance_m
            || pair_is_inaudible(scene, source, false, distance_m)) {
            return false;
        }
        cnossos_ray_transfer(scene, ray_source_terms(source), source.start_x_m, source.start_y_m,
                             receiver_x_m, receiver_y_m, receiver_altitude_m, true, profile,
                             transfer);
        // CPU compute_point_sources: the divergence distance never falls inside the footprint.
        const float source_altitude_m = profile.elevation_m[0] + source.source_height_m;
        const float slant_m = fmaxf(
            hypotf(fmaxf(distance_m, source.extent_m), source_altitude_m - receiver_altitude_m), 1.0f);
        divergence_linear = quietmap_energy_from_db(
            receiver_reflection_db - (8.685889638065036f * __logf(slant_m) + 11.0f));
    } else {
        float closest_distance_m;
        if (!line_closest_horizontal_distance(source, receiver_x_m, receiver_y_m, closest_distance_m)
            || closest_distance_m > source.max_distance_m
            || pair_is_inaudible(scene, source, true, closest_distance_m)) {
            return false;
        }
        LinePieceGeometry geometry;
        if (!line_piece_geometry(scene, source, receiver_x_m, receiver_y_m, receiver_altitude_m,
                                 geometry)) {
            return false;
        }
        for (int period = 0; period < QUIETMAP_PERIOD_COUNT; ++period) {
            for (int band = 0; band < QUIETMAP_BAND_COUNT; ++band) {
                transfer[period][band] = 0.0f;
            }
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
                transfer[period][band] * QUIETMAP_A_WEIGHTING_LINEAR[band], period_energy);
        }
        output_energy[period] = period_energy * divergence_linear;
    }
    return true;
}

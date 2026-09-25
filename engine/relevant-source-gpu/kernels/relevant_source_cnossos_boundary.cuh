//! CNOSSOS-EU A_boundary of one meteorological state from its diffraction points and the ground
//! moments of their sides: the f32 form of noise-compute propagation/cnossos (ground.rs,
//! mean_plane.rs, rubber_band.rs, diffraction.rs). Path differences are formed without
//! subtracting long lengths, so a grazing edge keeps its millimetres on a 12 km ray.

#pragma once

#include "relevant_source_obstacles.cuh"

constexpr int QUIETMAP_STATE_HOMOGENEOUS = 0;
constexpr int QUIETMAP_STATE_FAVOURABLE = 1;

/// ∫(z − z_ref) dx, ∫(x − x_ref)(z − z_ref) dx and ∫G dx over one range of the roofed ground,
/// relative to the range's own origin (x_ref, z_ref).
struct SideMoments {
    float ground_z;
    float ground_xz;
    float ground_factor;
};

/// A mean ground line z = slope·x + intercept_m (x from the source).
struct GroundPlane {
    float slope;
    float intercept_m;
};

/// mean_plane.rs EquivalentGeometry.
struct EquivalentGeometry {
    GroundPlane plane;
    float source_side_height_m;
    float receiver_side_height_m;
    bool source_side_below_plane;
    bool receiver_side_below_plane;
    float projected_distance_m;
};

/// The continuous least-squares plane of a range [x_ref, x_ref + length] from its moments
/// (fit_mean_plane); a range under a millimetre is the level plane through `fallback_z`.
__device__ __forceinline__ GroundPlane plane_from_moments(
    const SideMoments& moments, float x_ref, float z_ref, float length, float fallback_z
) {
    GroundPlane plane;
    if (length < 1.0e-3f) {
        plane.slope = 0.0f;
        plane.intercept_m = fallback_z;
        return plane;
    }
    plane.slope = 12.0f * (moments.ground_xz - 0.5f * length * moments.ground_z)
                  / (length * length * length);
    const float intercept_relative = moments.ground_z / length - 0.5f * plane.slope * length;
    plane.intercept_m = z_ref + intercept_relative - plane.slope * x_ref;
    return plane;
}

__device__ __forceinline__ float plane_signed_height(const GroundPlane& p, float x, float z) {
    return (z - fmaf(p.slope, x, p.intercept_m)) * rsqrtf(fmaf(p.slope, p.slope, 1.0f));
}

__device__ __forceinline__ void plane_mirror(const GroundPlane& p, float x, float z,
                                             float& mirror_x, float& mirror_z) {
    const float offset = (z - fmaf(p.slope, x, p.intercept_m)) / fmaf(p.slope, p.slope, 1.0f);
    mirror_x = fmaf(2.0f * offset, p.slope, x);
    mirror_z = z - 2.0f * offset;
}

__device__ __forceinline__ EquivalentGeometry equivalent_geometry(
    const GroundPlane& plane, float from_x, float from_z, float to_x, float to_z
) {
    EquivalentGeometry g;
    g.plane = plane;
    const float source_height = plane_signed_height(plane, from_x, from_z);
    const float receiver_height = plane_signed_height(plane, to_x, to_z);
    g.source_side_height_m = fmaxf(source_height, 0.0f);
    g.receiver_side_height_m = fmaxf(receiver_height, 0.0f);
    g.source_side_below_plane = source_height < 0.0f;
    g.receiver_side_below_plane = receiver_height < 0.0f;
    g.projected_distance_m = fabsf((to_x - from_x) + plane.slope * (to_z - from_z))
                             * rsqrtf(fmaf(plane.slope, plane.slope, 1.0f));
    return g;
}

/// (2.5.14) G′path.
__device__ __forceinline__ float ground_factor_near_source(
    const EquivalentGeometry& g, float path_ground_factor, float source_ground_factor
) {
    const float height_sum = fmaxf(g.source_side_height_m + g.receiver_side_height_m,
                                   QUIETMAP_MINIMUM_HEIGHT_SUM_M);
    const float ratio = g.projected_distance_m / (QUIETMAP_SHORT_PATH_HEIGHT_FACTOR * height_sum);
    return ratio <= 1.0f ? fmaf(path_ground_factor - source_ground_factor, ratio,
                                source_ground_factor)
                         : path_ground_factor;
}

/// (2.5.15)–(2.5.18) analytic term.
__device__ __forceinline__ float analytic_ground_db(int band, float dp, float zs, float zr,
                                                    float impedance) {
    const float f = QUIETMAP_BAND_FREQUENCIES[band];
    const float k = 2.0f * CUDART_PI_F * f / QUIETMAP_SPEED_OF_SOUND_M_PER_S;
    const float root_f = sqrtf(f);
    const float gw13 = impedance > 0.0f ? __powf(impedance, 1.3f) : 0.0f;
    const float gw26 = gw13 * gw13;
    const float w = 0.0185f * f * f * root_f * gw26
                    / (f * root_f * gw26 + 1.3e3f * sqrtf(f * root_f) * gw13 + 1.16e6f);
    const float wd = w * dp;
    const float cf = dp * (1.0f + 3.0f * wd * __expf(-sqrtf(wd))) / (1.0f + wd);
    const float root = sqrtf(2.0f * cf / k);
    const float product = (zs * zs - root * zs + cf / k) * (zr * zr - root * zr + cf / k);
    return -4.342944819032518f * __logf(4.0f * k * k / (dp * dp) * product);
}

/// (2.5.20) favourable lower bound on the unmodified heights.
__device__ __forceinline__ float favourable_floor_db(const EquivalentGeometry& g,
                                                     float floor_factor) {
    const float height_sum = fmaxf(g.source_side_height_m + g.receiver_side_height_m,
                                   QUIETMAP_MINIMUM_HEIGHT_SUM_M);
    const float base = -3.0f * (1.0f - floor_factor);
    const float limit = QUIETMAP_SHORT_PATH_HEIGHT_FACTOR * height_sum;
    return g.projected_distance_m <= limit
        ? base : base * (1.0f + 2.0f * (1.0f - limit / g.projected_distance_m));
}

/// Table 2.5.b roles: Ḡpath (and the hard-ground branch), Ḡw inside (2.5.17), Ḡm of the floor.
struct GroundFactors {
    float path;
    float impedance;
    float floor;
};

__device__ __forceinline__ void ground_attenuation_bands(
    const EquivalentGeometry& g, GroundFactors factors, int state,
    float attenuation_db[QUIETMAP_BAND_COUNT]
) {
    const float dp = fmaxf(g.projected_distance_m, 1.0e-6f);
    float zs = g.source_side_height_m;
    float zr = g.receiver_side_height_m;
    float floor_db;
    if (state == QUIETMAP_STATE_HOMOGENEOUS) {
        if (factors.path == 0.0f) {
            for (int band = 0; band < QUIETMAP_BAND_COUNT; ++band) attenuation_db[band] = -3.0f;
            return;
        }
        floor_db = -3.0f * (1.0f - factors.floor);
    } else {
        floor_db = favourable_floor_db(g, factors.floor);
        if (factors.path == 0.0f) {
            for (int band = 0; band < QUIETMAP_BAND_COUNT; ++band) attenuation_db[band] = floor_db;
            return;
        }
        const float height_sum = fmaxf(zs + zr, QUIETMAP_MINIMUM_HEIGHT_SUM_M);
        const float curvature = QUIETMAP_FAVOURABLE_CURVATURE_A0_PER_M * dp * dp * 0.5f;
        const float terrain = QUIETMAP_FAVOURABLE_TERRAIN_HEIGHT_COEFFICIENT * dp / height_sum;
        const float source_share = zs / height_sum;
        const float receiver_share = zr / height_sum;
        zs = zs + curvature * source_share * source_share + terrain;
        zr = zr + curvature * receiver_share * receiver_share + terrain;
    }
    for (int band = 0; band < QUIETMAP_BAND_COUNT; ++band) {
        attenuation_db[band] = fmaxf(analytic_ground_db(band, dp, zs, zr, factors.impedance),
                                     floor_db);
    }
}

/// Table 2.5.b for a (sub-)path that starts at the source: homogeneous G′/G′, favourable G/G′.
__device__ __forceinline__ GroundFactors source_side_factors(int state, float path_ground,
                                                             float near_source) {
    GroundFactors factors;
    factors.path = path_ground;
    factors.impedance = state == QUIETMAP_STATE_HOMOGENEOUS ? near_source : path_ground;
    factors.floor = near_source;
    return factors;
}

/// A point of the vertical plane.
struct PlanePoint {
    float x;
    float z;
};

/// |AB| + |BC| − |AC| without subtracting lengths: 2(a×b)²/((|a||b| + a·b)(|a| + |b| + |a+b|)).
__device__ __forceinline__ float bend_excess(PlanePoint a_point, PlanePoint b_point,
                                             PlanePoint c_point) {
    const float ax = b_point.x - a_point.x, az = b_point.z - a_point.z;
    const float bx = c_point.x - b_point.x, bz = c_point.z - b_point.z;
    const float first = hypotf(ax, az);
    const float second = hypotf(bx, bz);
    const float whole = hypotf(ax + bx, az + bz);
    const float dot = fmaf(ax, bx, az * bz);
    if (dot <= 0.0f) {
        return first + second - whole;
    }
    const float cross = ax * bz - az * bx;
    return 2.0f * cross * cross / ((first * second + dot) * (first + second + whole));
}

/// The favourable arc's excess over its chord ℓ: 2Γ(asin(ℓ/2Γ) − ℓ/2Γ), by series (ℓ/2Γ ≤ 1/16).
__device__ __forceinline__ float arc_excess(float chord, float radius) {
    const float y = chord / (2.0f * radius);
    const float y2 = y * y;
    return 2.0f * radius * y * y2
           * (1.0f / 6.0f + y2 * (3.0f / 40.0f + y2 * (5.0f / 112.0f + y2 * (35.0f / 1152.0f))));
}

__device__ __forceinline__ float state_length(int state, float radius, PlanePoint a, PlanePoint b) {
    const float chord = hypotf(b.x - a.x, b.z - a.z);
    return state == QUIETMAP_STATE_HOMOGENEOUS ? chord : chord + arc_excess(chord, radius);
}

/// rubber_band.rs StateRay::path_difference over points[0..count): Σ state lengths − |from to|
/// with the single-point sign rules (homogeneous −excess below the chord; favourable (2.5.26)
/// above the straight chord, else (2.5.27) with A on the straight chord).
__device__ float state_path_difference(int state, float radius, PlanePoint from,
                                       const PlanePoint* points, int count, PlanePoint to) {
    if (count <= 0) {
        return 0.0f;
    }
    float straight = 0.0f;
    float arcs = 0.0f;
    PlanePoint previous = from;
    for (int index = 0; index < count; ++index) {
        const PlanePoint next = index + 1 < count ? points[index + 1] : to;
        straight += bend_excess(from, points[index], next);
        if (state == QUIETMAP_STATE_FAVOURABLE) {
            arcs += arc_excess(hypotf(points[index].x - previous.x, points[index].z - previous.z),
                               radius);
        }
        previous = points[index];
    }
    if (state == QUIETMAP_STATE_FAVOURABLE) {
        arcs += arc_excess(hypotf(to.x - previous.x, to.z - previous.z), radius)
                - arc_excess(hypotf(to.x - from.x, to.z - from.z), radius);
    }
    const float excess = straight + arcs;
    if (count > 1) {
        return excess;
    }
    const PlanePoint o = points[0];
    const float chord_z = from.z + (to.z - from.z) * (o.x - from.x) / (to.x - from.x);
    if (o.z >= chord_z) {
        return excess;
    }
    if (state == QUIETMAP_STATE_HOMOGENEOUS) {
        return -excess;
    }
    // (2.5.27) = 2·exc(A) − exc(O): A on the straight chord has no straight excess.
    const PlanePoint a = {o.x, chord_z};
    const float arc_via_a = arc_excess(hypotf(a.x - from.x, a.z - from.z), radius)
                            + arc_excess(hypotf(to.x - a.x, to.z - a.z), radius)
                            - arc_excess(hypotf(to.x - from.x, to.z - from.z), radius);
    return 2.0f * arc_via_a - excess;
}

/// (2.5.21) with C_h = 1: 10·lg(3 + 40·C″·δ/λ) while its argument reaches −2, else 0.
__device__ __forceinline__ float diffraction_db(float path_difference_m, float c_second, int band) {
    const float lambda = QUIETMAP_SPEED_OF_SOUND_M_PER_S / QUIETMAP_BAND_FREQUENCIES[band];
    const float x = 40.0f * c_second * path_difference_m / lambda;
    return x >= -2.0f ? 4.342944819032518f * __logf(3.0f + x) : 0.0f;
}

/// (2.5.31)/(2.5.32).
__device__ __forceinline__ float ground_split_db(float ground_db, float mirrored_db,
                                                 float direct_db) {
    return -8.685889638065036f * __logf(
        1.0f + (quietmap_energy_from_db(-0.5f * ground_db) - 1.0f)
                   * quietmap_energy_from_db(-0.5f * (mirrored_db - direct_db)));
}

/// What one state's stream hands the boundary: its diffraction points and the moments of the
/// ground before the first and after the last.
struct StateDiffraction {
    const PlanePoint* points;
    int count;
    bool blocked;
    SideMoments before_first;
    SideMoments after_last;
    /// Fallbacks of a side shorter than a millimetre: the ground factor under the source, and
    /// the terrain and ground factor under the last point.
    float ground_factor_at_source;
    float terrain_z_at_last;
    float ground_factor_at_last;
};

/// diffraction.rs boundary_for_candidates.
__device__ void state_boundary_bands(
    int state, PlanePoint source, PlanePoint receiver, float source_ground_factor,
    const SideMoments& whole, float ground_origin_z, const StateDiffraction& d,
    float attenuation_db[QUIETMAP_BAND_COUNT]
) {
    const float length = receiver.x;
    const GroundPlane whole_plane = plane_from_moments(whole, 0.0f, ground_origin_z, length,
                                                       ground_origin_z);
    const EquivalentGeometry direct = equivalent_geometry(whole_plane, source.x, source.z,
                                                          receiver.x, receiver.z);
    const float path_ground = whole.ground_factor / length;
    const float near_source = ground_factor_near_source(direct, path_ground, source_ground_factor);
    ground_attenuation_bands(direct, source_side_factors(state, path_ground, near_source), state,
                             attenuation_db);
    if (d.count <= 0) {
        return;
    }
    const float radius = fmaxf(QUIETMAP_FAVOURABLE_RAY_RADIUS_MINIMUM_M,
                               QUIETMAP_FAVOURABLE_RAY_RADIUS_PER_DISTANCE
                                   * hypotf(receiver.x - source.x, receiver.z - source.z));
    const PlanePoint first = d.points[0];
    const PlanePoint last = d.points[d.count - 1];
    const float delta = state_path_difference(state, radius, source, d.points, d.count, receiver);
    float span = 0.0f;
    for (int index = 0; index + 1 < d.count; ++index) {
        span += state_length(state, radius, d.points[index], d.points[index + 1]);
    }
    const GroundPlane source_plane = plane_from_moments(d.before_first, 0.0f, ground_origin_z,
                                                        first.x, ground_origin_z);
    const GroundPlane receiver_plane = plane_from_moments(d.after_last, last.x, last.z,
                                                          length - last.x, d.terrain_z_at_last);
    const EquivalentGeometry source_side = equivalent_geometry(source_plane, source.x, source.z,
                                                               first.x, first.z);
    const EquivalentGeometry receiver_side = equivalent_geometry(receiver_plane, last.x, last.z,
                                                                 receiver.x, receiver.z);
    PlanePoint source_image;
    PlanePoint receiver_image;
    plane_mirror(source_plane, source.x, source.z, source_image.x, source_image.z);
    plane_mirror(receiver_plane, receiver.x, receiver.z, receiver_image.x, receiver_image.z);
    const float source_image_delta =
        state_path_difference(state, radius, source_image, d.points, d.count, receiver);
    const float receiver_image_delta =
        state_path_difference(state, radius, source, d.points, d.count, receiver_image);
    const float source_side_ground = first.x >= 1.0e-3f
        ? d.before_first.ground_factor / first.x : d.ground_factor_at_source;
    const float source_side_near =
        ground_factor_near_source(source_side, source_side_ground, source_ground_factor);
    float ground_so[QUIETMAP_BAND_COUNT];
    ground_attenuation_bands(source_side,
                             source_side_factors(state, source_side_ground, source_side_near),
                             state, ground_so);
    const float receiver_side_ground = length - last.x >= 1.0e-3f
        ? d.after_last.ground_factor / (length - last.x) : d.ground_factor_at_last;
    float ground_or[QUIETMAP_BAND_COUNT];
    ground_attenuation_bands(receiver_side,
                             GroundFactors{receiver_side_ground, receiver_side_ground,
                                           receiver_side_ground},
                             state, ground_or);
    const float rayleigh_star = d.blocked ? 0.0f
        : state_path_difference(state, radius, source_image, d.points, d.count, receiver_image);
    for (int band = 0; band < QUIETMAP_BAND_COUNT; ++band) {
        const float lambda = QUIETMAP_SPEED_OF_SOUND_M_PER_S / QUIETMAP_BAND_FREQUENCIES[band];
        const bool admitted = d.blocked
            || (delta > -lambda / 20.0f && delta > lambda * 0.25f - rayleigh_star);
        if (!admitted) {
            continue;
        }
        float c_second = 1.0f;
        if (d.count > 1 && span > QUIETMAP_MULTIPLE_DIFFRACTION_MINIMUM_SPAN_M) {
            const float x = 5.0f * lambda / span;
            c_second = (1.0f + x * x) / (1.0f / 3.0f + x * x);
        }
        const float direct_db = diffraction_db(delta, c_second, band);
        float sr = direct_db;
        float split_so;
        if (source_side.source_side_below_plane) {
            sr = diffraction_db(source_image_delta, c_second, band);
            split_so = ground_so[band];
        } else {
            split_so = ground_split_db(ground_so[band],
                                       diffraction_db(source_image_delta, c_second, band),
                                       direct_db);
        }
        // Match noise-compute's documented ground-split log-domain fallback, source then
        // receiver: a non-finite split takes its whole ground term and its image diffraction.
        if (!isfinite(split_so)) {
            split_so = ground_so[band];
            sr = diffraction_db(source_image_delta, c_second, band);
        }
        float split_or;
        if (receiver_side.receiver_side_below_plane) {
            sr = diffraction_db(receiver_image_delta, c_second, band);
            split_or = ground_or[band];
        } else {
            split_or = ground_split_db(ground_or[band],
                                       diffraction_db(receiver_image_delta, c_second, band), sr);
        }
        if (!isfinite(split_or)) {
            split_or = ground_or[band];
            sr = diffraction_db(receiver_image_delta, c_second, band);
        }
        attenuation_db[band] = quietmap_clamp(sr, 0.0f, QUIETMAP_DIFFRACTION_CAP_DB)
                               + split_so + split_or;
    }
}

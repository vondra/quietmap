// CNOSSOS-EU boundary attenuation of one path in the vertical plane, per meteorological state:
// physics/src/cnossos/{mod,mean_plane,ground,rubber_band,diffraction}.rs, in f32. The path is not
// stored: its mean planes and ground factors come from moments the ray's stream sums (ray.cuh), the
// rubber band is the stream's hull. Path differences and arc lengths are formed without
// cancellation (common.cuh triangle_excess and arc_excess below); everything else follows the Rust
// line by line.

#define HOMOGENEOUS 0
#define FAVOURABLE 1

struct MeanPlane {
    float slope;
    float intercept_m;
};

// The integrals of the path's ground over one range, relative to an origin (x_r, z_r) the range
// starts at: Moments.z = ∫(z - z_r) dx, xz = ∫(x - x_r)(z - z_r) dx, g = ∫ G dx. The path is linear
// between its vertices, so a stretch adds its trapezoids exactly (mean_plane::fit_mean_plane's
// `accumulate`, VerticalProfile::mean_ground_factor's sum).
struct Moments {
    float z;
    float xz;
    float g;
};

// A linear stretch of the path from (a, za, ga) to (b, zb, gb) added to moments taken about
// (x_r, z_r); z_r = 0 adds a stretch given as a rise over the path (a roof over its terrain).
__device__ __forceinline__ void add_stretch(Moments& m, float a, float b, float za, float zb, float ga, float gb, float x_r, float z_r) {
    float width = b - a;
    if (width <= 0.0f) return;
    float wa = za - z_r, wb = zb - z_r;
    float u0 = a - x_r, u1 = b - x_r;
    m.z += 0.5f * (wa + wb) * width;
    m.xz += width * (u0 * (2.0f * wa + wb) + u1 * (wa + 2.0f * wb)) * (1.0f / 6.0f);
    m.g += 0.5f * (ga + gb) * width;
}

// Moments of the range [x1, x1 + length] taken about (x1, z1), re-taken about (x0, z0).
__device__ __forceinline__ Moments rebased(const Moments& m, float x1, float z1, float x0, float z0, float length) {
    float dx = x1 - x0, dz = z1 - z0;
    return Moments{m.z + dz * length, m.xz + dx * m.z + dz * (0.5f * length * length + dx * length), m.g};
}

// mean_plane::fit_mean_plane of the range [start, start + length] from its moments about
// (start, z_r): the normal equations of the continuous fit; a range under a millimetre is a
// level plane through z_r (the Rust's ground at the range start).
__device__ MeanPlane plane_of_moments(const Moments& m, float start, float length, float z_r) {
    MeanPlane plane;
    if (length < 1e-3f) {
        plane.slope = 0.0f;
        plane.intercept_m = z_r;
        return plane;
    }
    float slope = 12.0f * (m.xz - 0.5f * length * m.z) / (length * length * length);
    plane.slope = slope;
    plane.intercept_m = z_r + (m.z / length - 0.5f * slope * length) - slope * start;
    return plane;
}

// VerticalProfile::mean_ground_factor from the range's moments; G at the range start under a
// millimetre.
__device__ __forceinline__ float mean_ground_of_moments(const Moments& m, float length, float g_at_start) {
    return length < 1e-3f ? g_at_start : m.g / length;
}

__device__ __forceinline__ float plane_altitude_at(const MeanPlane& plane, float x) {
    return plane.slope * x + plane.intercept_m;
}
__device__ __forceinline__ float plane_signed_height(const MeanPlane& plane, float x, float z) {
    return (z - plane_altitude_at(plane, x)) * rsqrtf(1.0f + plane.slope * plane.slope);
}
__device__ __forceinline__ float plane_projected_distance(const MeanPlane& plane, PlanePoint from, PlanePoint to) {
    return fabsf((to.x - from.x) + plane.slope * (to.z - from.z)) * rsqrtf(1.0f + plane.slope * plane.slope);
}
__device__ __forceinline__ PlanePoint plane_mirror(const MeanPlane& plane, PlanePoint point) {
    float norm_sq = 1.0f + plane.slope * plane.slope;
    float offset = (point.z - plane_altitude_at(plane, point.x)) / norm_sq;
    PlanePoint image;
    image.x = point.x + 2.0f * offset * plane.slope;
    image.z = point.z - 2.0f * offset;
    return image;
}

struct EquivalentGeometry {
    MeanPlane plane;
    float source_side_height_m;
    float receiver_side_height_m;
    bool source_side_below_plane;
    bool receiver_side_below_plane;
    float projected_distance_m;
};

__device__ EquivalentGeometry equivalent_geometry_over(MeanPlane plane, PlanePoint from, PlanePoint to) {
    float source_height = plane_signed_height(plane, from.x, from.z);
    float receiver_height = plane_signed_height(plane, to.x, to.z);
    EquivalentGeometry g;
    g.plane = plane;
    g.source_side_height_m = fmaxf(source_height, 0.0f);
    g.receiver_side_height_m = fmaxf(receiver_height, 0.0f);
    g.source_side_below_plane = source_height < 0.0f;
    g.receiver_side_below_plane = receiver_height < 0.0f;
    g.projected_distance_m = plane_projected_distance(plane, from, to);
    return g;
}

// ground.rs
#define SHORT_PATH_HEIGHT_FACTOR 30.0f
#define FAVOURABLE_CURVATURE_A0_PER_M 2e-4f
#define FAVOURABLE_TERRAIN_HEIGHT_COEFFICIENT 6e-3f
#define MINIMUM_HEIGHT_SUM_M 1e-3f
#define HARD_GROUND_BELOW 1e-3f

struct GroundFactors {
    float path;
    float impedance;
    float floor;
};

__device__ float ground_factor_near_source(const EquivalentGeometry& geometry, float path_ground_factor, float source_ground_factor) {
    float height_sum = fmaxf(geometry.source_side_height_m + geometry.receiver_side_height_m, MINIMUM_HEIGHT_SUM_M);
    float ratio = geometry.projected_distance_m / (SHORT_PATH_HEIGHT_FACTOR * height_sum);
    if (ratio <= 1.0f) return path_ground_factor * ratio + source_ground_factor * (1.0f - ratio);
    return path_ground_factor;
}

// ground::analytic_ground_db; BAND_F_* and BAND_WAVENUMBER are ground.rs's BAND_CONSTANTS from
// the host (device.rs band_constants).
__device__ float analytic_ground_db(int band, float dp, float zs, float zr, float gw13) {
    float f_2_5 = BAND_F_2_5[band], f_1_5 = BAND_F_1_5[band], f_0_75 = BAND_F_0_75[band];
    float k = BAND_WAVENUMBER[band];
    float gw26 = gw13 * gw13;
    float w = 0.0185f * f_2_5 * gw26 / (f_1_5 * gw26 + 1.3e3f * f_0_75 * gw13 + 1.16e6f);
    float wd = w * dp;
    float cf = dp * (1.0f + 3.0f * wd * expf(-sqrtf(wd))) / (1.0f + wd);
    float root = sqrtf(2.0f * cf / k);
    float product = (zs * zs - root * zs + cf / k) * (zr * zr - root * zr + cf / k);
    return -10.0f * log10f(4.0f * k * k / (dp * dp) * product);
}

__device__ float favourable_floor_db(const EquivalentGeometry& geometry, float floor_factor) {
    float height_sum = fmaxf(geometry.source_side_height_m + geometry.receiver_side_height_m, MINIMUM_HEIGHT_SUM_M);
    float base = -3.0f * (1.0f - floor_factor);
    float limit = SHORT_PATH_HEIGHT_FACTOR * height_sum;
    if (geometry.projected_distance_m <= limit) return base;
    return base * (1.0f + 2.0f * (1.0f - limit / geometry.projected_distance_m));
}

// ground::ground_attenuation_bands up to its band loop: the heights and impedance of one side (or
// the direct path) in one state, ready for any band; a hard path (G under 0.001) is its floor in every band.
struct GroundTerm {
    float dp;
    float zs;
    float zr;
    float gw13;
    float floor;
    bool hard;
};

__device__ GroundTerm ground_term(const EquivalentGeometry& geometry, GroundFactors factors, int state) {
    GroundTerm term;
    term.dp = fmaxf(geometry.projected_distance_m, 1e-6f);
    term.zs = geometry.source_side_height_m;
    term.zr = geometry.receiver_side_height_m;
    term.hard = factors.path < HARD_GROUND_BELOW;
    term.gw13 = term.hard ? 0.0f : powf(fmaxf(factors.impedance, 0.0f), 1.3f);
    if (state == HOMOGENEOUS) {
        term.floor = term.hard ? -3.0f : -3.0f * (1.0f - factors.floor);
        return term;
    }
    term.floor = favourable_floor_db(geometry, factors.floor);
    if (term.hard) return term;
    float height_sum = fmaxf(term.zs + term.zr, MINIMUM_HEIGHT_SUM_M);
    float curvature = FAVOURABLE_CURVATURE_A0_PER_M * term.dp * term.dp * 0.5f;
    float terrain = FAVOURABLE_TERRAIN_HEIGHT_COEFFICIENT * term.dp / height_sum;
    float zs_ratio = term.zs / height_sum, zr_ratio = term.zr / height_sum;
    term.zs += curvature * (zs_ratio * zs_ratio) + terrain;
    term.zr += curvature * (zr_ratio * zr_ratio) + terrain;
    return term;
}

// ground::ground_attenuation_bands, one band.
__device__ __forceinline__ float ground_db(const GroundTerm& term, int band) {
    if (term.hard) return term.floor;
    return fmaxf(analytic_ground_db(band, term.dp, term.zs, term.zr, term.gw13), term.floor);
}

// rubber_band.rs
#define FAVOURABLE_RAY_RADIUS_MINIMUM_M 1000.0f
#define FAVOURABLE_RAY_RADIUS_PER_DISTANCE 8.0f

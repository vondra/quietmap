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

// Moments of a range [x_t, x_t + width] taken about (x_t, z_t) re-taken about (x_p, z_p).
__device__ __forceinline__ Moments moments_about(const Moments& m, float width, float x_t, float z_t, float x_p, float z_p) {
    float dx = x_t - x_p, dz = z_t - z_p;
    Moments r;
    r.z = m.z + dz * width;
    r.xz = m.xz + dz * 0.5f * width * width + dx * m.z + dx * dz * width;
    r.g = m.g;
    return r;
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
// the direct path) in one state, ready for any band; a hard path (G = 0) is its floor in every band.
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
    term.gw13 = powf(fmaxf(factors.impedance, 0.0f), 1.3f);
    term.hard = factors.path == 0.0f;
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

__device__ __forceinline__ float plane_distance(PlanePoint a, PlanePoint b) {
    return hypotf(b.x - a.x, b.z - a.z);
}
__device__ __forceinline__ float chord_altitude(PlanePoint from, PlanePoint to, float x) {
    return from.z + (to.z - from.z) * (x - from.x) / (to.x - from.x);
}

struct StateRay {
    int state;
    float radius_m;
};

__device__ StateRay state_ray_between(int state, PlanePoint source, PlanePoint receiver) {
    StateRay ray;
    ray.state = state;
    ray.radius_m = fmaxf(FAVOURABLE_RAY_RADIUS_MINIMUM_M, FAVOURABLE_RAY_RADIUS_PER_DISTANCE * plane_distance(source, receiver));
    return ray;
}

// 2 Gamma asin(l / 2 Gamma) - l, the arc's excess over its chord: by its series in x = l / 2 Gamma
// up to 1/16 (Gamma >= 8 d holds the direct chord there): x^3/6 + 3x^5/40 + 5x^7/112 + 35x^9/1152;
// by asin beyond (a leg to an image or through A can be longer than d).
__device__ __forceinline__ float arc_excess(const StateRay& ray, float chord) {
    if (ray.state == HOMOGENEOUS) return 0.0f;
    float x = chord / (2.0f * ray.radius_m);
    if (x > 1.0f / 16.0f) return 2.0f * ray.radius_m * asinf(fminf(x, 1.0f)) - chord;
    float x2 = x * x;
    return 2.0f * ray.radius_m * x * x2 * (1.0f / 6.0f + x2 * (3.0f / 40.0f + x2 * (5.0f / 112.0f + x2 * (35.0f / 1152.0f))));
}

// StateRay::length
__device__ __forceinline__ float state_ray_length(const StateRay& ray, PlanePoint from, PlanePoint to) {
    float chord = plane_distance(from, to);
    return chord + arc_excess(ray, chord);
}

// StateRay::path_difference: the polyline's excess over S-R along the state's rays. The chords'
// excess sums the triangle excesses (S, O1, R) + (O1, O2, R) + ... (each free of cancellation),
// the arcs add their own small excesses.
template <typename P>
__device__ float state_ray_path_difference(const StateRay& ray, PlanePoint from, const P& points, int count, PlanePoint to) {
    if (count == 0) return 0.0f;
    float chords = 0.0f, arcs = 0.0f;
    PlanePoint start = from;
    for (int index = 0; index < count; index++) {
        PlanePoint o = points[index];
        chords += triangle_excess(start, o, to);
        arcs += arc_excess(ray, plane_distance(start, o));
        start = o;
    }
    arcs += arc_excess(ray, plane_distance(start, to)) - arc_excess(ray, plane_distance(from, to));
    float excess = chords + arcs;
    if (count > 1) return excess;
    PlanePoint first = points[0];
    float chord = chord_altitude(from, to, first.x);
    if (ray.state == HOMOGENEOUS) return first.z >= chord ? excess : -excess;
    if (first.z >= chord) return excess;
    // (2.5.27): 2 SA + 2 AR - SO - OR - SR with A on the chord under O, that is 2 (SA + AR - SR)
    // - (SO + OR - SR) and the arcs' excesses; SA + AR = SR only while A lies between the ends (an
    // image mirrored in a tilted plane can stand beyond O).
    PlanePoint on_chord;
    on_chord.x = first.x;
    on_chord.z = chord;
    return 2.0f * triangle_excess(from, on_chord, to) - triangle_excess(from, first, to)
        + 2.0f * arc_excess(ray, plane_distance(from, on_chord)) + 2.0f * arc_excess(ray, plane_distance(on_chord, to))
        - arc_excess(ray, plane_distance(from, first)) - arc_excess(ray, plane_distance(first, to)) - arc_excess(ray, plane_distance(from, to));
}

// StateRay::ray_height_above_chord
__device__ float ray_height_above_chord(const StateRay& ray, PlanePoint source, PlanePoint receiver, float x) {
    if (ray.state == HOMOGENEOUS) return 0.0f;
    float horizontal = receiver.x - source.x;
    float chord = plane_distance(source, receiver);
    float along = (x - source.x) * chord / horizontal;
    float gamma = ray.radius_m;
    float offset = along - 0.5f * chord;
    float perpendicular = along * (chord - along)
        / (sqrtf(fmaxf(gamma * gamma - offset * offset, 0.0f)) + sqrtf(gamma * gamma - 0.25f * chord * chord));
    return perpendicular * chord / horizontal;
}

// diffraction.rs
#define DIFFRACTION_CAP_DB 25.0f
#define MULTIPLE_DIFFRACTION_MINIMUM_SPAN_M 0.3f

// diffraction::diffraction_db, one band of wavelength `lambda`.
__device__ __forceinline__ float diffraction_db(float path_difference_m, float c_second, float lambda) {
    float x = 40.0f * c_second * path_difference_m / lambda;
    return x >= -2.0f ? 10.0f * log10f(3.0f + x) : 0.0f;
}

__device__ __forceinline__ float ground_split_db(float ground_db, float mirrored_db, float direct_db) {
    return -20.0f * log10f(1.0f + (amplitude_of_level(-ground_db) - 1.0f) * amplitude_of_level(-(mirrored_db - direct_db)));
}

__device__ __forceinline__ GroundFactors source_side_factors(int state, float path_ground, float near_source) {
    GroundFactors factors;
    factors.path = path_ground;
    factors.impedance = state == HOMOGENEOUS ? near_source : path_ground;
    factors.floor = near_source;
    return factors;
}

// One diffraction point of a state as the ray's stream left it (rubber_band.rs diffraction_path's
// points, with what diffraction.rs needs of the path around it): the point, its height lowered by
// the state's ray above the chord (the hull's ordinate), the terrain under it, and the moments of
// the path before it (about the source's ground) and after it up to the next point of the hull (about
// itself; the hull's last point carries everything to the receiver).
struct HullEntry {
    float x;
    float z;
    float lowered;
    float terrain_z;
    float terrain_g;
    Moments before;
    Moments after;
};

// The state's diffraction points read as plane points by state_ray_path_difference.
struct EntryPoints {
    const HullEntry* entries;
    __device__ __forceinline__ PlanePoint operator[](int i) const {
        PlanePoint p;
        p.x = entries[i].x;
        p.z = entries[i].z;
        return p;
    }
};

// What the path's stream knows when the ray is done (ray.cuh): its whole moments about the
// source's ground and the ground factors the boundary needs.
struct StreamedPath {
    PlanePoint source;
    PlanePoint receiver;
    float source_ground_z;      // the path's altitude at the source
    float source_ground_g;      // G at the source (the path's first vertex)
    float source_ground_factor; // Gs of (2.5.14)
    Moments whole;              // [0, length] about (0, source_ground_z)
};

// diffraction::boundary_for_candidates up to its band loop, for the points `points[0..count]` the
// stream found: the ground of the direct path and of both sides, the path differences.
struct StateTerm {
    GroundTerm direct;
    int count;                 // diffraction points; 0: the direct path's ground alone
    bool blocked;
    float delta;
    float delta_source_image;
    float delta_receiver_image;
    float rayleigh_star;
    float span;
    GroundTerm source_side;
    GroundTerm receiver_side;
    bool source_below;
    bool receiver_below;
};

__device__ void state_term(const StreamedPath& path, const StateRay& ray, const HullEntry* points, int count, bool blocked,
                           StateTerm* term) {
    int state = ray.state;
    PlanePoint source = path.source, receiver = path.receiver;
    float length = receiver.x;
    term->count = count;
    term->blocked = blocked;
    // A blocked path with diffraction points is admitted in every band (state_attenuation_db), so
    // its direct path's ground is never read.
    if (!blocked || count == 0) {
        EquivalentGeometry direct = equivalent_geometry_over(plane_of_moments(path.whole, 0.0f, length, path.source_ground_z), source, receiver);
        float path_ground = mean_ground_of_moments(path.whole, length, path.source_ground_g);
        float near_source = ground_factor_near_source(direct, path_ground, path.source_ground_factor);
        term->direct = ground_term(direct, source_side_factors(state, path_ground, near_source), state);
    }
    if (count == 0) return;
    EntryPoints at = {points};
    const HullEntry& first = points[0];
    const HullEntry& last = points[count - 1];
    term->delta = state_ray_path_difference(ray, source, at, count, receiver);
    term->span = 0.0f;
    for (int index = 0; index + 1 < count; index++) term->span += state_ray_length(ray, at[index], at[index + 1]);
    float receiver_side_length = length - last.x;
    EquivalentGeometry source_side = equivalent_geometry_over(plane_of_moments(first.before, 0.0f, first.x, path.source_ground_z), source, at[0]);
    EquivalentGeometry receiver_side = equivalent_geometry_over(plane_of_moments(last.after, last.x, receiver_side_length, last.terrain_z), at[count - 1], receiver);
    PlanePoint source_image = plane_mirror(source_side.plane, source);
    PlanePoint receiver_image = plane_mirror(receiver_side.plane, receiver);
    term->delta_source_image = state_ray_path_difference(ray, source_image, at, count, receiver);
    term->delta_receiver_image = state_ray_path_difference(ray, source, at, count, receiver_image);
    term->rayleigh_star = blocked ? 0.0f : state_ray_path_difference(ray, source_image, at, count, receiver_image);
    float source_side_ground = mean_ground_of_moments(first.before, first.x, path.source_ground_g);
    float source_side_near = ground_factor_near_source(source_side, source_side_ground, path.source_ground_factor);
    term->source_side = ground_term(source_side, source_side_factors(state, source_side_ground, source_side_near), state);
    float receiver_side_ground = mean_ground_of_moments(last.after, receiver_side_length, last.terrain_g);
    GroundFactors receiver_factors;
    receiver_factors.path = receiver_side_ground;
    receiver_factors.impedance = receiver_side_ground;
    receiver_factors.floor = receiver_side_ground;
    term->receiver_side = ground_term(receiver_side, receiver_factors, state);
    term->source_below = source_side.source_side_below_plane;
    term->receiver_below = receiver_side.receiver_side_below_plane;
}

// diffraction::boundary_for_candidates' band loop: A_boundary of one band.
__device__ float state_attenuation_db(const StateTerm& term, int band) {
    if (term.count == 0) return ground_db(term.direct, band);
    float lambda = SPEED_OF_SOUND_M_PER_S / BAND_FREQUENCY_HZ[band];
    bool admitted = term.blocked || (term.delta > -lambda / 20.0f && term.delta > lambda / 4.0f - term.rayleigh_star);
    if (!admitted) return ground_db(term.direct, band);
    float c_second = 1.0f;
    if (term.count > 1 && term.span > MULTIPLE_DIFFRACTION_MINIMUM_SPAN_M) {
        float x = 5.0f * lambda / term.span;
        c_second = (1.0f + x * x) / (1.0f / 3.0f + x * x);
    }
    float direct_db = diffraction_db(term.delta, c_second, lambda);
    float source_image_db = diffraction_db(term.delta_source_image, c_second, lambda);
    float receiver_image_db = diffraction_db(term.delta_receiver_image, c_second, lambda);
    float ground_so = ground_db(term.source_side, band);
    float ground_or = ground_db(term.receiver_side, band);
    float sr = direct_db;
    float split_so;
    if (term.source_below) {
        sr = source_image_db;
        split_so = ground_so;
    } else {
        split_so = ground_split_db(ground_so, source_image_db, direct_db);
    }
    if (!isfinite(split_so)) {
        split_so = ground_so;
        sr = source_image_db;
    }
    float split_or;
    if (term.receiver_below) {
        sr = receiver_image_db;
        split_or = ground_or;
    } else {
        split_or = ground_split_db(ground_or, receiver_image_db, sr);
    }
    if (!isfinite(split_or)) {
        split_or = ground_or;
        sr = receiver_image_db;
    }
    return clampf(sr, 0.0f, DIFFRACTION_CAP_DB) + split_so + split_or;
}

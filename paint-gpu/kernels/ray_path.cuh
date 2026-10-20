// Streamed roofs, terrain moments and diffraction candidates (ray.rs, rubber_band.rs).

// The rise of `roof` over the terrain on [a, b] within it, its moments about (x_r, 0), a roof hard
// (its ground factor rises by minus the terrain's): stretch by stretch of the kept vertices under it
// from vertex k, which ends the stretch holding a (-1: found here), or from the terrain's moments from
// the source at a and b where the range starts before them.
__device__ Moments roof_rise(const RayStream& s, const Roof& roof, float a, float b, float x_r, const Moments& prefix_a,
                             const Moments& prefix_b, int k) {
    Moments m = {0.0f, 0.0f, 0.0f};
    float slope = roof.x1 > roof.x0 ? (roof.top1 - roof.top0) / (roof.x1 - roof.x0) : 0.0f;
    if (k < 0) k = recent_stretch_holding(s, a);
    else if (k - 1 < s.vertices - RECENT_VERTICES) k = -1;
    if (k < 0) {
        add_stretch(m, a, b, roof.top0 + slope * (a - roof.x0), roof.top0 + slope * (b - roof.x0), 0.0f, 0.0f, x_r, 0.0f);
        Moments terrain = terrain_between(prefix_a, prefix_b, a, b, x_r, 0.0f, s.path.source_ground_z);
        return Moments{m.z - terrain.z, m.xz - terrain.xz, -terrain.g};
    }
    int newest = s.vertices - 1;
    TerrainPoint lo = recent_vertex(s, k - 1), hi = recent_vertex(s, k);
    float za, ga;
    terrain_on(lo, hi, a, &za, &ga);
    while (a < b) {
        float next = k < newest ? fminf(hi.x, b) : b, zb, gb;
        terrain_on(lo, hi, next, &zb, &gb);
        add_stretch(m, a, next, roof.top0 + slope * (a - roof.x0) - za, roof.top0 + slope * (next - roof.x0) - zb, -ga, -gb, x_r, 0.0f);
        a = next;
        za = zb;
        ga = gb;
        if (k < newest) {
            k++;
            lo = hi;
            hi = recent_vertex(s, k);
        }
    }
    return m;
}

// Moments of a range taken about (x_t, 0) re-taken about (x_p, 0).
__device__ __forceinline__ Moments moved_to(const Moments& m, float x_t, float x_p) {
    Moments r = m;
    r.xz += (x_t - x_p) * m.z;
    return r;
}

__device__ __forceinline__ void add_moments(Moments& m, const Moments& a) {
    m.z += a.z;
    m.xz += a.xz;
    m.g += a.g;
}

// The path's moments from the source to `x` about (0, the source's ground): the terrain's to x
// (`prefix_x`) and the roofs' rises before it.
__device__ Moments path_from_source(const RayStream& s, float x, const Moments& prefix_x) {
    Moments m = prefix_x;
    for (int r = 0; r < s.roof_count && s.roofs[r].x0 < x; r++) {
        const Roof& roof = s.roofs[r];
        add_moments(m, roof.x1 <= x ? moved_to(roof.rise, roof.x0, 0.0f) : roof_rise(s, roof, roof.x0, x, 0.0f, roof.prefix0, prefix_x, -1));
    }
    return m;
}

// The path's moments from `x` to the receiver about (x, z_x): the terrain's, summed a stretch at a
// time from the receiver back so a short stretch at the end of a long ray keeps its digits in f32
// (from the moments from the source, `prefix_x` and `prefix_end`, where x lies before the kept
// vertices), and the roofs' rises after x.
__device__ Moments path_to_receiver(const RayStream& s, float x, float z_x, const Moments& prefix_x, const Moments& prefix_end) {
    Moments m = {0.0f, 0.0f, 0.0f};
    int k = recent_stretch_holding(s, x);
    if (k < 0) {
        m = terrain_between(prefix_x, prefix_end, x, s.length, x, z_x, s.path.source_ground_z);
    } else {
        int newest = s.vertices - 1;
        TerrainPoint hi = recent_vertex(s, k), b = recent_vertex(s, newest);
        float za, ga, last_x = b.x;
        terrain_on(recent_vertex(s, k - 1), hi, x, &za, &ga);
        add_stretch(m, x, hi.x, za, hi.z, ga, hi.g, x, z_x);
        // The terrain from hi to the receiver about hi.
        Moments suffix = {0.0f, 0.0f, 0.0f};
        for (int j = newest - 1; j >= k; j--) {
            TerrainPoint a = recent_vertex(s, j);
            suffix = rebased(suffix, b.x, b.z, a.x, a.z, last_x - b.x);
            add_stretch(suffix, a.x, b.x, a.z, b.z, a.g, b.g, a.x, a.z);
            b = a;
        }
        add_moments(m, rebased(suffix, hi.x, hi.z, x, z_x, last_x - hi.x));
    }
    for (int r = s.roof_count - 1; r >= 0 && s.roofs[r].x1 > x; r--) {
        const Roof& roof = s.roofs[r];
        add_moments(m, roof.x0 >= x ? moved_to(roof.rise, roof.x0, x) : roof_rise(s, roof, x, roof.x1, x, prefix_x, roof.prefix1, -1));
    }
    return m;
}

// Whether band point b stands on or under the state's ray from a to (x, z): straight in calm air,
// the arc of the one radius Gamma downwind (rubber_band.rs diffraction_path).
__device__ __forceinline__ bool under_state_ray(const StateRay& ray, const HullEntry& a, float x, float z, const HullEntry& b) {
    PlanePoint from = {a.x, a.z}, to = {x, z};
    return b.z <= chord_altitude(from, to, b.x) + ray_height_above_chord(ray, from, to, b.x);
}

// What every candidate of one ray reads: the path's ends, its length, the states' rays, and the
// chord S-R with the favourable arc's constants over it, in the walk's registers.
struct RayFrame {
    PlanePoint source, receiver;
    float length, inverse_length;
    StateRay rays[2];
    float chord_m;        // |SR|
    float arc_chord_m;    // the favourable arc's excess over |SR|
    float chord_root_m;   // sqrt(Gamma^2 - |SR|^2 / 4) of the favourable arc
};

// ray_height_above_chord on the ray's own chord S-R from the frame's constants.
__device__ __forceinline__ float frame_height_above_chord(const RayFrame& f, int state, float x) {
    if (state == HOMOGENEOUS) return 0.0f;
    float along = x * f.chord_m * f.inverse_length;
    float gamma = f.rays[state].radius_m;
    float offset = along - 0.5f * f.chord_m;
    float perpendicular = along * (f.chord_m - along) / (sqrtf(fmaxf(gamma * gamma - offset * offset, 0.0f)) + f.chord_root_m);
    return perpendicular * f.chord_m * f.inverse_length;
}

// state_ray_path_difference of the one point O = (x, z) from |SO| and |OR|: the triangle's excess
// without cancellation (triangle_excess's) and downwind the arcs' excesses, over O, or for O under
// the chord (2.5.27) with A on the chord under O, where SA + AR = SR leaves the triangle S A R
// nothing.
__device__ float single_path_difference(const StateRay& ray, const RayFrame& f, float x, float z, float chord,
                                        float to_source, float to_receiver) {
    PlanePoint s = f.source, r = f.receiver;
    float ux = x - s.x, uz = z - s.z, vx = r.x - x, vz = r.z - z;
    float dot = ux * vx + uz * vz;
    float excess;
    if (dot <= 0.0f) {
        excess = to_source + to_receiver - f.chord_m;
    } else {
        float cross = ux * vz - uz * vx;
        excess = 2.0f * cross * cross / ((to_source * to_receiver + dot) * (to_source + to_receiver + f.chord_m));
    }
    if (ray.state == HOMOGENEOUS) return z >= chord ? excess : -excess;
    float arcs = arc_excess(ray, to_source) + arc_excess(ray, to_receiver);
    if (z >= chord) return excess + arcs - f.arc_chord_m;
    float along = x * f.chord_m * f.inverse_length;
    return 2.0f * (arc_excess(ray, along) + arc_excess(ray, f.chord_m - along)) - excess - arcs - f.arc_chord_m;
}

// A diffraction candidate, in the CPU's candidate order, into both states (rubber_band.rs
// diffraction_path): one above the state's chord after lowering joins the hull, otherwise it
// competes for the unblocked path's single point. Inlined into the walk, so `bands` stays in its
// registers.
__device__ __forceinline__ u32 stream_candidate(RayStream& s, BandState* bands, const RayFrame& f, const BandCandidate& c) {
    PlanePoint source = f.source, receiver = f.receiver;
    float chord = source.z + (receiver.z - source.z) * c.x * f.inverse_length;
    // |SO| and |OR|, taken once for both states when either is unblocked.
    float to_source = -1.0f, to_receiver = 0.0f;
    HullEntry entry = {c.x, c.z, c.terrain_z, c.terrain_g};
#pragma unroll
    for (int state = 0; state < 2; state++) {
        StateStream& st = s.states[state];
        BandState& band = bands[state];
        const StateRay& ray = f.rays[state];
        float lowered = c.z - frame_height_above_chord(f, state, c.x);
        if (lowered > chord) {
            band.blocked = true;
            while (band.depth >= 2 && under_state_ray(ray, st.hull[band.depth - 2], c.x, c.z, st.hull[band.depth - 1])) band.depth--;
            if (band.depth >= HULL_CAPACITY) return FAILED_CAPACITY;
            st.hull[band.depth] = entry;
            st.hull_prefix[band.depth] = c.prefix;
            band.depth++;
        } else if (!band.blocked) {
            if (to_source < 0.0f) {
                to_source = hypotf(c.x - source.x, c.z - source.z);
                to_receiver = hypotf(receiver.x - c.x, receiver.z - c.z);
            }
            float delta = single_path_difference(ray, f, c.x, c.z, chord, to_source, to_receiver);
            if (!band.has_best || delta > band.best_delta) {
                band.has_best = true;
                band.best_delta = delta;
                st.best = entry;
                st.best_prefix = c.prefix;
            }
        }
    }
    return 0;
}

// A roof from the wall `from` to the wall at x1 (top1), clipped to start where the roofs closed
// before it end (clip_roofs_in_closing_order: roofs close here in the order the ray leaves them),
// kept with its rise over the terrain.
__device__ u32 stream_roof(RayStream& s, const OpenRoof& from, float x1, float top1, const Moments& prefix1, int vertex1) {
    if (x1 <= s.covered_to) return 0;
    Roof roof = {from.x, from.top, x1, top1, {0.0f, 0.0f, 0.0f}, from.prefix, prefix1};
    int vertex0 = from.vertex;
    if (roof.x0 < s.covered_to) {
        roof.top0 += (s.covered_to - roof.x0) / (x1 - roof.x0) * (top1 - roof.top0);
        roof.x0 = s.covered_to;
        roof.prefix0 = s.covered_prefix;
        vertex0 = s.covered_vertex;
    }
    s.covered_to = x1;
    s.covered_prefix = prefix1;
    s.covered_vertex = vertex1;
    if (s.roof_count == ROOFS) return FAILED_CAPACITY;
    roof.rise = roof_rise(s, roof, roof.x0, x1, roof.x0, roof.prefix0, prefix1, vertex0);
    s.roofs[s.roof_count++] = roof;
    return 0;
}

// The roofs closed at one distance: clip_roofs_in_closing_order sorts by the far wall, then the near
// one.
__device__ u32 stream_closing_roofs(RayStream& s) {
    for (int i = 1; i < s.closing_count; i++) {
        OpenRoof from = s.closing_from[i];
        float top = s.closing_top[i];
        int j = i - 1;
        while (j >= 0 && s.closing_from[j].x > from.x) {
            s.closing_from[j + 1] = s.closing_from[j];
            s.closing_top[j + 1] = s.closing_top[j];
            j--;
        }
        s.closing_from[j + 1] = from;
        s.closing_top[j + 1] = top;
    }
    for (int i = 0; i < s.closing_count; i++) {
        u32 failed = stream_roof(s, s.closing_from[i], s.closing_x, s.closing_top[i], s.closing_prefix, s.closing_vertex);
        if (failed) return failed;
    }
    s.closing_count = 0;
    return 0;
}

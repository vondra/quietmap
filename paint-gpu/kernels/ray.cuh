// One source-receiver ray in f32, streamed: popup/src/scene.rs (the ground at every lattice line the
// ray crosses, walked in scene.cuh) and physics/src/ray.rs (the vertical path of terrain, roofs and
// obstacle tops, and its transfer) without storing the ray. Terrain samples and wall crossings arrive in order of distance; each stretch of
// the path is added at once to the moments the mean planes need, every candidate at once to both
// states' bands (rubber_band.rs: a monotone chain of the states' rays), a footprint's crossings pair into a roof
// as they arrive. The CPU keeps the same path in arrays and sorts; the answer is the same.

// Per-thread capacities; a ray needing more fails with FAILED_CAPACITY and goes to the CPU.
#define TERRAIN_HISTORY 32   // vertices kept behind the stream, for a roof that closes later
#define HULL_CAPACITY 40     // points of one state's hull (dev4's oracle rays needed at most 21)
#define OPEN_ROOFS 12        // footprints the ray is inside at once
#define RECENT_CROSSINGS 8   // crossings kept to drop one found twice
#define CONTAINING 8         // footprints a building's source stands in, its own among them

// ray.rs RayEnds
struct RayEnds {
    float source_height_m;
    float receiver_altitude_m;
    float source_ground_factor;   // negative: the ground under the source
    float platform_half_width_m;
    u64 own_footprint;            // the source's building
};

// A terrain sample of the ray: distance from the source, altitude (flattened to the source's
// ground within its platform), ground factor.
struct TerrainPoint {
    float x;
    float z;
    float g;
};

// One state's hull (rubber_band.rs diffraction_path): the source first, the candidates that
// stand above the state's chord after the monotone chain; while no candidate does, the one
// nearest to blocking.
struct StateStream {
    StateRay ray;
    HullEntry hull[HULL_CAPACITY];
    int depth;
    bool blocked;
    bool has_best;
    float best_delta;
    HullEntry best;
};

struct OpenRoof {
    u64 footprint_id;
    float x;
    float top;
};

// A crossing kept, for dropping the same one found again.
struct KeptCrossing {
    u64 footprint_id;
    u64 edge;
    float t;
};

// Everything a ray carries while it streams.
struct RayStream {
    const TerrainScene* ground;
    float receiver_x, receiver_y;    // frame metres
    float offset_x, offset_y;        // source - receiver
    u64 receiver_footprint;          // the receiver's building: its walls do not screen it
    // The footprints containing a building's source, whose walls and roofs do not screen it:
    // those the ray crosses an odd number of times, and its own (ray.rs PathBuffers::fill).
    u64 containing[CONTAINING];
    int containing_count;
    float length;
    float inverse_length;
    float platform_half_width_m;
    StreamedPath path;
    // The terrain vertices: the walk producing them, how many there are and were sampled, the
    // last TERRAIN_HISTORY.
    LatticeWalk walk;
    int count;
    int sampled;
    TerrainPoint history[TERRAIN_HISTORY];
    // The path is streamed over [0, streamed_to]; `segment` is the sample ending the terrain
    // stretch streamed_to lies in, `below` and `above` its two ends.
    float streamed_to;
    int segment;
    TerrainPoint below, above;
    float covered_to;                // where the roofs closed so far end
    OpenRoof open[OPEN_ROOFS];
    int open_count;
    // Roofs closed by crossings at one t, applied together in the order the CPU sorts them.
    OpenRoof closing_from[OPEN_ROOFS];
    float closing_top[OPEN_ROOFS];
    int closing_count;
    float closing_x;
    KeptCrossing recent[RECENT_CROSSINGS];   // a ring, the newest at recent_next - 1
    int recent_next;
    int recent_count;
    float last_x;
    StateStream states[2];
};

// Terrain vertex k (generated in order, kept for TERRAIN_HISTORY vertices): the source, the walk's
// crossings, the receiver.
__device__ u32 ray_sample(RayStream& s, int k, TerrainPoint* out) {
    while (s.sampled <= k) {
        GroundSample sample;
        u32 failed;
        float t;
        if (s.sampled == 0) {
            t = 0.0f;
            failed = ground_at(*s.ground, s.receiver_x + s.offset_x, s.receiver_y + s.offset_y, &sample);
        } else if (s.sampled == s.count - 1) {
            t = 1.0f;
            failed = ground_at(*s.ground, s.receiver_x, s.receiver_y, &sample);
        } else {
            bool on_row;
            int row;
            if (!walk_next(*s.ground, &s.walk, &t, &on_row, &row)) return FAILED_CAPACITY;
            float keep = 1.0f - t;
            failed = walk_sample(*s.ground, s.receiver_x + s.offset_x * keep, s.receiver_y + s.offset_y * keep, on_row, row,
                                 s.walk.north, &sample);
        }
        if (failed) return failed;
        TerrainPoint point;
        point.x = t * s.length;
        point.g = sample.ground_factor;
        if (s.sampled == 0) {
            s.path.source_ground_z = sample.height_m;
            s.path.source_ground_g = sample.ground_factor;
        }
        point.z = point.x < s.platform_half_width_m ? fminf(sample.height_m, s.path.source_ground_z) : sample.height_m;
        s.history[s.sampled % TERRAIN_HISTORY] = point;
        s.sampled++;
    }
    if (k <= s.sampled - 1 - TERRAIN_HISTORY) return FAILED_CAPACITY;
    *out = s.history[k % TERRAIN_HISTORY];
    return 0;
}

// ray::interpolate over the terrain samples at `x` (with its ground factor), from the samples still
// kept; `hint` is a sample index near x.
__device__ u32 ray_terrain_at(RayStream& s, float x, int hint, float* z, float* g) {
    TerrainPoint below = s.below, above = s.above;
    if (below.x <= x && (x < above.x || s.segment == s.count - 1)) {
        float f = above.x > below.x ? clampf((x - below.x) / (above.x - below.x), 0.0f, 1.0f) : 0.0f;
        *z = below.z + f * (above.z - below.z);
        *g = below.g + f * (above.g - below.g);
        return 0;
    }
    int upper = max(1, min(hint, s.count - 1));
    u32 failed;
    while (true) {
        if ((failed = ray_sample(s, upper - 1, &below))) return failed;
        if (upper > 1 && below.x > x) { upper--; continue; }
        if ((failed = ray_sample(s, upper, &above))) return failed;
        if (upper < s.count - 1 && above.x <= x) { upper++; continue; }
        break;
    }
    float f = above.x > below.x ? clampf((x - below.x) / (above.x - below.x), 0.0f, 1.0f) : 0.0f;
    *z = below.z + f * (above.z - below.z);
    *g = below.g + f * (above.g - below.g);
    return 0;
}

// A linear stretch [a, b] of the path into a hull entry's sums: its part after the point (up to
// `end`) about the point's own terrain, and, for a roof's rise, its part before the point.
__device__ __forceinline__ void entry_stretch(HullEntry& e, float a, float b, float end, float za, float zb, float ga, float gb, bool rise) {
    float lo = fmaxf(a, e.x), hi = fminf(b, end);
    if (hi > lo) {
        float f_lo = (lo - a) / (b - a), f_hi = (hi - a) / (b - a);
        add_stretch(e.after, lo, hi, za + f_lo * (zb - za), za + f_hi * (zb - za), ga + f_lo * (gb - ga), ga + f_hi * (gb - ga),
                    e.x, rise ? 0.0f : e.terrain_z);
    }
    if (rise && e.x > a) {
        float hi_before = fminf(b, e.x);
        float f_hi = (hi_before - a) / (b - a);
        add_stretch(e.before, a, hi_before, za, za + f_hi * (zb - za), ga, ga + f_hi * (gb - ga), 0.0f, 0.0f);
    }
}

// A stretch [a, b] of the path, linear from (za, ga) to (zb, gb), into every sum it belongs to: the
// whole path, the hull points whose stretch it falls in, and each state's best point. A terrain
// stretch lies after every point streamed (only the hull's top takes it); a roof's `rise` over its
// terrain comes when the roof closes and is shared by the points it spans, before and after them.
__device__ void stream_stretch(RayStream& s, float a, float b, float za, float zb, float ga, float gb, bool rise) {
    if (b <= a) return;
    add_stretch(s.path.whole, a, b, za, zb, ga, gb, 0.0f, rise ? 0.0f : s.path.source_ground_z);
    for (int state = 0; state < 2; state++) {
        StateStream& st = s.states[state];
        for (int i = st.depth - 1; i >= 1; i--) {
            float end = i == st.depth - 1 ? QM_INFINITY : st.hull[i + 1].x;
            if (end <= a) break;
            entry_stretch(st.hull[i], a, b, end, za, zb, ga, gb, rise);
            if (!rise) break;
        }
        if (st.has_best && !st.blocked) entry_stretch(st.best, a, b, QM_INFINITY, za, zb, ga, gb, rise);
    }
}

// Whether band point b stands on or under the state's ray from a to (x, z): straight in calm air,
// the arc of the one radius Gamma downwind (rubber_band.rs diffraction_path).
__device__ __forceinline__ bool under_state_ray(const StateRay& ray, const HullEntry& a, float x, float z, const HullEntry& b) {
    PlanePoint from = {a.x, a.z}, to = {x, z};
    return b.z <= chord_altitude(from, to, b.x) + ray_height_above_chord(ray, from, to, b.x);
}

// A diffraction candidate at (x, z), in the CPU's candidate order, into both states
// (rubber_band.rs diffraction_path): one above the state's chord after lowering joins the hull,
// otherwise it competes for the unblocked path's single point.
__device__ u32 stream_candidate(RayStream& s, float x, float z, float terrain_z, float terrain_g) {
    PlanePoint source = s.path.source, receiver = s.path.receiver;
    float chord = source.z + (receiver.z - source.z) * x * s.inverse_length;
    for (int state = 0; state < 2; state++) {
        StateStream& st = s.states[state];
        float lowered = z - ray_height_above_chord(st.ray, source, receiver, x);
        HullEntry entry;
        entry.x = x;
        entry.z = z;
        entry.lowered = lowered;
        entry.terrain_z = terrain_z;
        entry.terrain_g = terrain_g;
        entry.before = s.path.whole;
        entry.after.z = entry.after.xz = entry.after.g = 0.0f;
        if (lowered > chord) {
            st.blocked = true;
            while (st.depth >= 2) {
                const HullEntry& a = st.hull[st.depth - 2];
                const HullEntry& b = st.hull[st.depth - 1];
                if (!under_state_ray(st.ray, a, x, z, b)) break;
                // The popped point's stretch now belongs to the point below it.
                if (st.depth - 2 >= 1) {
                    HullEntry& below = st.hull[st.depth - 2];
                    Moments moved = moments_about(b.after, s.streamed_to - b.x, b.x, b.terrain_z, below.x, below.terrain_z);
                    below.after.z += moved.z;
                    below.after.xz += moved.xz;
                    below.after.g += moved.g;
                }
                st.depth--;
            }
            if (st.depth >= HULL_CAPACITY) return FAILED_CAPACITY;
            st.hull[st.depth++] = entry;
        } else if (!st.blocked) {
            PlanePoint single[1] = {{x, z}};
            float delta = state_ray_path_difference(st.ray, source, single, 1, receiver);
            if (!st.has_best || delta > st.best_delta) {
                st.has_best = true;
                st.best_delta = delta;
                st.best = entry;
            }
        }
    }
    return 0;
}

// Streams the path up to `x`: the terrain stretches before it, and every terrain sample at or
// before it as a candidate (samples come before obstacle tops at one distance, as the CPU's stable
// sort leaves them).
__device__ u32 stream_to(RayStream& s, float x) {
    u32 failed;
    while (true) {
        TerrainPoint below = s.below, above = s.above;
        float b = fminf(x, above.x);
        if (b > s.streamed_to) {
            float span = above.x - below.x;
            float fa = span > 0.0f ? (s.streamed_to - below.x) / span : 0.0f;
            float fb = span > 0.0f ? (b - below.x) / span : 1.0f;
            stream_stretch(s, s.streamed_to, b, below.z + fa * (above.z - below.z), below.z + fb * (above.z - below.z),
                           below.g + fa * (above.g - below.g), below.g + fb * (above.g - below.g), false);
            s.streamed_to = b;
        }
        if (above.x > x || s.segment == s.count - 1) return 0;
        if (above.x > 0.0f && above.x < s.length) {
            if ((failed = stream_candidate(s, above.x, above.z, above.z, above.g))) return failed;
        }
        s.segment++;
        s.below = above;
        if ((failed = ray_sample(s, s.segment, &s.above))) return failed;
    }
}

// A roof from the wall at x0 (top0) to the wall at x1 (top1), clipped to start where the roofs
// closed before it end (clip_roofs_in_closing_order: roofs close here in the order the ray leaves
// them), streamed as its rise over the terrain stretch by stretch.
__device__ u32 stream_roof(RayStream& s, float x0, float top0, float x1, float top1) {
    if (x1 <= s.covered_to) return 0;
    if (x0 < s.covered_to) {
        top0 += (s.covered_to - x0) / (x1 - x0) * (top1 - top0);
        x0 = s.covered_to;
    }
    s.covered_to = x1;
    float slope = x1 > x0 ? (top1 - top0) / (x1 - x0) : 0.0f;
    // k: the first sample after x0.
    int k = s.segment;
    TerrainPoint sample;
    u32 failed;
    while (k > 1) {
        if ((failed = ray_sample(s, k - 1, &sample))) return failed;
        if (sample.x <= x0) break;
        k--;
    }
    float a = x0, terrain_a, ground_a;
    if ((failed = ray_terrain_at(s, a, k, &terrain_a, &ground_a))) return failed;
    while (a < x1) {
        float b = x1;
        if (k < s.count - 1) {
            if ((failed = ray_sample(s, k, &sample))) return failed;
            b = fminf(sample.x, x1);
        }
        float terrain_b, ground_b;
        if ((failed = ray_terrain_at(s, b, k, &terrain_b, &ground_b))) return failed;
        stream_stretch(s, a, b, top0 + slope * (a - x0) - terrain_a, top0 + slope * (b - x0) - terrain_b, -ground_a, -ground_b, true);
        a = b;
        terrain_a = terrain_b;
        ground_a = ground_b;
        k++;
    }
    return 0;
}

// Whether a crossing was found before, remembering it otherwise; the crossings of the receiver's
// building are dropped first (evaluate.rs). crossings::sort_and_deduplicate keeps one crossing per
// footprint within 1e-9 of the ray: the same edge seen from two cells, or two edges meeting where the
// ray passes a vertex. Here the same edge is the same key, and two edges of a footprint are one
// crossing within f32's error of t (about 1e-7 of t: the terms grow with the distance from the
// source). A wider window would merge a ray clipping a building's corner within a millimetre, and
// its odd count would take the building for the source's own (22 dB louder at 919 m).
__device__ u32 crossing_dropped(RayStream& s, const Crossing& c, bool* dropped) {
    *dropped = true;
    if (c.footprint_id == s.receiver_footprint) return 0;
    // Kept crossings come in order of t, but a cell takes crossings up to its slack beyond its
    // stretch, so the same edge found again from the next cell lies within twice the slack.
    float window = 1e-9f + 5e-7f * c.t;
    float reach = fmaxf(window, 2.0f * CELL_WINDOW_SLACK);
    for (int k = 1; k <= s.recent_count; k++) {
        const KeptCrossing& r = s.recent[(s.recent_next - k + RECENT_CROSSINGS) % RECENT_CROSSINGS];
        if (c.t - r.t > reach) break;
        if (r.edge == c.edge) return 0;
        if (r.footprint_id == c.footprint_id && fabsf(c.t - r.t) < window) return 0;
    }
    *dropped = false;
    // A full ring whose oldest crossing could still be found again: past what it can tell.
    if (s.recent_count == RECENT_CROSSINGS && c.t - s.recent[s.recent_next].t <= reach) return FAILED_CAPACITY;
    KeptCrossing& kept = s.recent[s.recent_next];
    kept.footprint_id = c.footprint_id;
    kept.edge = c.edge;
    kept.t = c.t;
    s.recent_next = (s.recent_next + 1) % RECENT_CROSSINGS;
    s.recent_count = min(s.recent_count + 1, RECENT_CROSSINGS);
    return 0;
}

// The first walk of a building's source's ray: each building crossing toggles its footprint, so the
// footprints left are those crossed an odd number of times.
struct ParityTaker {
    RayStream* s;
    __device__ u32 operator()(const Crossing& c) {
        RayStream& r = *s;
        bool dropped;
        u32 failed = crossing_dropped(r, c, &dropped);
        if (failed || dropped || !c.building) return failed;
        for (int k = 0; k < r.containing_count; k++) {
            if (r.containing[k] == c.footprint_id) {
                r.containing[k] = r.containing[--r.containing_count];
                return 0;
            }
        }
        if (r.containing_count == CONTAINING - 1) return FAILED_CAPACITY;
        r.containing[r.containing_count++] = c.footprint_id;
        return 0;
    }
};

// One crossing, in order: the CPU's filters (the receiver's building, one crossing per footprint
// and point, the footprints containing the source), then its wall as an entry or exit of its
// footprint's roof and its top as a candidate.
// The roofs closed at one t: clip_roofs_in_closing_order sorts by the far wall, then the near one.
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
        u32 failed = stream_roof(s, s.closing_from[i].x, s.closing_from[i].top, s.closing_x, s.closing_top[i]);
        if (failed) return failed;
    }
    s.closing_count = 0;
    return 0;
}

// One crossing kept: a footprint the receiver stands in is passed through, a building's entry
// opens its roof and its exit queues the roof to close.
__device__ u32 take_crossing(RayStream& s, const Crossing& c) {
    for (int k = 0; k < s.containing_count; k++) {
        if (s.containing[k] == c.footprint_id) return 0;
    }
    float x = fmaxf(c.t * s.length, s.last_x);
    s.last_x = x;
    u32 failed;
    if ((failed = stream_to(s, x))) return failed;
    float terrain_z, terrain_g;
    if ((failed = ray_terrain_at(s, x, s.segment, &terrain_z, &terrain_g))) return failed;
    float top = terrain_z + c.height_m;
    if (c.building) {
        int open = -1;
        for (int k = 0; k < s.open_count; k++) {
            if (s.open[k].footprint_id == c.footprint_id) open = k;
        }
        if (open >= 0) {
            if (s.closing_count == OPEN_ROOFS) return FAILED_CAPACITY;
            s.closing_from[s.closing_count] = s.open[open];
            s.closing_top[s.closing_count] = top;
            s.closing_count++;
            s.closing_x = x;
            s.open[open] = s.open[--s.open_count];
        } else {
            if (s.open_count == OPEN_ROOFS) return FAILED_CAPACITY;
            s.open[s.open_count].footprint_id = c.footprint_id;
            s.open[s.open_count].x = x;
            s.open[s.open_count].top = top;
            s.open_count++;
        }
    }
    if (x > 0.0f && x < s.length && (failed = stream_candidate(s, x, top, terrain_z, terrain_g))) return failed;
    return 0;
}

__device__ u32 stream_crossing(RayStream& s, const Crossing& c) {
    bool dropped;
    u32 failed = crossing_dropped(s, c, &dropped);
    if (failed) return failed;
    if (!dropped && (failed = take_crossing(s, c))) return failed;
    // The roofs closed at this t, once no other crossing at it follows, the last one dropped or
    // passed through alike (one call site: a warp that splits around a call stays split long
    // after).
    if (!c.tie && s.closing_count > 0) return stream_closing_roofs(s);
    return 0;
}

// One ray's boundary in each state and its slant: ray::ray_terms before its band loop.
struct Transfer {
    StateTerm states[2];
    float slant_m;
};

__device__ __forceinline__ float attenuation_energy(float attenuation_db) {
    return expf(-attenuation_db * (QM_LN_10 / 10.0f));
}

// What the crossing walk hands each crossing to.
struct CrossingTaker {
    RayStream* s;
    __device__ u32 operator()(const Crossing& c) { return stream_crossing(*s, c); }
};

// The receiver's hull closes each state (the CPU adds R last), merging every popped point's
// stretch into the point below it.
__device__ void close_hull(RayStream& s, StateStream& st) {
    PlanePoint receiver = s.path.receiver;
    while (st.depth >= 2) {
        const HullEntry& a = st.hull[st.depth - 2];
        const HullEntry& b = st.hull[st.depth - 1];
        if (!under_state_ray(st.ray, a, receiver.x, receiver.z, b)) break;
        if (st.depth - 2 >= 1) {
            HullEntry& below = st.hull[st.depth - 2];
            Moments moved = moments_about(b.after, s.streamed_to - b.x, b.x, b.terrain_z, below.x, below.terrain_z);
            below.after.z += moved.z;
            below.after.xz += moved.xz;
            below.after.g += moved.g;
        }
        st.depth--;
    }
}

// ray::ray_terms from (sx, sy) to the receiver at (rx, ry) at `receiver_altitude_m` standing in
// building `receiver_footprint`, with the obstacles when `obstacles_on_ray`; 0 or a failure.
__device__ u32 ray_transfer(const TerrainScene& ground, const ObstacleScene& obstacles, float sx, float sy, float rx, float ry,
                            u64 receiver_footprint, bool obstacles_on_ray, const RayEnds& ends, RayStream& s, Transfer* transfer) {
    s.ground = &ground;
    s.receiver_x = rx;
    s.receiver_y = ry;
    s.offset_x = sx - rx;
    s.offset_y = sy - ry;
    s.receiver_footprint = receiver_footprint;
    float horizontal = fmaxf(hypotf(s.offset_x, s.offset_y), 1.0f);
    s.length = horizontal;
    s.inverse_length = 1.0f / horizontal;
    s.platform_half_width_m = ends.platform_half_width_m;
    walk_start(ground, sx, sy, rx, ry, &s.walk);
    LatticeWalk counting = s.walk;
    float t_counted;
    bool on_row_counted;
    int row_counted;
    s.count = 2;
    while (walk_next(ground, &counting, &t_counted, &on_row_counted, &row_counted)) s.count++;
    s.sampled = 0;
    TerrainPoint first;
    u32 failed = ray_sample(s, 0, &first);
    if (failed) return failed;
    float source_altitude = s.path.source_ground_z + ends.source_height_m;
    s.path.source.x = 0.0f;
    s.path.source.z = source_altitude;
    s.path.receiver.x = horizontal;
    s.path.receiver.z = ends.receiver_altitude_m;
    s.path.source_ground_factor = ends.source_ground_factor >= 0.0f ? ends.source_ground_factor : first.g;
    s.path.whole.z = s.path.whole.xz = s.path.whole.g = 0.0f;
    s.streamed_to = 0.0f;
    s.segment = 1;
    s.below = first;
    if ((failed = ray_sample(s, 1, &s.above))) return failed;
    s.covered_to = -QM_INFINITY;
    s.open_count = 0;
    s.closing_count = 0;
    s.recent_next = 0;
    s.recent_count = 0;
    s.last_x = 0.0f;
    for (int state = 0; state < 2; state++) {
        StateStream& st = s.states[state];
        st.ray = state_ray_between(state, s.path.source, s.path.receiver);
        st.depth = 1;
        st.hull[0].x = 0.0f;
        st.hull[0].z = source_altitude;
        st.hull[0].lowered = source_altitude;
        st.blocked = false;
        st.has_best = false;
        st.best_delta = 0.0f;
    }
    s.containing_count = 0;
    if (obstacles_on_ray && ends.own_footprint != 0) {
        ParityTaker parity = {&s};
        if ((failed = walk_crossings(obstacles, sx, sy, rx, ry, parity))) return failed;
        s.containing[s.containing_count++] = ends.own_footprint;
        s.recent_next = 0;
        s.recent_count = 0;
    }
    if (obstacles_on_ray) {
        CrossingTaker take = {&s};
        failed = walk_crossings(obstacles, sx, sy, rx, ry, take);
        if (failed) return failed;
    }
    if (s.closing_count > 0 && (failed = stream_closing_roofs(s))) return failed;
    if ((failed = stream_to(s, horizontal))) return failed;
    for (int state = 0; state < 2; state++) {
        StateStream& st = s.states[state];
        close_hull(s, st);
        if (st.blocked) {
            state_term(s.path, st.ray, st.hull + 1, st.depth - 1, true, &transfer->states[state]);
        } else {
            state_term(s.path, st.ray, &st.best, st.has_best ? 1 : 0, false, &transfer->states[state]);
        }
    }
    transfer->slant_m = fmaxf(hypotf(horizontal, ends.receiver_altitude_m - source_altitude), 1.0f);
    return 0;
}

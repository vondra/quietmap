// One source-receiver ray in f32, in two passes: popup/src/scene.rs (the ground at every lattice
// line the ray crosses, walked in scene.cuh into a per-thread buffer with the terrain's moments from
// the source and each stretch's metres in the crowns), then physics/src/ray.rs (the vertical path of
// terrain, roofs and obstacle tops, and its transfer) over it. Terrain samples and wall crossings
// arrive in order of distance: every candidate at once to both states' bands (rubber_band.rs: a
// monotone chain of the states' rays), a footprint's crossings pair into a roof kept with its rise;
// the mean planes' moments are taken once the hulls are known, from the source out of the prefixes
// and to the receiver summed. The CPU keeps the same path in arrays and sorts; the answer is the
// same.

// Per-thread capacities; a ray needing more fails with FAILED_CAPACITY and goes to the CPU.
// Terrain vertices a ray keeps: 12 km east-west crosses ~1,200 lattice lines at 71 N, 2,048 at 79 N.
#define RAY_SAMPLES 2048
#define HULL_CAPACITY 40     // points of one state's hull (dev4's oracle rays needed at most 21)
#define OPEN_ROOFS 12        // footprints the ray is inside at once
#define ROOFS 512            // roofs one ray passes under
#define RECENT_CROSSINGS 8   // crossings kept to drop one found twice
#define CONTAINING 8         // footprints a building's source stands in, its own among them
#define LINE_NODES 64        // quadrature nodes of one line piece (five buckets, each wide one split at its edges)
// ray::CREST_ENDS_M and CREST_GROUP: beyond 300 m of both ends only the highest of each 8 samples
// may diffract.
#define CREST_ENDS_M 300.0f
#define CREST_GROUP 8

// ray.rs RayEnds
struct RayEnds {
    float source_height_m;
    float source_ground_m;        // a bridge's deck under the source; NAN: the terrain under it
    float receiver_altitude_m;
    float source_ground_factor;   // negative: the ground under the source
    float platform_half_width_m;
    u64 own_footprint;            // the source's building
};

// A terrain sample of the ray: distance from the source, altitude (flattened to the source's
// ground within its platform), ground factor, and the next sample that may diffract
// (ray::crests_of) after one that may (-1: none).
struct TerrainPoint {
    float x;
    float z;
    float g;
    int next_crest;
};

// A roof the ray passes under, clipped where the roofs closed before it end: its walls'
// distances and tops, and its rise over the terrain's moments about (x0, 0).
struct Roof {
    float x0, top0, x1, top1;
    Moments rise;
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
    float source_stand_z;            // the ground the source stands on: a deck, else the terrain
    StreamedPath path;
    // The terrain vertices, all of them: the source, every lattice line crossed, the receiver; and
    // the terrain's moments from the source to each about the whole path's origin (0, the source's
    // ground), which every range from the source is read from.
    int count;
    TerrainPoint samples[RAY_SAMPLES];
    Moments prefix[RAY_SAMPLES];
    // The next sample that may diffract, not yet a candidate; the sample ending the stretch of the
    // last crossing.
    int next_crest;
    int segment;
    // The roofs closed so far, in order of distance, and where they end.
    Roof roofs[ROOFS];
    int roof_count;
    float covered_to;
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
    // The cover-weighted metres of each state's ray inside the crowns (the first pass's).
    float foliage_m[2];
};

// The tree cover where a ray at `altitude` runs inside the crowns over `ground`, else 0.
__device__ __forceinline__ float cover_inside_crowns(float altitude, const GroundSample& ground) {
    float above = altitude - ground.height_m;
    return above > 0.0f && above <= ground.canopy_m ? ground.forest_cover : 0.0f;
}

// The crowns the states' rays have passed so far: the last sample's ground and distance from the
// source, and each state's cover-weighted metres inside them (physics/src/foliage.rs depth_m).
struct Crowns {
    GroundSample behind;
    float behind_x;
    float metres[2];
};

// Each state's foliage depth over the stretch from the last sample to `ground` at `x`: the cover
// where the state's ray runs inside the crowns, linear between the samples, times the stretch's
// length along the ray.
__device__ __forceinline__ void add_foliage(const PlanePoint& source, const PlanePoint& receiver, const StateRay* rays, Crowns& crowns,
                                            float x, const GroundSample& ground) {
    const GroundSample& behind = crowns.behind;
    if (behind.forest_cover > 0.0f || ground.forest_cover > 0.0f) {
        for (int state = 0; state < 2; state++) {
            float za = chord_altitude(source, receiver, crowns.behind_x)
                + ray_height_above_chord(rays[state], source, receiver, crowns.behind_x);
            float zb = chord_altitude(source, receiver, x) + ray_height_above_chord(rays[state], source, receiver, x);
            float cover = cover_inside_crowns(za, behind) + cover_inside_crowns(zb, ground);
            crowns.metres[state] += 0.5f * cover * hypotf(x - crowns.behind_x, zb - za);
        }
    }
    crowns.behind = ground;
    crowns.behind_x = x;
}

// Terrain vertex k of the ray, as `sample_ray` wrote it.
__device__ __forceinline__ u32 ray_sample(RayStream& s, int k, TerrainPoint* out) {
    if (k >= s.count) return FAILED_CAPACITY;
    *out = s.samples[k];
    return 0;
}

// The first of a ray's two passes: its terrain vertices into `s.samples` (the source, every lattice
// line crossed, the receiver) with the terrain's moments from the source, each state's foliage with
// them. Out of line, once a ray: the walk's state is not live while the second pass streams the
// hulls, so the kernel's registers are the larger pass's (above 168 the card keeps 8 warps an SM;
// evidence 2026-10-11, sampling). The pass's own state (the previous vertex, the moments so far,
// the open group of the middle's samples, the last sample that may diffract, the crowns) stays in
// registers: the ray's buffer lives in local memory, which half the card's time waits on.
__device__ __noinline__ u32 sample_ray(const TerrainScene& ground, float sx, float sy, const RayEnds& ends, RayStream& s) {
    GroundSample sample;
    u32 failed = ground_at(ground, s.receiver_x + s.offset_x, s.receiver_y + s.offset_y, &sample);
    if (failed) return failed;
    // The source stands on its deck, else on the terrain; the states' rays from there.
    float length = s.length, platform = s.platform_half_width_m;
    float stand = isnan(ends.source_ground_m) ? sample.height_m : ends.source_ground_m;
    float ground_z = fminf(sample.height_m, stand);
    s.source_stand_z = stand;
    s.path.source_ground_z = ground_z;
    s.path.source_ground_g = sample.ground_factor;
    PlanePoint source = {0.0f, stand + ends.source_height_m}, receiver = {length, ends.receiver_altitude_m};
    s.path.source = source;
    s.path.receiver = receiver;
    StateRay rays[2];
    for (int state = 0; state < 2; state++) rays[state] = s.states[state].ray = state_ray_between(state, source, receiver);
    Crowns crowns = {sample, 0.0f, {0.0f, 0.0f}};
    TerrainPoint previous = {0.0f, 0.0f < platform ? fminf(sample.height_m, stand) : sample.height_m, sample.ground_factor, -1};
    Moments prefix = {0.0f, 0.0f, 0.0f};
    s.samples[0] = previous;
    s.prefix[0] = prefix;
    int count = 1, group_size = 0, group_best = 0, last_crest = 0;
    float group_best_z = 0.0f;
    LatticeWalk walk;
    walk_start(ground, sx, sy, s.receiver_x, s.receiver_y, &walk);
    for (bool last = false; !last;) {
        float t;
        bool on_row;
        int row;
        if (walk_next(ground, &walk, &t, &on_row, &row)) {
            float keep = 1.0f - t;
            failed = walk_sample(ground, s.receiver_x + s.offset_x * keep, s.receiver_y + s.offset_y * keep, on_row, row,
                                 walk.north, &sample);
        } else {
            t = 1.0f;
            last = true;
            failed = ground_at(ground, s.receiver_x, s.receiver_y, &sample);
        }
        if (failed) return failed;
        if (count == RAY_SAMPLES) return FAILED_CAPACITY;
        TerrainPoint point;
        point.x = t * length;
        point.z = point.x < platform ? fminf(sample.height_m, stand) : sample.height_m;
        point.g = sample.ground_factor;
        point.next_crest = -1;
        add_foliage(source, receiver, rays, crowns, point.x, sample);
        add_stretch(prefix, previous.x, point.x, previous.z, point.z, previous.g, point.g, 0.0f, ground_z);
        // A sample that may diffract follows the last that may in their chain.
        bool middle = point.x > CREST_ENDS_M && point.x < length - CREST_ENDS_M;
        if (!middle && group_size > 0) {
            s.samples[last_crest].next_crest = group_best;
            last_crest = group_best;
            group_size = 0;
        }
        s.samples[count] = point;
        s.prefix[count] = prefix;
        if (middle) {
            if (group_size == 0 || point.z > group_best_z) {
                group_best = count;
                group_best_z = point.z;
            }
            if (++group_size == CREST_GROUP) {
                s.samples[last_crest].next_crest = group_best;
                last_crest = group_best;
                group_size = 0;
            }
        } else {
            s.samples[last_crest].next_crest = count;
            last_crest = count;
        }
        previous = point;
        count++;
    }
    s.count = count;
    s.foliage_m[0] = crowns.metres[0];
    s.foliage_m[1] = crowns.metres[1];
    return 0;
}

// ray::interpolate on the stretch from `lo` to `hi` at `x` (with its ground factor).
__device__ __forceinline__ void terrain_on(const TerrainPoint& lo, const TerrainPoint& hi, float x, float* z, float* g) {
    float f = hi.x > lo.x ? clampf((x - lo.x) / (hi.x - lo.x), 0.0f, 1.0f) : 0.0f;
    *z = lo.z + f * (hi.z - lo.z);
    *g = lo.g + f * (hi.g - lo.g);
}

// The terrain at `x` (ray::interpolate): `segment` moves to the sample ending the stretch that
// holds x (crossings arrive in order of distance, so it moves ahead).
__device__ void ray_terrain_at(RayStream& s, float x, float* z, float* g) {
    int k = s.segment;
    while (k > 1 && s.samples[k - 1].x > x) k--;
    while (k < s.count - 1 && s.samples[k].x <= x) k++;
    s.segment = k;
    terrain_on(s.samples[k - 1], s.samples[k], x, z, g);
}

// The sample ending the stretch that holds `x` (1 to the last).
__device__ int stretch_holding(const RayStream& s, float x) {
    int lo = 1, hi = s.count - 1;
    while (lo < hi) {
        int mid = (lo + hi) / 2;
        if (s.samples[mid].x <= x) lo = mid + 1;
        else hi = mid;
    }
    return lo;
}

// The rise of `roof` over the terrain on [a, b] within it, its moments about (x_r, 0): stretch by
// stretch of the terrain under it, a roof hard (its ground factor rises by minus the terrain's).
__device__ Moments roof_rise(const RayStream& s, const Roof& roof, float a, float b, float x_r) {
    Moments m = {0.0f, 0.0f, 0.0f};
    float slope = roof.x1 > roof.x0 ? (roof.top1 - roof.top0) / (roof.x1 - roof.x0) : 0.0f;
    int k = stretch_holding(s, a);
    float za, ga;
    terrain_on(s.samples[k - 1], s.samples[k], a, &za, &ga);
    while (a < b) {
        float next = k < s.count - 1 ? fminf(s.samples[k].x, b) : b, zb, gb;
        terrain_on(s.samples[k - 1], s.samples[k], next, &zb, &gb);
        add_stretch(m, a, next, roof.top0 + slope * (a - roof.x0) - za, roof.top0 + slope * (next - roof.x0) - zb, -ga, -gb, x_r, 0.0f);
        a = next;
        za = zb;
        ga = gb;
        k = min(k + 1, s.count - 1);
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

// The path's moments from the source to `x` about (0, the source's ground): the terrain's prefix
// up to x and the roofs' rises before it.
__device__ Moments path_from_source(const RayStream& s, float x) {
    int k = stretch_holding(s, x);
    const TerrainPoint &lo = s.samples[k - 1], &hi = s.samples[k];
    Moments m = s.prefix[k];
    if (x < hi.x) {
        float z, g;
        terrain_on(lo, hi, x, &z, &g);
        m = s.prefix[k - 1];
        add_stretch(m, lo.x, x, lo.z, z, lo.g, g, 0.0f, s.path.source_ground_z);
    }
    for (int r = 0; r < s.roof_count && s.roofs[r].x0 < x; r++) {
        const Roof& roof = s.roofs[r];
        add_moments(m, roof.x1 <= x ? moved_to(roof.rise, roof.x0, 0.0f) : roof_rise(s, roof, roof.x0, x, 0.0f));
    }
    return m;
}

// The path's moments from `x` to the receiver about (x, z_x): the terrain's stretches after x and
// the roofs' rises after it, summed here, not as a difference of the prefixes (which f32 would
// lose for a short stretch at the end of a long ray).
__device__ Moments path_to_receiver(const RayStream& s, float x, float z_x) {
    Moments m = {0.0f, 0.0f, 0.0f};
    int k = stretch_holding(s, x);
    float za, ga;
    terrain_on(s.samples[k - 1], s.samples[k], x, &za, &ga);
    for (float a = x; k < s.count; k++) {
        const TerrainPoint& hi = s.samples[k];
        add_stretch(m, a, hi.x, za, hi.z, ga, hi.g, x, z_x);
        a = fmaxf(a, hi.x);
        za = hi.z;
        ga = hi.g;
    }
    for (int r = s.roof_count - 1; r >= 0 && s.roofs[r].x1 > x; r--) {
        const Roof& roof = s.roofs[r];
        add_moments(m, roof.x0 >= x ? moved_to(roof.rise, roof.x0, x) : roof_rise(s, roof, x, roof.x1, x));
    }
    return m;
}

// Whether band point b stands on or under the state's ray from a to (x, z): straight in calm air,
// the arc of the one radius Gamma downwind (rubber_band.rs diffraction_path).
__device__ __forceinline__ bool under_state_ray(const StateRay& ray, const HullEntry& a, float x, float z, const HullEntry& b) {
    PlanePoint from = {a.x, a.z}, to = {x, z};
    return b.z <= chord_altitude(from, to, b.x) + ray_height_above_chord(ray, from, to, b.x);
}

// What every candidate of one ray reads: the path's ends, its length and the states' rays, in the
// stream's registers rather than the ray's buffer.
struct RayFrame {
    PlanePoint source, receiver;
    float length, inverse_length;
    StateRay rays[2];
};

// A diffraction candidate at (x, z), in the CPU's candidate order, into both states
// (rubber_band.rs diffraction_path): one above the state's chord after lowering joins the hull,
// otherwise it competes for the unblocked path's single point.
__device__ u32 stream_candidate(RayStream& s, const RayFrame& f, float x, float z, float terrain_z, float terrain_g) {
    PlanePoint source = f.source, receiver = f.receiver;
    float chord = source.z + (receiver.z - source.z) * x * f.inverse_length;
    for (int state = 0; state < 2; state++) {
        StateStream& st = s.states[state];
        const StateRay& ray = f.rays[state];
        float lowered = z - ray_height_above_chord(ray, source, receiver, x);
        HullEntry entry;
        entry.x = x;
        entry.z = z;
        entry.lowered = lowered;
        entry.terrain_z = terrain_z;
        entry.terrain_g = terrain_g;
        if (lowered > chord) {
            st.blocked = true;
            while (st.depth >= 2 && under_state_ray(ray, st.hull[st.depth - 2], x, z, st.hull[st.depth - 1])) st.depth--;
            if (st.depth >= HULL_CAPACITY) return FAILED_CAPACITY;
            st.hull[st.depth++] = entry;
        } else if (!st.blocked) {
            PlanePoint single[1] = {{x, z}};
            float delta = state_ray_path_difference(ray, source, single, 1, receiver);
            if (!st.has_best || delta > st.best_delta) {
                st.has_best = true;
                st.best_delta = delta;
                st.best = entry;
            }
        }
    }
    return 0;
}

// Every terrain sample that may diffract, up to `x`, into both states in order (samples come
// before obstacle tops at one distance, as the CPU's stable sort leaves them).
__device__ u32 stream_to(RayStream& s, const RayFrame& f, float x) {
    while (s.next_crest > 0) {
        TerrainPoint point;
        u32 failed = ray_sample(s, s.next_crest, &point);
        if (failed) return failed;
        if (point.x > x) return 0;
        if (point.x > 0.0f && point.x < f.length && (failed = stream_candidate(s, f, point.x, point.z, point.z, point.g))) return failed;
        s.next_crest = point.next_crest;
    }
    return 0;
}

// A roof from the wall at x0 (top0) to the wall at x1 (top1), clipped to start where the roofs
// closed before it end (clip_roofs_in_closing_order: roofs close here in the order the ray leaves
// them), kept with its rise over the terrain.
__device__ u32 stream_roof(RayStream& s, float x0, float top0, float x1, float top1) {
    if (x1 <= s.covered_to) return 0;
    if (x0 < s.covered_to) {
        top0 += (s.covered_to - x0) / (x1 - x0) * (top1 - top0);
        x0 = s.covered_to;
    }
    s.covered_to = x1;
    if (s.roof_count == ROOFS) return FAILED_CAPACITY;
    Roof& roof = s.roofs[s.roof_count++];
    roof.x0 = x0;
    roof.top0 = top0;
    roof.x1 = x1;
    roof.top1 = top1;
    roof.rise = roof_rise(s, roof, x0, x1, x0);
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
__device__ u32 take_crossing(RayStream& s, const RayFrame& f, const Crossing& c) {
    for (int k = 0; k < s.containing_count; k++) {
        if (s.containing[k] == c.footprint_id) return 0;
    }
    float x = fmaxf(c.t * f.length, s.last_x);
    s.last_x = x;
    u32 failed;
    if ((failed = stream_to(s, f, x))) return failed;
    float terrain_z, terrain_g;
    ray_terrain_at(s, x, &terrain_z, &terrain_g);
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
    if (x > 0.0f && x < f.length && (failed = stream_candidate(s, f, x, top, terrain_z, terrain_g))) return failed;
    return 0;
}

__device__ u32 stream_crossing(RayStream& s, const RayFrame& f, const Crossing& c) {
    bool dropped;
    u32 failed = crossing_dropped(s, c, &dropped);
    if (failed) return failed;
    if (!dropped && (failed = take_crossing(s, f, c))) return failed;
    // The roofs closed at this t, once no other crossing at it follows, the last one dropped or
    // passed through alike (one call site: a warp that splits around a call stays split long
    // after).
    if (!c.tie && s.closing_count > 0) return stream_closing_roofs(s);
    return 0;
}

// One ray's boundary and foliage depth in each state and its slant: ray::ray_terms before its
// band loop.
struct Transfer {
    StateTerm states[2];
    float foliage_m[2];
    float slant_m;
};

// physics/src/foliage.rs attenuation_db: ISO 9613-2 Table A.1 for `depth_m` of foliage, one band.
__constant__ float FOLIAGE_SHORT_DB[BANDS] = {0.0f, 0.0f, 1.0f, 1.0f, 1.0f, 1.0f, 2.0f, 3.0f};
__constant__ float FOLIAGE_DB_PER_M[BANDS] = {0.02f, 0.03f, 0.04f, 0.05f, 0.06f, 0.08f, 0.09f, 0.12f};
__device__ __forceinline__ float foliage_db(float depth_m, int band) {
    if (depth_m < 10.0f) return 0.0f;
    if (depth_m < 20.0f) return FOLIAGE_SHORT_DB[band];
    return FOLIAGE_DB_PER_M[band] * fminf(depth_m, 200.0f);
}

__device__ __forceinline__ float attenuation_energy(float attenuation_db) {
    return expf(-attenuation_db * (QM_LN_10 / 10.0f));
}

// What the crossing walk hands each crossing to.
struct CrossingTaker {
    RayStream* s;
    const RayFrame* f;
    __device__ u32 operator()(const Crossing& c) { return stream_crossing(*s, *f, c); }
};

// The receiver's hull closes each state (the CPU adds R last).
__device__ void close_hull(RayStream& s, StateStream& st) {
    PlanePoint receiver = s.path.receiver;
    while (st.depth >= 2 && under_state_ray(st.ray, st.hull[st.depth - 2], receiver.x, receiver.z, st.hull[st.depth - 1])) st.depth--;
}

// The second of a ray's two passes, over the samples `sample_ray` wrote: the obstacles' crossings
// and roofs, the hulls, and each state's terms. Out of line, once a ray.
__device__ __noinline__ u32 stream_ray(const ObstacleScene& obstacles, float sx, float sy, float rx, float ry, bool obstacles_on_ray,
                                       const RayEnds& ends, RayStream& s, Transfer* transfer) {
    float horizontal = s.length;
    float source_altitude = s.path.source.z;
    s.path.source_ground_factor = ends.source_ground_factor >= 0.0f ? ends.source_ground_factor : s.samples[0].g;
    for (int state = 0; state < 2; state++) {
        StateStream& st = s.states[state];
        st.depth = 1;
        st.hull[0].x = 0.0f;
        st.hull[0].z = source_altitude;
        st.hull[0].lowered = source_altitude;
        st.blocked = false;
        st.has_best = false;
        st.best_delta = 0.0f;
    }
    RayFrame frame = {s.path.source, s.path.receiver, horizontal, s.inverse_length, {s.states[0].ray, s.states[1].ray}};
    s.next_crest = s.samples[0].next_crest;
    s.segment = 1;
    s.roof_count = 0;
    u32 failed;
    s.covered_to = -QM_INFINITY;
    s.open_count = 0;
    s.closing_count = 0;
    s.recent_next = 0;
    s.recent_count = 0;
    s.last_x = 0.0f;
    s.containing_count = 0;
    if (obstacles_on_ray && ends.own_footprint != 0) {
        ParityTaker parity = {&s};
        if ((failed = walk_crossings(obstacles, sx, sy, rx, ry, parity))) return failed;
        s.containing[s.containing_count++] = ends.own_footprint;
        s.recent_next = 0;
        s.recent_count = 0;
    }
    if (obstacles_on_ray) {
        CrossingTaker take = {&s, &frame};
        failed = walk_crossings(obstacles, sx, sy, rx, ry, take);
        if (failed) return failed;
    }
    if (s.closing_count > 0 && (failed = stream_closing_roofs(s))) return failed;
    if ((failed = stream_to(s, frame, horizontal))) return failed;
    s.path.whole = path_from_source(s, horizontal);
    // Each state's diffraction points and the path's moments on either side of them, taken from the
    // samples and roofs once where both states share the point.
    Moments before[2], after[2];
    float first_x[2] = {-1.0f, -1.0f}, last_x[2] = {-1.0f, -1.0f};
    for (int state = 0; state < 2; state++) {
        StateStream& st = s.states[state];
        close_hull(s, st);
        const HullEntry* points = st.blocked ? st.hull + 1 : &st.best;
        int count = st.blocked ? st.depth - 1 : (st.has_best ? 1 : 0);
        before[state] = after[state] = Moments{0.0f, 0.0f, 0.0f};
        if (count > 0) {
            const HullEntry &first = points[0], &last = points[count - 1];
            first_x[state] = first.x;
            last_x[state] = last.x;
            before[state] = state == 1 && first.x == first_x[0] ? before[0] : path_from_source(s, first.x);
            after[state] = state == 1 && last.x == last_x[0] ? after[0] : path_to_receiver(s, last.x, last.terrain_z);
        }
        state_term(s.path, st.ray, points, count, st.blocked, before[state], after[state], &transfer->states[state]);
        transfer->foliage_m[state] = s.foliage_m[state];
    }
    transfer->slant_m = fmaxf(hypotf(horizontal, ends.receiver_altitude_m - source_altitude), 1.0f);
    return 0;
}

// ray::ray_terms from (sx, sy) to the receiver at (rx, ry) at `receiver_altitude_m` standing in
// building `receiver_footprint`, with the obstacles when `obstacles_on_ray`; 0 or a failure. Two
// passes: the terrain's samples, then the stream over them.
__device__ u32 ray_transfer(const TerrainScene& ground, const ObstacleScene& obstacles, float sx, float sy, float rx, float ry,
                            u64 receiver_footprint, bool obstacles_on_ray, const RayEnds& ends, RayStream& s, Transfer* transfer) {
    s.receiver_x = rx;
    s.receiver_y = ry;
    s.offset_x = sx - rx;
    s.offset_y = sy - ry;
    s.receiver_footprint = receiver_footprint;
    s.length = fmaxf(hypotf(s.offset_x, s.offset_y), 1.0f);
    s.inverse_length = 1.0f / s.length;
    s.platform_half_width_m = ends.platform_half_width_m;
    u32 failed = sample_ray(ground, sx, sy, ends, s);
    if (failed) return failed;
    return stream_ray(obstacles, sx, sy, rx, ry, obstacles_on_ray, ends, s, transfer);
}

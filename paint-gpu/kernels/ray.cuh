// One source-receiver ray in f32 through the terrain and obstacle physics of ray.rs.
// The obstacles' crossings are listed first (scene.cuh, with the CPU's filters); the terrain
// is then walked once from the source, its vertices and the crossings taken in order of distance:
// every candidate at once to both states' bands (rubber_band.rs: a monotone chain of the states'
// rays) with the terrain's moments from the source to it, a footprint's crossings paired into a roof
// kept with its rise, the moments to the receiver summed over the vertices kept behind the walk. The
// CPU builds the same path in arrays before sorting the candidates.

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

// The crossings walk: each crossing the CPU keeps (not the receiver's building's, one per footprint
// and point) listed, the last at each place, kept or not, closing the roofs queued there; for a
// building's source each building crossing toggles its footprint, so the footprints left are those
// crossed an odd number of times, whose crossings the terrain walk then passes.
struct CrossingLister {
    RayStream* s;
    bool parity;
    __device__ u32 operator()(const Crossing& c) {
        RayStream& r = *s;
        bool dropped;
        u32 failed = crossing_dropped(r, c, &dropped);
        if (failed) return failed;
        if (!dropped) {
            if (parity && c.building) {
                int k = 0;
                while (k < r.containing_count && r.containing[k] != c.footprint_id) k++;
                if (k < r.containing_count) {
                    r.containing[k] = r.containing[--r.containing_count];
                } else {
                    if (r.containing_count == CONTAINING - 1) return FAILED_CAPACITY;
                    r.containing[r.containing_count++] = c.footprint_id;
                }
            }
            if (r.crossing_count == RAY_CROSSINGS) return FAILED_CAPACITY;
            r.crossings[r.crossing_count++] = RayCrossing{c.t, c.height_m, c.footprint_id, c.building, 0};
        }
        if (!c.tie && r.crossing_count > 0) r.crossings[r.crossing_count - 1].closes = 1;
        return 0;
    }
};

// The crossings the ray keeps, in order, into `s.crossings`, and the footprints containing a
// building's source (ray.rs PathBuffers::fill): those crossed an odd number of times, and its own.
// Out of line, once a ray: the walk's state is not live while the terrain is walked.
__device__ __noinline__ u32 list_crossings(const ObstacleScene& obstacles, float sx, float sy, float rx, float ry, bool obstacles_on_ray,
                                           const RayEnds& ends, RayStream& s) {
    s.crossing_count = 0;
    s.recent_next = 0;
    s.recent_count = 0;
    s.containing_count = 0;
    if (!obstacles_on_ray) return 0;
    CrossingLister list = {&s, ends.own_footprint != 0};
    u32 failed = walk_crossings(obstacles, sx, sy, rx, ry, list);
    if (failed) return failed;
    if (ends.own_footprint != 0) s.containing[s.containing_count++] = ends.own_footprint;
    return 0;
}

// The next listed crossing the terrain walk takes after `*crossing`, and its distance from the source
// (never behind the last taken's, `*last_x`), INFINITY for none; the crossings of footprints
// containing the source passed, the roofs they close closed.
__device__ __forceinline__ u32 next_taken_crossing(RayStream& s, int* crossing, float* crossing_x, float* last_x) {
    for ((*crossing)++; *crossing < s.crossing_count; (*crossing)++) {
        const RayCrossing& c = s.crossings[*crossing];
        bool passed = false;
        for (int k = 0; k < s.containing_count; k++) passed |= s.containing[k] == c.footprint_id;
        if (!passed) {
            *crossing_x = *last_x = fmaxf(c.t * s.length, *last_x);
            return 0;
        }
        if (c.closes && s.closing_count > 0) {
            u32 failed = stream_closing_roofs(s);
            if (failed) return failed;
        }
    }
    *crossing_x = QM_INFINITY;
    return 0;
}

// One listed crossing at distance `x` on the stretch from `behind` to `ahead` (`prefix_behind` the
// terrain's moments from the source to behind): its wall as an entry or exit of its footprint's roof,
// its top a candidate, waiting while a middle stretch's crest is open (`crest_open`).
__device__ __forceinline__ u32 take_crossing(RayStream& s, BandState* bands, const RayFrame& f, const RayCrossing& c, float x,
                                             const TerrainPoint& behind, const TerrainPoint& ahead, const Moments& prefix_behind,
                                             bool crest_open) {
    float terrain_z, terrain_g;
    terrain_on(behind, ahead, x, &terrain_z, &terrain_g);
    Moments prefix = prefix_behind;
    add_stretch(prefix, behind.x, x, behind.z, terrain_z, behind.g, terrain_g, 0.0f, s.path.source_ground_z);
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
            s.closing_prefix = prefix;
            s.closing_vertex = s.vertices - 1;
            s.open[open] = s.open[--s.open_count];
        } else {
            if (s.open_count == OPEN_ROOFS) return FAILED_CAPACITY;
            s.open[s.open_count++] = OpenRoof{c.footprint_id, x, top, prefix, s.vertices - 1};
        }
    }
    if (x > 0.0f && x < f.length) {
        BandCandidate candidate = {x, top, terrain_z, terrain_g, prefix};
        if (crest_open) {
            if (s.waiting_count == WAITING_TOPS) return FAILED_CAPACITY;
            s.waiting[s.waiting_count++] = candidate;
        } else {
            u32 failed = stream_candidate(s, bands, f, candidate);
            if (failed) return failed;
        }
    }
    // Close the roofs at this distance once no other crossing there follows.
    if (c.closes && s.closing_count > 0) return stream_closing_roofs(s);
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

// The receiver's hull closes each state (the CPU adds R last).
__device__ void close_hull(RayStream& s, const StateStream& st, BandState& band, const StateRay& ray) {
    PlanePoint receiver = s.path.receiver;
    while (band.depth >= 2 && under_state_ray(ray, st.hull[band.depth - 2], receiver.x, receiver.z, st.hull[band.depth - 1])) band.depth--;
}

// The ray's one walk from the source over its terrain vertices (the source, every lattice line
// crossed, the 1/6" one's within FINE_ENDS_M of either end where the tiles have it, the receiver),
// each after the crossings before it, then each state's terms. Out of line, once a ray.
__device__ __noinline__ u32 walk_ray(const TerrainScene& ground, float sx, float sy, const RayEnds& ends, RayStream& s, Transfer* transfer) {
    GroundSample sample;
    u32 failed = ground_at(ground, s.receiver_x + s.offset_x, s.receiver_y + s.offset_y, &sample);
    if (failed) return failed;
    // The source stands on its deck, else on the terrain; the states' rays from there.
    float length = s.length, platform = s.platform_half_width_m;
    float stand = isnan(ends.source_ground_m) ? sample.height_m : ends.source_ground_m;
    float ground_z = fminf(sample.height_m, stand);
    s.path.source_ground_z = ground_z;
    s.path.source_ground_g = sample.ground_factor;
    s.path.source_ground_factor = ends.source_ground_factor >= 0.0f ? ends.source_ground_factor : sample.ground_factor;
    PlanePoint source = {0.0f, stand + ends.source_height_m}, receiver = {length, ends.receiver_altitude_m};
    s.path.source = source;
    s.path.receiver = receiver;
    StateRay rays[2];
    BandState bands[2];
    for (int state = 0; state < 2; state++) {
        rays[state] = state_ray_between(state, source, receiver);
        s.states[state].hull[0].x = 0.0f;
        s.states[state].hull[0].z = source.z;
        bands[state] = BandState{1, false, false, 0.0f};
    }
    float chord_m = plane_distance(source, receiver), gamma = rays[1].radius_m;
    RayFrame frame = {source, receiver, length, s.inverse_length, {rays[0], rays[1]}, chord_m, arc_excess(rays[1], chord_m),
                      sqrtf(gamma * gamma - 0.25f * chord_m * chord_m)};
    s.roof_count = 0;
    s.covered_to = -QM_INFINITY;
    s.open_count = 0;
    s.closing_count = 0;
    s.waiting_count = 0;
    s.vertices = 0;
    Crowns crowns = {sample, 0.0f, {0.0f, 0.0f}};
    TerrainPoint behind = {0.0f, 0.0f < platform ? fminf(sample.height_m, stand) : sample.height_m, sample.ground_factor};
    Moments prefix_behind = {0.0f, 0.0f, 0.0f};
    keep_vertex(s, behind);
    // The open stretch of the middle (its number from the source, -1: none) and its highest vertex;
    // the next listed crossing and its distance.
    int open_group = -1, crossing = -1;
    float crossing_x, last_crossing_x = 0.0f;
    if ((failed = next_taken_crossing(s, &crossing, &crossing_x, &last_crossing_x))) return failed;
    BandCandidate crest = {};
    // The stretches by lattice: their ends and the lattices' scales.
    float stretch_ends[3] = {1.0f, 1.0f, 1.0f};
    int stretch_scales[3] = {1, 1, 1};
    if (ground.has_fine) stretches_of(ground, sx, sy, s.receiver_x, s.receiver_y, stretch_ends, stretch_scales);
    int stretch = 0;
    LatticeWalk walk;
    walk_start(ground, stretch_scales[0], sx, sy, s.receiver_x, s.receiver_y, 0.0f, &walk);
    for (bool last = false; !last;) {
        float t;
        bool on_row;
        int row;
        if (walk_next(ground, &walk, stretch_ends[stretch], &t, &on_row, &row)) {
            float keep = 1.0f - t, x = s.receiver_x + s.offset_x * keep, y = s.receiver_y + s.offset_y * keep;
            failed = walk.scale == 1 ? walk_sample(ground, x, y, on_row, row, walk.north, &sample)
                                     : fine_walk_sample(ground, x, y, on_row, row, walk.north, &sample);
        } else if (stretch_ends[stretch] < 1.0f) {
            // A change of lattice is no line of the coarser one: a vertex there, on the finer.
            t = stretch_ends[stretch];
            float keep = 1.0f - t;
            failed = ground_at(ground, s.receiver_x + s.offset_x * keep, s.receiver_y + s.offset_y * keep, &sample);
            walk_start(ground, stretch_scales[++stretch], sx, sy, s.receiver_x, s.receiver_y, t, &walk);
        } else {
            t = 1.0f;
            last = true;
            failed = ground_at(ground, s.receiver_x, s.receiver_y, &sample);
        }
        if (failed) return failed;
        TerrainPoint ahead;
        ahead.x = t * length;
        ahead.z = ahead.x < platform ? fminf(sample.height_m, stand) : sample.height_m;
        ahead.g = sample.ground_factor;
        keep_vertex(s, ahead);
        // The crossings on the stretch to it, then the vertex (vertices before tops at one distance).
        while (crossing_x < ahead.x) {
            failed = take_crossing(s, bands, frame, s.crossings[crossing], crossing_x, behind, ahead, prefix_behind, open_group >= 0);
            if (failed) return failed;
            if ((failed = next_taken_crossing(s, &crossing, &crossing_x, &last_crossing_x))) return failed;
        }
        add_foliage(source, receiver, rays, crowns, ahead.x, sample);
        Moments prefix = prefix_behind;
        add_stretch(prefix, behind.x, ahead.x, behind.z, ahead.z, behind.g, ahead.g, 0.0f, ground_z);
        // A vertex that may diffract (ray::crests_of): within CREST_ENDS_M of either end each one, in
        // the middle the highest of each CREST_SPACING_M once its stretch closes.
        bool middle = ahead.x > CREST_ENDS_M && ahead.x < length - CREST_ENDS_M;
        int group = (int)floorf(ahead.x / CREST_SPACING_M);
        if (open_group >= 0 && (!middle || group != open_group)) {
            if ((failed = stream_crest_and_waiting(s, bands, frame, crest))) return failed;
            open_group = -1;
        }
        BandCandidate here = {ahead.x, ahead.z, ahead.z, ahead.g, prefix};
        if (middle) {
            if (open_group < 0 || ahead.z > crest.z) crest = here;
            open_group = group;
        } else if (ahead.x > 0.0f && ahead.x < length && (failed = stream_candidate(s, bands, frame, here))) {
            return failed;
        }
        behind = ahead;
        prefix_behind = prefix;
    }
    if (s.closing_count > 0 && (failed = stream_closing_roofs(s))) return failed;
    s.path.whole = path_from_source(s, length, prefix_behind);
    // Each state's diffraction points and the path's moments on either side of them, taken once
    // where both states share the point.
    Moments before[2], after[2];
    float first_x[2] = {-1.0f, -1.0f}, last_x[2] = {-1.0f, -1.0f};
    // Indexed by a loop below: a copy, so the walk's bands stay in registers.
    BandState closed[2] = {bands[0], bands[1]};
    for (int state = 0; state < 2; state++) {
        const StateStream& st = s.states[state];
        BandState& band = closed[state];
        close_hull(s, st, band, rays[state]);
        const HullEntry* points = band.blocked ? st.hull + 1 : &st.best;
        const Moments* prefixes = band.blocked ? st.hull_prefix + 1 : &st.best_prefix;
        int count = band.blocked ? band.depth - 1 : (band.has_best ? 1 : 0);
        before[state] = after[state] = Moments{0.0f, 0.0f, 0.0f};
        if (count > 0) {
            const HullEntry &first = points[0], &last = points[count - 1];
            first_x[state] = first.x;
            last_x[state] = last.x;
            before[state] = state == 1 && first.x == first_x[0] ? before[0] : path_from_source(s, first.x, prefixes[0]);
            after[state] = state == 1 && last.x == last_x[0] ? after[0]
                                                               : path_to_receiver(s, last.x, last.terrain_z, prefixes[count - 1], prefix_behind);
        }
        state_term(s.path, rays[state], points, count, band.blocked, before[state], after[state], &transfer->states[state]);
        transfer->foliage_m[state] = crowns.metres[state];
    }
    transfer->slant_m = fmaxf(hypotf(length, ends.receiver_altitude_m - source.z), 1.0f);
    return 0;
}

// ray::ray_terms from (sx, sy) to the receiver at (rx, ry) at `receiver_altitude_m` standing in
// building `receiver_footprint`, with the obstacles when `obstacles_on_ray`; 0 or a failure. The
// crossings listed, then the one walk.
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
    u32 failed = list_crossings(obstacles, sx, sy, rx, ry, obstacles_on_ray, ends, s);
    if (failed) return failed;
    return walk_ray(ground, sx, sy, ends, s, transfer);
}

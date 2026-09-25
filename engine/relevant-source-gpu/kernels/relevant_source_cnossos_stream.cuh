//! One ray's CNOSSOS-EU transfer on the GPU (noise-compute ray_transfer + ray_path): the terrain
//! samples and the obstacle crossings of the region's one grid stream in chainage order into
//! both states' monotone-chain hulls; every hull entry carries the ground moments of its two
//! sides, so the side planes and ground factors of whichever entries end up first and last
//! come out without storing the ray's roofs. A footprint's crossings pair into roofs as they
//! arrive; a roof starts no earlier than where the roofs left before it end (ray_path.rs).

#pragma once

#include "relevant_source_cnossos_boundary.cuh"

/// Hull entries per state: the 2026-09-24 oracle rays needed at most 21 (homogeneous) and 11
/// (favourable) over 1.1 million rays; a longer hull raises `quietmap_profile_overflow`.
constexpr int QUIETMAP_HULL_CAPACITY = 32;
/// Crossings of one grid cell sorted at a time (cells are at least 32 m; a fuller cell is walked
/// again for the next ones).
constexpr int QUIETMAP_CELL_CROSSING_CAPACITY = 32;
/// Footprints the ray may be inside at once (at most 10 on the oracle rays).
constexpr int QUIETMAP_OPEN_FOOTPRINT_CAPACITY = 16;
/// Accepted crossings remembered to drop a crossing found again across a cell boundary or at a
/// ring vertex.
constexpr int QUIETMAP_RECENT_CROSSING_CAPACITY = 8;
/// Two crossings of one footprint closer than this are one (a ray through a ring vertex hits
/// both edges); f32 chainages of one point agree to well under it on a 12 km ray.
constexpr float QUIETMAP_SAME_CROSSING_M = 0.005f;
/// Slack of a cell's chainage window: the crossing's own f32 error and the cell boundary's.
constexpr float QUIETMAP_CELL_WINDOW_SLACK_M = 0.01f;

struct HullEntry {
    PlanePoint point;
    /// The point lowered by the state's ray height above the chord (homogeneous: its z).
    float hull_z;
    /// Ground over [0, x] relative to (0, z0) and over [x, L] relative to the point itself.
    SideMoments before;
    SideMoments after;
};

struct StateStream {
    HullEntry hull[QUIETMAP_HULL_CAPACITY];
    int depth;
    bool blocked;
    bool has_best;
    float best_delta;
    HullEntry best;
    float radius;
};

/// A stretch of ground, linear on [a, b]: absolute altitude (`absolute`) or a roof's rise over
/// the terrain, and the ground factor (or its change).
struct GroundPiece {
    float a, b;
    float wa, wb;
    float ga, gb;
    bool absolute;
};

struct OpenFootprint {
    uint32_t footprint;
    float x;
    float top;
};

struct RecentCrossing {
    uint32_t edge;
    uint32_t footprint;
    float x;
};

struct CnossosStream {
    StateStream states[2];
    SideMoments whole;
    PlanePoint source;
    PlanePoint receiver;
    float ground_origin_z;
    float length;
    /// Next terrain sample to stream; samples before it are in.
    int next_sample;
    float last_x;
    float covered_to;
    int open_count;
    OpenFootprint open[QUIETMAP_OPEN_FOOTPRINT_CAPACITY];
    int recent_count;
    RecentCrossing recent[QUIETMAP_RECENT_CROSSING_CAPACITY];
};

__device__ __forceinline__ void add_piece(SideMoments& m, const GroundPiece& p, float x_ref,
                                          float z_ref) {
    const float width = p.b - p.a;
    if (width <= 0.0f) {
        return;
    }
    const float wa = p.absolute ? p.wa - z_ref : p.wa;
    const float wb = p.absolute ? p.wb - z_ref : p.wb;
    const float u0 = p.a - x_ref;
    const float u1 = p.b - x_ref;
    m.ground_z = fmaf(0.5f * (wa + wb), width, m.ground_z);
    m.ground_xz = fmaf(width, (u0 * (2.0f * wa + wb) + u1 * (wa + 2.0f * wb)) * (1.0f / 6.0f),
                       m.ground_xz);
    m.ground_factor = fmaf(0.5f * (p.ga + p.gb), width, m.ground_factor);
}

__device__ __forceinline__ GroundPiece piece_part(const GroundPiece& p, float lo, float hi) {
    const float width = p.b - p.a;
    const float f0 = (lo - p.a) / width;
    const float f1 = (hi - p.a) / width;
    GroundPiece part = p;
    part.a = lo;
    part.b = hi;
    part.wa = fmaf(f0, p.wb - p.wa, p.wa);
    part.wb = fmaf(f1, p.wb - p.wa, p.wa);
    part.ga = fmaf(f0, p.gb - p.ga, p.ga);
    part.gb = fmaf(f1, p.gb - p.ga, p.ga);
    return part;
}

__device__ __forceinline__ void route_piece(HullEntry& e, const GroundPiece& p, float z0) {
    if (p.b <= e.point.x) {
        add_piece(e.before, p, 0.0f, z0);
    } else if (p.a >= e.point.x) {
        add_piece(e.after, p, e.point.x, e.point.z);
    } else {
        add_piece(e.before, piece_part(p, p.a, e.point.x), 0.0f, z0);
        add_piece(e.after, piece_part(p, e.point.x, p.b), e.point.x, e.point.z);
    }
}

__device__ void stream_piece(CnossosStream& s, const GroundPiece& p) {
    add_piece(s.whole, p, 0.0f, s.ground_origin_z);
    for (int state = 0; state < 2; ++state) {
        StateStream& st = s.states[state];
        for (int index = 1; index < st.depth; ++index) {
            route_piece(st.hull[index], p, s.ground_origin_z);
        }
        if (st.has_best && !st.blocked) {
            route_piece(st.best, p, s.ground_origin_z);
        }
    }
}

/// rubber_band.rs ray_height_above_chord.
__device__ __forceinline__ float ray_height_above_chord(const CnossosStream& s, int state,
                                                        float x) {
    if (state == QUIETMAP_STATE_HOMOGENEOUS) {
        return 0.0f;
    }
    const float horizontal = s.length;
    const float chord = hypotf(s.receiver.x - s.source.x, s.receiver.z - s.source.z);
    const float along = x * chord / horizontal;
    const float gamma = s.states[state].radius;
    const float offset = along - 0.5f * chord;
    const float perpendicular = along * (chord - along)
        / (sqrtf(fmaxf(gamma * gamma - offset * offset, 0.0f))
           + sqrtf(gamma * gamma - 0.25f * chord * chord));
    return perpendicular * chord / horizontal;
}

__device__ __forceinline__ HullEntry fresh_entry(const CnossosStream& s, PlanePoint point,
                                                 float hull_z) {
    HullEntry e;
    e.point = point;
    e.hull_z = hull_z;
    e.before = s.whole;
    e.after = SideMoments{0.0f, 0.0f, 0.0f};
    return e;
}

/// One diffraction candidate for both states: a blocking one joins the state's hull, otherwise
/// it competes for the unblocked path's single point (rubber_band.rs diffraction_path).
__device__ void stream_candidate(CnossosStream& s, PlanePoint point) {
    const float chord_z = s.source.z + (s.receiver.z - s.source.z) * point.x / s.length;
    for (int state = 0; state < 2; ++state) {
        StateStream& st = s.states[state];
        const float lowered = point.z - ray_height_above_chord(s, state, point.x);
        if (lowered > chord_z) {
            st.blocked = true;
            while (st.depth >= 2) {
                const HullEntry& a = st.hull[st.depth - 2];
                const HullEntry& b = st.hull[st.depth - 1];
                if ((b.point.x - a.point.x) * (lowered - a.hull_z)
                        - (b.hull_z - a.hull_z) * (point.x - a.point.x) >= 0.0f) {
                    --st.depth;
                } else {
                    break;
                }
            }
            if (st.depth < QUIETMAP_HULL_CAPACITY) {
                st.hull[st.depth++] = fresh_entry(s, point, lowered);
            } else {
                atomicOr(&quietmap_profile_overflow, QUIETMAP_OVERFLOW_HULL_POINTS);
            }
        } else if (!st.blocked) {
            const float delta = state_path_difference(state, st.radius, s.source, &point, 1,
                                                      s.receiver);
            if (!st.has_best || delta > st.best_delta) {
                st.has_best = true;
                st.best_delta = delta;
                st.best = fresh_entry(s, point, lowered);
            }
        }
    }
}

__device__ __forceinline__ float profile_x(const PathProfile& profile, int index) {
    return profile.t[index] * profile.distance_m;
}

/// G = (100 − imd)/100, formed from the complement so a sealed sample is exactly 0 (1 − imd·0.01
/// contracts to an FMA that leaves 2·10⁻⁸ and takes the soft-ground branch).
__device__ __forceinline__ float profile_ground_factor(const PathProfile& profile, int index) {
    return static_cast<float>(100 - min(static_cast<int>(profile.imd[index]), 100)) * 0.01f;
}

/// Terrain altitude and ground factor at `x`, linear between the samples around it; the walk
/// starts at sample `hint` (any index; the stream passes its next sample).
__device__ __forceinline__ void terrain_at(const PathProfile& profile, float x, int hint,
                                           float& z, float& g) {
    int upper = max(1, min(hint, profile.count - 1));
    while (upper > 1 && profile_x(profile, upper - 1) > x) {
        --upper;
    }
    while (upper < profile.count - 1 && profile_x(profile, upper) <= x) {
        ++upper;
    }
    const float x0 = profile_x(profile, upper - 1);
    const float x1 = profile_x(profile, upper);
    const float f = x1 > x0 ? quietmap_clamp((x - x0) / (x1 - x0), 0.0f, 1.0f) : 0.0f;
    z = fmaf(f, profile.elevation_m[upper] - profile.elevation_m[upper - 1],
             profile.elevation_m[upper - 1]);
    const float g0 = profile_ground_factor(profile, upper - 1);
    const float g1 = profile_ground_factor(profile, upper);
    g = fmaf(f, g1 - g0, g0);
}

/// Streams terrain samples up to and including chainage `x` (never the receiver's sample).
__device__ void stream_terrain_through(CnossosStream& s, const PathProfile& profile, float x) {
    while (s.next_sample < profile.count - 1 && profile_x(profile, s.next_sample) <= x) {
        const int k = s.next_sample;
        GroundPiece piece;
        piece.a = profile_x(profile, k - 1);
        piece.b = profile_x(profile, k);
        piece.wa = profile.elevation_m[k - 1];
        piece.wb = profile.elevation_m[k];
        piece.ga = profile_ground_factor(profile, k - 1);
        piece.gb = profile_ground_factor(profile, k);
        piece.absolute = true;
        stream_piece(s, piece);
        stream_candidate(s, PlanePoint{piece.b, piece.wb});
        ++s.next_sample;
    }
}

/// A closed roof from the wall at x0 to the wall at x1: clipped to start where the roofs left
/// before it end, then streamed as its rise over the terrain, sample stretch by sample stretch.
__device__ void stream_roof(CnossosStream& s, const PathProfile& profile, float x0, float top0,
                            float x1, float top1) {
    if (x1 <= s.covered_to) {
        return;
    }
    if (x0 < s.covered_to) {
        top0 += (s.covered_to - x0) / (x1 - x0) * (top1 - top0);
        x0 = s.covered_to;
    }
    s.covered_to = x1;
    const float slope = x1 > x0 ? (top1 - top0) / (x1 - x0) : 0.0f;
    float a = x0;
    float terrain_a, ground_a;
    terrain_at(profile, a, s.next_sample, terrain_a, ground_a);
    int k = s.next_sample;
    while (k > 1 && profile_x(profile, k - 1) > a) {
        --k;
    }
    while (a < x1) {
        const float b = k < profile.count - 1 ? fminf(profile_x(profile, k), x1) : x1;
        float terrain_b, ground_b;
        terrain_at(profile, b, k, terrain_b, ground_b);
        GroundPiece piece;
        piece.a = a;
        piece.b = b;
        piece.wa = fmaf(slope, a - x0, top0) - terrain_a;
        piece.wb = fmaf(slope, b - x0, top0) - terrain_b;
        piece.ga = -ground_a;
        piece.gb = -ground_b;
        piece.absolute = false;
        stream_piece(s, piece);
        a = b;
        terrain_a = terrain_b;
        ground_a = ground_b;
        ++k;
    }
}

/// One accepted crossing in chainage order: a top for both hulls, and a wall of its footprint.
__device__ void stream_crossing(CnossosStream& s, const DeviceScenePointers& scene,
                                const PathProfile& profile, uint32_t edge, float x,
                                float exclusion_radius_m) {
    const bool building = scene.obstacle_edge_is_building[edge] != 0u;
    if (building && x < exclusion_radius_m) {
        return;
    }
    stream_terrain_through(s, profile, x);
    float terrain, ground;
    terrain_at(profile, x, s.next_sample, terrain, ground);
    const float top = terrain + scene.obstacle_edge_height_m[edge];
    stream_candidate(s, PlanePoint{x, top});
    if (!building) {
        return;
    }
    const uint32_t footprint = scene.obstacle_edge_footprint_id[edge];
    for (int index = 0; index < s.open_count; ++index) {
        if (s.open[index].footprint == footprint) {
            const OpenFootprint entry = s.open[index];
            s.open[index] = s.open[--s.open_count];
            stream_roof(s, profile, entry.x, entry.top, x, top);
            return;
        }
    }
    if (s.open_count < QUIETMAP_OPEN_FOOTPRINT_CAPACITY) {
        s.open[s.open_count++] = OpenFootprint{footprint, x, top};
    } else {
        atomicOr(&quietmap_profile_overflow, QUIETMAP_OVERFLOW_OPEN_FOOTPRINTS);
    }
}

/// True when this crossing was already taken: the same edge again (found from the next cell),
/// or another edge of the same footprint at the same point (a ring vertex).
__device__ __forceinline__ bool crossing_already_taken(const CnossosStream& s, uint32_t edge,
                                                       uint32_t footprint, float x) {
    for (int index = 0; index < s.recent_count; ++index) {
        const RecentCrossing& r = s.recent[index];
        if (r.edge == edge || (r.footprint == footprint && fabsf(r.x - x) < QUIETMAP_SAME_CROSSING_M)) {
            return true;
        }
    }
    return false;
}

__device__ __forceinline__ void remember_crossing(CnossosStream& s, uint32_t edge,
                                                  uint32_t footprint, float x) {
    const RecentCrossing r = {edge, footprint, x};
    if (s.recent_count < QUIETMAP_RECENT_CROSSING_CAPACITY) {
        s.recent[s.recent_count++] = r;
    } else {
        for (int index = 1; index < QUIETMAP_RECENT_CROSSING_CAPACITY; ++index) {
            s.recent[index - 1] = s.recent[index];
        }
        s.recent[QUIETMAP_RECENT_CROSSING_CAPACITY - 1] = r;
    }
}

/// Walks the region grid along the ray and streams every crossing in chainage order: a cell
/// takes the crossings inside its own chainage window, sorted, so an edge listed in several
/// cells is taken once, where the ray meets it.
__device__ void stream_obstacle_crossings(CnossosStream& s, const DeviceScenePointers& scene,
                                          const PathProfile& profile, float source_x_m,
                                          float source_y_m, float receiver_x_m,
                                          float receiver_y_m, float exclusion_radius_m) {
    if (scene.obstacle_grid_count == 0u) {
        return;
    }
    const DeviceObstacleGrid grid = scene.obstacle_grids[0];
    const float start_x = fmaf(source_x_m, grid.query_x_scale, grid.query_x_offset_m);
    const float start_y = source_y_m + grid.query_y_offset_m;
    const float end_x = fmaf(receiver_x_m, grid.query_x_scale, grid.query_x_offset_m);
    const float end_y = receiver_y_m + grid.query_y_offset_m;
    if (!ray_may_enter_grid(start_x, start_y, end_x, end_y, grid)) {
        return;
    }
    const float dx = end_x - start_x;
    const float dy = end_y - start_y;
    const float inverse_cell = 1.0f / grid.cell_m;
    const int columns = static_cast<int>(grid.columns);
    const int rows = static_cast<int>(grid.rows);
    int cell_x = max(0, min(static_cast<int>(floorf((start_x - grid.minimum_x_m) * inverse_cell)),
                            columns - 1));
    int cell_y = max(0, min(static_cast<int>(floorf((start_y - grid.minimum_y_m) * inverse_cell)),
                            rows - 1));
    const int end_cell_x = max(0, min(
        static_cast<int>(floorf((end_x - grid.minimum_x_m) * inverse_cell)), columns - 1));
    const int end_cell_y = max(0, min(
        static_cast<int>(floorf((end_y - grid.minimum_y_m) * inverse_cell)), rows - 1));
    const int step_x = dx >= 0.0f ? 1 : -1;
    const int step_y = dy >= 0.0f ? 1 : -1;
    const float slack_t = QUIETMAP_CELL_WINDOW_SLACK_M / s.length;
    // Each boundary's chainage from its own grid line, never accumulated along the walk.
    auto boundary_t_x = [&](int column) {
        const float line = grid.minimum_x_m + static_cast<float>(column + (dx >= 0.0f ? 1 : 0))
                                                  * grid.cell_m;
        return dx != 0.0f ? (line - start_x) / dx : CUDART_INF_F;
    };
    auto boundary_t_y = [&](int row) {
        const float line = grid.minimum_y_m + static_cast<float>(row + (dy >= 0.0f ? 1 : 0))
                                                  * grid.cell_m;
        return dy != 0.0f ? (line - start_y) / dy : CUDART_INF_F;
    };
    float entry_t = 0.0f;
    float cell_t[QUIETMAP_CELL_CROSSING_CAPACITY];
    uint32_t cell_edge[QUIETMAP_CELL_CROSSING_CAPACITY];
    int guard = columns + rows + 4;
    while (guard-- > 0) {
        const float next_x = boundary_t_x(cell_x);
        const float next_y = boundary_t_y(cell_y);
        const bool last_cell = cell_x == end_cell_x && cell_y == end_cell_y;
        const float exit_t = last_cell ? 1.0f : fminf(fminf(next_x, next_y), 1.0f);
        const uint32_t cell = static_cast<uint32_t>(cell_y) * grid.columns + cell_x;
        const uint32_t first = scene.obstacle_cell_starts[grid.cell_starts_offset + cell];
        const uint32_t end = scene.obstacle_cell_starts[grid.cell_starts_offset + cell + 1];
        // Crossings in (t, edge) order, at most a buffer at a time: a cell with more in its
        // window is walked again for the next buffer after the last one taken.
        float taken_t = -CUDART_INF_F;
        uint32_t taken_edge = 0u;
        bool more = end > first;
        while (more) {
            more = false;
            int count = 0;
            for (uint32_t position = first; position < end; ++position) {
                const uint32_t edge = grid.edge_index_offset
                    + scene.obstacle_edge_references[grid.edge_references_offset + position];
                const float4 ends = load_obstacle_edge_endpoints(scene, edge);
                float t;
                if (!segment_crossing_fraction(start_x, start_y, dx, dy, ends.x, ends.y, ends.z,
                                               ends.w, t)
                    || t < entry_t - slack_t || t > exit_t + slack_t
                    || t < taken_t || (t == taken_t && edge <= taken_edge)) {
                    continue;
                }
                if (count == QUIETMAP_CELL_CROSSING_CAPACITY) {
                    more = true;
                    const int last = count - 1;
                    if (t > cell_t[last] || (t == cell_t[last] && edge > cell_edge[last])) {
                        continue;
                    }
                    --count;
                }
                int slot = count++;
                while (slot > 0 && (cell_t[slot - 1] > t
                                    || (cell_t[slot - 1] == t && cell_edge[slot - 1] > edge))) {
                    cell_t[slot] = cell_t[slot - 1];
                    cell_edge[slot] = cell_edge[slot - 1];
                    --slot;
                }
                cell_t[slot] = t;
                cell_edge[slot] = edge;
            }
            for (int index = 0; index < count; ++index) {
                const uint32_t edge = cell_edge[index];
                const uint32_t footprint = scene.obstacle_edge_footprint_id[edge];
                const float x = fmaxf(cell_t[index] * s.length, s.last_x);
                if (crossing_already_taken(s, edge, footprint, x)) {
                    continue;
                }
                remember_crossing(s, edge, footprint, x);
                s.last_x = x;
                stream_crossing(s, scene, profile, edge, x, exclusion_radius_m);
            }
            if (count > 0) {
                taken_t = cell_t[count - 1];
                taken_edge = cell_edge[count - 1];
            }
        }
        if (last_cell) {
            break;
        }
        entry_t = exit_t;
        if (next_x < next_y) {
            cell_x += step_x;
        } else {
            cell_y += step_y;
        }
        if (cell_x < 0 || cell_y < 0 || cell_x >= columns || cell_y >= rows) {
            break;
        }
    }
}

/// What one ray's transfer needs to know about its source.
struct RaySourceTerms {
    float height_m;
    /// Negative: sample the ground factor under the source (a point source).
    float ground_factor;
    float platform_half_width_m;
    float exclusion_radius_m;
};

/// Linear transfer 10^(−A/10) per period and band of the ray from (sx, sy) to the receiver:
/// everything but divergence and reflection (ray_transfer.rs evaluate_ray_transfer, full).
__device__ void cnossos_ray_transfer(
    const DeviceScenePointers& scene, const RaySourceTerms& terms, float source_x_m,
    float source_y_m, float receiver_x_m, float receiver_y_m, float receiver_altitude_m,
    bool obstacles_on_ray, PathProfile& profile,
    float transfer[QUIETMAP_PERIOD_COUNT][QUIETMAP_BAND_COUNT]
) {
    const float length = fmaxf(hypotf(receiver_x_m - source_x_m, receiver_y_m - source_y_m), 1.0f);
    build_path_profile(scene, source_x_m, source_y_m, receiver_x_m, receiver_y_m, length, false,
                       profile);
    const float source_ground_m = profile.elevation_m[0];
    const float source_altitude_m = source_ground_m + terms.height_m;
    for (int index = 1; index < profile.count; ++index) {
        if (profile_x(profile, index) < terms.platform_half_width_m) {
            profile.elevation_m[index] = fminf(profile.elevation_m[index], source_ground_m);
        }
    }
    const float source_ground_factor = terms.ground_factor >= 0.0f ? terms.ground_factor
                                                                   : profile_ground_factor(profile, 0);
    CnossosStream s;
    s.source = PlanePoint{0.0f, source_altitude_m};
    s.receiver = PlanePoint{length, receiver_altitude_m};
    s.length = length;
    s.ground_origin_z = source_ground_m;
    s.whole = SideMoments{0.0f, 0.0f, 0.0f};
    s.next_sample = 1;
    s.last_x = 0.0f;
    s.covered_to = -CUDART_INF_F;
    s.open_count = 0;
    s.recent_count = 0;
    const float radius = fmaxf(QUIETMAP_FAVOURABLE_RAY_RADIUS_MINIMUM_M,
                               QUIETMAP_FAVOURABLE_RAY_RADIUS_PER_DISTANCE
                                   * hypotf(length, receiver_altitude_m - source_altitude_m));
    for (int state = 0; state < 2; ++state) {
        StateStream& st = s.states[state];
        st.depth = 1;
        st.hull[0].point = s.source;
        st.hull[0].hull_z = s.source.z;
        st.blocked = false;
        st.has_best = false;
        st.best_delta = 0.0f;
        st.radius = radius;
    }
    if (obstacles_on_ray) {
        stream_obstacle_crossings(s, scene, profile, source_x_m, source_y_m, receiver_x_m,
                                  receiver_y_m, terms.exclusion_radius_m);
    }
    stream_terrain_through(s, profile, length);
    const int last = profile.count - 1;
    stream_piece(s, GroundPiece{profile_x(profile, last - 1), length,
                                profile.elevation_m[last - 1], profile.elevation_m[last],
                                profile_ground_factor(profile, last - 1),
                                profile_ground_factor(profile, last), true});
    float attenuation[2][QUIETMAP_BAND_COUNT];
    for (int state = 0; state < 2; ++state) {
        StateStream& st = s.states[state];
        // The receiver closes the hull.
        while (st.depth >= 2) {
            const HullEntry& a = st.hull[st.depth - 2];
            const HullEntry& b = st.hull[st.depth - 1];
            if ((b.point.x - a.point.x) * (s.receiver.z - a.hull_z)
                    - (b.hull_z - a.hull_z) * (s.receiver.x - a.point.x) >= 0.0f) {
                --st.depth;
            } else {
                break;
            }
        }
        PlanePoint points[QUIETMAP_HULL_CAPACITY];
        StateDiffraction d;
        d.blocked = st.blocked;
        const HullEntry* first_entry = nullptr;
        const HullEntry* last_entry = nullptr;
        d.count = 0;
        if (st.blocked) {
            for (int index = 1; index < st.depth; ++index) {
                points[d.count++] = st.hull[index].point;
            }
            first_entry = &st.hull[1];
            last_entry = &st.hull[st.depth - 1];
        } else if (st.has_best) {
            points[d.count++] = st.best.point;
            first_entry = &st.best;
            last_entry = &st.best;
        }
        d.points = points;
        if (d.count > 0) {
            d.before_first = first_entry->before;
            d.after_last = last_entry->after;
            d.ground_factor_at_source = profile_ground_factor(profile, 0);
            terrain_at(profile, last_entry->point.x, profile.count - 1, d.terrain_z_at_last,
                       d.ground_factor_at_last);
        }
        state_boundary_bands(state, s.source, s.receiver, source_ground_factor, s.whole,
                             s.ground_origin_z, d, attenuation[state]);
    }
    const float slant_km = 0.001f * fmaxf(hypotf(length, receiver_altitude_m - source_altitude_m),
                                          1.0f);
    const float azimuth = atan2f(receiver_y_m - source_y_m, receiver_x_m - source_x_m);
    for (int period = 0; period < QUIETMAP_PERIOD_COUNT; ++period) {
        const float p = favourable_probability(*scene.weather, period, azimuth);
        for (int band = 0; band < QUIETMAP_BAND_COUNT; ++band) {
            const float air_db = fmaxf(
                fmaf(scene.weather->absorption_mean_db_per_km[period][band], slant_km,
                     -0.11512925464970229f
                         * scene.weather->absorption_variance_db2_per_km2[period][band]
                         * slant_km * slant_km),
                scene.weather->absorption_minimum_db_per_km[period][band] * slant_km);
            const float forest_db = fminf(QUIETMAP_VEGETATION_DB_PER_M[band] * profile.forest_depth_m,
                                          QUIETMAP_VEGETATION_CAP_DB[band]);
            transfer[period][band] = quietmap_energy_from_db(-(air_db + forest_db))
                * (p * quietmap_energy_from_db(-attenuation[QUIETMAP_STATE_FAVOURABLE][band])
                   + (1.0f - p) * quietmap_energy_from_db(-attenuation[QUIETMAP_STATE_HOMOGENEOUS][band]));
        }
    }
}

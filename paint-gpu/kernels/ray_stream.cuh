// Per-ray records and recent terrain vertices for the one terrain walk.

// Per-thread capacities; a ray needing more fails with FAILED_CAPACITY and goes to the CPU.
#define HULL_CAPACITY 40     // points of one state's hull (dev4's oracle rays needed at most 21)
#define OPEN_ROOFS 12        // footprints the ray is inside at once
#define ROOFS 512            // roofs one ray passes under
#define RAY_CROSSINGS 1024   // crossings one ray keeps: two a roof, and the walls
#define RECENT_CROSSINGS 8   // crossings kept to drop one found twice
#define CONTAINING 8         // footprints a building's source stands in, its own among them
#define LINE_NODES 64        // quadrature nodes of one line piece (five buckets, each wide one split at its edges)
// Recent terrain vertices: ranges starting among them are summed locally to avoid cancellation.
// Earlier starts use differences of source moments; the window's physical span varies with latitude
// and lattice scale, and an older roof's partial interval can still be short.
#define RECENT_VERTICES 128
// The walk takes the terrain CHUNK_VERTICES vertices at a time, then the crossings and candidates
// up to where its vertices tell them (ray.cuh walk_ray); it keeps its last VERTEX_RING vertices (a
// power of two): the recent ones of the crossings still to be taken and of the chunk's own.
#define CHUNK_VERTICES 64
#define VERTEX_RING 256
// ray::CREST_ENDS_M and CREST_SPACING_M: beyond 300 m of both ends only the highest vertex of each
// 150 m of the ray may diffract.
#define CREST_ENDS_M 300.0f
#define CREST_SPACING_M 150.0f

// ray.rs RayEnds
struct RayEnds {
    float source_height_m;
    float source_ground_m;        // a bridge's deck under the source; NAN: the terrain under it
    float receiver_altitude_m;
    float source_ground_factor;   // negative: the ground under the source
    float platform_half_width_m;
    u64 own_footprint;            // the source's building
};

// A terrain vertex of the ray: distance from the source, altitude (flattened to the source's ground
// within its platform), ground factor.
struct TerrainPoint {
    float x;
    float z;
    float g;
};

// A diffraction candidate (a terrain vertex or an obstacle's top): the point, the terrain under it,
// and the terrain's moments from the source to it about (0, the source's ground).
struct BandCandidate {
    float x;
    float z;
    float terrain_z;
    float terrain_g;
    Moments prefix;
};

// A roof the ray passes under, clipped where the roofs closed before it end: its walls' distances
// and tops, its rise over the terrain about (x0, 0), and the terrain's moments from the source to
// either wall.
struct Roof {
    float x0, top0, x1, top1;
    Moments rise;
    Moments prefix0, prefix1;
};

// One state's hull (rubber_band.rs diffraction_path): the source first, the candidates that stand
// above the state's chord after the monotone chain; while no candidate does, the one nearest to
// blocking. Each point's terrain moments from the source beside it.
struct StateStream {
    HullEntry hull[HULL_CAPACITY];
    Moments hull_prefix[HULL_CAPACITY];
    HullEntry best;
    Moments best_prefix;
};

// What a state's band is while the walk streams, in the walk's registers: its hull's depth, whether a
// candidate has blocked it, and the unblocked path's best path difference so far.
struct BandState {
    int depth;
    bool blocked;
    bool has_best;
    float best_delta;
};

// A footprint the ray is inside: its entry's distance, top and terrain moments from the source, and
// the walk's vertex ending the stretch the entry lies on.
struct OpenRoof {
    u64 footprint_id;
    float x;
    float top;
    Moments prefix;
    int vertex;
};

// A crossing kept, for dropping the same one found again.
struct KeptCrossing {
    u64 footprint_id;
    u64 edge;
    float t;
};

// A crossing the ray keeps: its place along the ray (0 at the source, 1 at the receiver), its
// obstacle, and whether the roofs queued at its place close after it (the last crossing there, kept
// or not).
struct RayCrossing {
    float t;
    float height_m;
    u64 footprint_id;
    int building;
    int closes;
};

// Everything a ray carries while it walks.
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
    StreamedPath path;
    RayCrossing crossings[RAY_CROSSINGS];
    int crossing_count;
    KeptCrossing recent[RECENT_CROSSINGS];   // a ring, the newest at recent_next - 1
    int recent_next;
    int recent_count;
    // Vertex k of the walk and the terrain's moments from the source to it at k % VERTEX_RING;
    // `vertices` kept so far, as the crossing being taken saw them (walk_ray).
    TerrainPoint ring[VERTEX_RING];
    Moments ring_prefix[VERTEX_RING];
    int vertices;
    // The roofs closed so far, in order of distance, where they end and the terrain's moments there.
    Roof roofs[ROOFS];
    int roof_count;
    float covered_to;
    Moments covered_prefix;
    int covered_vertex;
    OpenRoof open[OPEN_ROOFS];
    int open_count;
    // Roofs closed by crossings at one distance, applied together in the order the CPU sorts them.
    OpenRoof closing_from[OPEN_ROOFS];
    float closing_top[OPEN_ROOFS];
    int closing_count;
    float closing_x;
    Moments closing_prefix;
    int closing_vertex;
    // The walk's indices of a chunk's candidate vertices waiting for the crossings before them (the
    // ends' vertices, the middle stretches' crests), in order: one a vertex walked, and a crest
    // carried in. Its bytes a multiple of 16 keep the hulls after it aligned for wide loads (65
    // entries cost a dense city square's rays 7 % on compute capability 7.5, 2026-10-20).
    u16 candidates[2 * CHUNK_VERTICES];
    StateStream states[2];
};

// The tree cover where a ray at `altitude` runs inside the crowns over `ground`, else 0.
__device__ __forceinline__ float cover_inside_crowns(float altitude, const GroundSample& ground) {
    float above = altitude - ground.height_m;
    return above > 0.0f && above <= ground.canopy_m ? ground.forest_cover : 0.0f;
}

// The crowns the states' rays have passed so far: the last vertex's ground and distance from the
// source, and each state's cover-weighted metres inside them (physics/src/foliage.rs depth_m).
struct Crowns {
    GroundSample behind;
    float behind_x;
    float metres[2];
};

// Each state's foliage depth over the stretch from the last vertex to `ground` at `x`: the cover
// where the state's ray runs inside the crowns, linear between the vertices, times the stretch's
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

// Vertex k of the walk, among the last VERTEX_RING.
__device__ __forceinline__ TerrainPoint recent_vertex(const RayStream& s, int k) {
    return s.ring[k & (VERTEX_RING - 1)];
}

// Vertex k's terrain moments from the source.
__device__ __forceinline__ Moments vertex_prefix(const RayStream& s, int k) {
    return s.ring_prefix[k & (VERTEX_RING - 1)];
}

__device__ __forceinline__ void keep_vertex(RayStream& s, const TerrainPoint& vertex, const Moments& prefix) {
    s.ring[s.vertices & (VERTEX_RING - 1)] = vertex;
    s.ring_prefix[s.vertices & (VERTEX_RING - 1)] = prefix;
    s.vertices++;
}

// The kept vertex ending the stretch that holds `x` (the newest beyond it), -1 when that stretch
// starts before the kept vertices.
__device__ int recent_stretch_holding(const RayStream& s, float x) {
    int oldest = max(s.vertices - RECENT_VERTICES, 0);
    if (recent_vertex(s, oldest).x > x) return -1;
    int lo = oldest + 1, hi = s.vertices - 1;
    while (lo < hi) {
        int mid = (lo + hi) / 2;
        if (recent_vertex(s, mid).x <= x) lo = mid + 1;
        else hi = mid;
    }
    return lo;
}

// ray::interpolate on the stretch from `lo` to `hi` at `x` (with its ground factor).
__device__ __forceinline__ void terrain_on(const TerrainPoint& lo, const TerrainPoint& hi, float x, float* z, float* g) {
    float f = hi.x > lo.x ? clampf((x - lo.x) / (hi.x - lo.x), 0.0f, 1.0f) : 0.0f;
    *z = lo.z + f * (hi.z - lo.z);
    *g = lo.g + f * (hi.g - lo.g);
}

// The terrain's moments over [a, b] about (x_r, z_r) from its moments from the source to a and to b
// about (0, the source's ground `ground_z`).
__device__ Moments terrain_between(const Moments& prefix_a, const Moments& prefix_b, float a, float b, float x_r, float z_r,
                                   float ground_z) {
    float z = prefix_b.z - prefix_a.z, lift = ground_z - z_r, ua = a - x_r, ub = b - x_r;
    return Moments{z + lift * (b - a), prefix_b.xz - prefix_a.xz - x_r * z + lift * 0.5f * (ub * ub - ua * ua), prefix_b.g - prefix_a.g};
}

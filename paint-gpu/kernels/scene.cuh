// Obstacle footprints and ordered ray crossings on the shared lattice.

// One obstacles tile of the neighbourhood (popup::obstacles::SceneTile): where its file lies in the
// blob, its counts, its local origin in scene steps, and its cells' tallest outline.
struct ObstacleTile {
    i32 state;
    u32 outline_count;
    u32 vertex_count;
    u32 run_count;
    u32 occupied_count;         // cells with runs: the bitmap's set bits
    u32 square_count;           // footprint squares the file names
    float offset_x;
    float offset_y;
    u64 base;
    u64 cell_max_offset;
};

// popup::obstacles::Scene of a square: the lattice of the square's frame and its tiles. Scene steps
// count from the north-west corner of the cell holding the frame origin (y south), so they stay
// below 2^24 and every stored vertex is exact in f32.
struct ObstacleScene {
    float origin_x;
    float origin_y;
    float steps_per_metre_x;
    float steps_per_metre_y;
    float metres_per_step_x;
    float metres_per_step_y;
    u32 centre_x;
    u32 centre_y;
    i64 origin_cell_in_centre_x;
    i64 origin_cell_in_centre_y;
    i64 radius;
    const ObstacleTile* tiles;
    const u8* blob;
    const float* cell_max_height;
};

__device__ __forceinline__ u32 read_u32(const u8* bytes) {
    return *(const u32*)bytes;  // files start 8-byte aligned in the blob; every field here is 4-aligned
}
__device__ __forceinline__ u16 read_u16(const u8* bytes) {
    return *(const u16*)bytes;  // every u16 field accessed here is 2-byte aligned in the blob
}

struct OutlineMeta {
    u32 first_vertex;
    u16 height_dm;
    int kind;
};

// tiles::obstacles (qmobs2): the occupied-cell bitmap, the occupied cells before each of its
// words, the first run of each occupied cell, the footprint squares, then runs, outlines, vertices.
__device__ __forceinline__ const u8* tile_cell_words(const ObstacleScene& scene, const ObstacleTile& tile) {
    return scene.blob + tile.base + OBSTACLES_HEADER_BYTES;
}
__device__ __forceinline__ const u8* tile_first_runs(const ObstacleScene& scene, const ObstacleTile& tile) {
    return tile_cell_words(scene, tile) + 10 * CELL_WORDS;
}
__device__ __forceinline__ const u8* tile_squares(const ObstacleScene& scene, const ObstacleTile& tile) {
    return tile_first_runs(scene, tile) + 4 * ((u64)tile.occupied_count + 1);
}
__device__ __forceinline__ const u8* tile_runs(const ObstacleScene& scene, const ObstacleTile& tile) {
    return tile_squares(scene, tile) + 4 * (u64)tile.square_count;
}
// Obstacles::cell_run_range: the runs of cell `index` (row * 128 + column), none for an empty cell.
__device__ __forceinline__ void tile_cell_runs(const ObstacleScene& scene, const ObstacleTile& tile, int index, u32* begin, u32* end) {
    const u8* words = tile_cell_words(scene, tile);
    u64 word = *(const u64*)(words + 8 * (index >> 6));  // the bitmap starts 8-aligned in the blob
    u64 bit = 1ull << (index & 63);
    if (!(word & bit)) {
        *begin = *end = 0;
        return;
    }
    u32 rank = read_u16(words + 8 * CELL_WORDS + 2 * (index >> 6)) + (u32)__popcll(word & (bit - 1));
    const u8* first_runs = tile_first_runs(scene, tile);
    *begin = read_u32(first_runs + 4 * rank);
    *end = read_u32(first_runs + 4 * (rank + 1));
}
__device__ __forceinline__ const u8* tile_outlines(const ObstacleScene& scene, const ObstacleTile& tile) {
    return tile_runs(scene, tile) + (u64)RUN_BYTES * tile.run_count;
}
__device__ __forceinline__ const u8* tile_vertices(const ObstacleScene& scene, const ObstacleTile& tile) {
    return tile_outlines(scene, tile) + (u64)OUTLINE_BYTES * tile.outline_count;
}

__device__ __forceinline__ u32 tile_outline_first_vertex(const ObstacleScene& scene, const ObstacleTile& tile, u32 index) {
    return read_u32(tile_outlines(scene, tile) + (u64)OUTLINE_BYTES * index);
}

__device__ __forceinline__ u64 tile_outline_footprint_id(const ObstacleScene& scene, const ObstacleTile& tile, u32 index) {
    const u8* record = tile_outlines(scene, tile) + (u64)OUTLINE_BYTES * index;
    u64 square = read_u32(tile_squares(scene, tile) + 4 * (u32)record[10]);
    return (square << 32) | (u64)read_u32(record + 4);
}

// An outline's geometry metadata without reading tile_squares for footprint_id.
__device__ __forceinline__ OutlineMeta tile_outline_meta(const ObstacleScene& scene, const ObstacleTile& tile, u32 index) {
    const u32* record = (const u32*)(tile_outlines(scene, tile) + (u64)OUTLINE_BYTES * index);
    u32 tail = record[2];
    return OutlineMeta{record[0], (u16)(tail & 0xffffu), (int)((tail >> 24) & 3u)};
}

// A stored vertex: tile-local int16 steps.
__device__ __forceinline__ void tile_vertex_local(const ObstacleScene& scene, const ObstacleTile& tile, u32 index, int* x, int* y) {
    u32 v = read_u32(tile_vertices(scene, tile) + (u64)VERTEX_BYTES * index);
    *x = (i16)(v & 0xffff);
    *y = (i16)(v >> 16);
}

// obstacles::Lattice::steps
__device__ __forceinline__ void lattice_steps(const ObstacleScene& scene, float east_m, float north_m, float* sx, float* sy) {
    *sx = scene.origin_x + east_m * scene.steps_per_metre_x;
    *sy = scene.origin_y - north_m * scene.steps_per_metre_y;
}

// obstacles::Scene::locate: the state of the tile holding a scene cell; `tile` and `index` are set
// for a read tile. CELLS_PER_SIDE is 128, so Euclidean div/rem are arithmetic shift and mask.
static_assert(CELLS_PER_SIDE == 128, "scene_locate uses power-of-two shift/mask");
__device__ __forceinline__ int scene_locate(const ObstacleScene& scene, i64 cell_x, i64 cell_y, const ObstacleTile** tile, int* index) {
    i64 column = scene.origin_cell_in_centre_x + cell_x;
    i64 row = scene.origin_cell_in_centre_y + cell_y;
    i64 offset_x = column >> 7;
    i64 offset_y = row >> 7;
    i64 y = (i64)scene.centre_y + offset_y;
    if (y < 0 || y >= TILES_PER_AXIS) return TILE_EMPTY;
    if (offset_x < -scene.radius || offset_x > scene.radius || offset_y < -scene.radius || offset_y > scene.radius) return TILE_NOT_READ;
    const ObstacleTile& slot = scene.tiles[(offset_y + scene.radius) * (2 * scene.radius + 1) + offset_x + scene.radius];
    if (slot.state == TILE_READ) {
        *tile = &slot;
        *index = (int)(((row & 127) << 7) | (column & 127));
    }
    return slot.state;
}

// A crossing this near a ray's end is the end (walk_crossings).
#define ENDPOINT_TOLERANCE_M 0.001f

// The f32 filters' error bound from the summed magnitudes of their rounded products. A filtered value
// is p - q of two f32 products whose factors are f32 inputs or their differences, each difference,
// product and subtraction rounded once in this order (the _rn intrinsics are neither contracted nor
// regrouped): it lies within 4u (|p| + |q|) of its exact value (u = 2^-24; Shewchuk 1997, orient2d's
// bound A, and the subtraction), the f64 tests' value within 1e-15 (|p| + |q|), and a rejecting
// comparison needs less than 5u of the summed magnitudes: 16u leaves room for the bound's own
// rounding. 2^-90 covers fast math's flushed subnormals while every factor stays below 2^25, as the
// scene's steps do (ObstacleScene).
__device__ __forceinline__ float filter_bound(float magnitude) {
    return magnitude * 0x1p-20f + 0x1p-90f;
}

// Whether segment_intersection_t below surely finds no crossing within [t_lo, t_hi], decided in f32
// (consumer cards run f64 at 1/32 to 1/64 of f32): the ray's line misses the edge (u outside [0, 1])
// or the edge's line misses the cell's ray stretch (t outside [t_lo, t_hi]) by more than the bound,
// which holds the f64 test's slack. Every other case goes to the f64 test unchanged.
__device__ __forceinline__ bool segment_surely_missed(float sx, float sy, float dx, float dy, float ax, float ay, float bx, float by,
                                                      float t_lo, float t_hi) {
    float ex = __fsub_rn(bx, ax), ey = __fsub_rn(by, ay), wx = __fsub_rn(ax, sx), wy = __fsub_rn(ay, sy);
    float d1 = __fmul_rn(dx, ey), d2 = __fmul_rn(dy, ex);
    float t1 = __fmul_rn(wx, ey), t2 = __fmul_rn(wy, ex);
    float u1 = __fmul_rn(wx, dy), u2 = __fmul_rn(wy, dx);
    float denominator = d1 - d2;
    float bound = filter_bound((fabsf(d1) + fabsf(d2)) + (fabsf(t1) + fabsf(t2)) + (fabsf(u1) + fabsf(u2)));
    float abs_d = fabsf(denominator);
    float sign = denominator > 0.0f ? 1.0f : -1.0f;
    float t_numerator = sign * (t1 - t2), u_numerator = sign * (u1 - u2);
    float t_min = t_lo * abs_d - bound - abs_d * 0x1p-20f;
    float t_max = t_hi * abs_d + bound + abs_d * 0x1p-20f;
    return abs_d > bound && (t_numerator < t_min || t_numerator > t_max || u_numerator < -bound || u_numerator > abs_d + bound);
}

// crossings::segment_intersection_t; false for no crossing. Also sets `*u_interior` when the hit
// lies strictly inside the edge (both endpoints strictly off the ray line, proving `a != 0 && b != 0`
// for `edge_bounds_roof`).
__device__ __forceinline__ bool segment_intersection_t(double sx, double sy, double dx, double dy, double ax, double ay, double bx, double by,
                                                       double* t_out, bool* u_interior) {
    double ex = bx - ax, ey = by - ay;
    double denominator = dx * ey - dy * ex;
    if (denominator == 0.0) return false;
    double wx = ax - sx, wy = ay - sy;
    double t_numerator = wx * ey - wy * ex;
    double u_numerator = wx * dy - wy * dx;
    double sign = denominator > 0.0 ? 1.0 : -1.0;
    double extent = fabs(denominator);
    // Reject clear misses before division. Eight double epsilons include rounding at u = 0 or 1.
    double slack = extent * 1.7763568394002505e-15;
    double st = sign * t_numerator;
    double su = sign * u_numerator;
    if (st < -slack || st > extent + slack || su < -slack || su > extent + slack) return false;
    // Exact IEEE double division preserves exact crossing-t ties when two overlapping roofs share
    // a common exit edge with different edge lengths (different denominators).
    double t = t_numerator / denominator;
    // Interior edge hits need no u division and prove both endpoints lie strictly off the ray line.
    bool interior = su > slack && su < extent - slack;
    bool within_edge = interior;
    if (!within_edge) {
        double u = u_numerator / denominator;
        within_edge = u >= 0.0 && u <= 1.0;
    }
    if (t > 0.0 && t < 1.0 && within_edge) {
        *t_out = t;
        *u_interior = interior;
        return true;
    }
    return false;
}

// One crossing of the ray with a building ring or a wall (ray.rs Crossing), and the edge it was
// found on (the tile slot and the edge's first vertex): the order of crossings at one t. `tie`: the
// next crossing handed on has the same t.
struct Crossing {
    double t;
    u16 height_dm;
    u64 footprint_id;
    u64 edge;
    int bounds_roof;
    int tie;
};

// Compact cell-local hit kept while sorting a cell's crossings: `slot` is constant across the
// cell, and `height_dm` and `edge_bounds_roof` are resolved only for the hits emitted.
struct CellHit {
    double t;
    u64 footprint_id;
    u32 edge;
    u32 outline_index;
};

__device__ __forceinline__ bool cell_hit_before(const CellHit& a, const CellHit& b) {
    if (a.t != b.t) return a.t < b.t;
    if (a.footprint_id != b.footprint_id) return a.footprint_id < b.footprint_id;
    return a.edge < b.edge;
}

// The sign of crossings::edge_bounds_roof's side of a vertex: from f32 where the filter's bound
// proves it (the vertex offset + int is exact in f32 below 2^24), else from the f64 value. Its uses,
// zero tests and the sign of a product of two sides, read the same from signs (a NaN ray has no
// hit, so no NaN side comes here).
__device__ int vertex_side_sign(const ObstacleScene& scene, const ObstacleTile& tile, u32 vertex,
                                float sx, float sy, float dx, float dy) {
    int x, y;
    tile_vertex_local(scene, tile, vertex, &x, &y);
    float vx = __fadd_rn(tile.offset_x, (float)x), vy = __fadd_rn(tile.offset_y, (float)y);
    if (fabsf(vx) < 0x1p24f && fabsf(vy) < 0x1p24f) {
        float p = __fmul_rn(dx, __fsub_rn(vy, sy)), q = __fmul_rn(dy, __fsub_rn(vx, sx));
        float side = p - q, bound = filter_bound(fabsf(p) + fabsf(q));
        if (side > bound) return 1;
        if (side < -bound) return -1;
    }
    double side = (double)dx * (tile.offset_y + (double)y - sy) - (double)dy * (tile.offset_x + (double)x - sx);
    return (side > 0.0) - (side < 0.0);
}

// crossings::edge_bounds_roof: a tangent vertex screens as a top but does not enter its ring.
__device__ bool edge_bounds_roof(const ObstacleScene& scene, const ObstacleTile& tile, u32 outline_index,
                                 const OutlineMeta& outline, u32 edge, float sx, float sy, float dx, float dy) {
    if (outline.kind == OUTLINE_KIND_WALL) return false;
    int a = vertex_side_sign(scene, tile, edge, sx, sy, dx, dy);
    int b = vertex_side_sign(scene, tile, edge + 1, sx, sy, dx, dy);
    if (a != 0 && b != 0) return true;
    u32 end = outline_index + 1 < tile.outline_count
        ? tile_outline_first_vertex(scene, tile, outline_index + 1) : tile.vertex_count;
    u32 distinct = end - outline.first_vertex - 1;
    u32 vertex = ((a == 0 ? edge : edge + 1) - outline.first_vertex) % distinct;
    int before = 0, after = 0;
    for (u32 offset = 1; offset < distinct; offset++) {
        if (before == 0) before = vertex_side_sign(scene, tile, outline.first_vertex + (vertex + distinct - offset) % distinct, sx, sy, dx, dy);
        if (after == 0) after = vertex_side_sign(scene, tile, outline.first_vertex + (vertex + offset) % distinct, sx, sy, dx, dy);
        if (before != 0 && after != 0) break;
    }
    return before * after < 0;
}

// Crossings of one cell sorted at a time; a cell with more inside its stretch of the ray is walked
// again for the next ones.
#define CELL_CROSSINGS 16
// A cell takes the crossings within this fraction of the ray beyond its own stretch: an edge found
// from two cells at its crossing is dropped again by the stream (f32's t of one point agrees to a
// few 1e-7).
#define CELL_WINDOW_SLACK 1e-6f
// The edges a cell's f32 filter keeps on a lane before they take their exact test (walk_crossings).
#define KEPT_EDGES 16

// obstacles::Scene::crossings, streamed: every crossing of the segment from -> to (frame metres)
// with a building ring or wall, handed to `take` in order of (t, footprint, edge); `take` returns 0
// or a failure, which ends the walk. A cell takes only the crossings inside its own stretch of the
// ray, so an edge listed in several cells is taken where the ray meets it. The cell walk takes each
// boundary's t from its own division, not a running sum, so it stays exact in f32.
template <typename Take>
__device__ u32 walk_crossings(const ObstacleScene& scene, float from_x, float from_y, float to_x, float to_y, Take& take) {
    float sx, sy, ex, ey;
    lattice_steps(scene, from_x, from_y, &sx, &sy);
    lattice_steps(scene, to_x, to_y, &ex, &ey);
    float delta[2] = {ex - sx, ey - sy};
    float start[2] = {sx, sy};
    const float cell_steps = (float)CELL_STEPS;
    int cell[2] = {(int)floorf(sx * (1.0f / cell_steps)), (int)floorf(sy * (1.0f / cell_steps))};
    int last[2] = {(int)floorf(ex * (1.0f / cell_steps)), (int)floorf(ey * (1.0f / cell_steps))};
    int step[2] = {delta[0] >= 0.0f ? 1 : -1, delta[1] >= 0.0f ? 1 : -1};
    int bound_off[2] = {delta[0] >= 0.0f ? 1 : 0, delta[1] >= 0.0f ? 1 : 0};
    float pad = 1e-5f * (1.0f + fabsf(delta[0]) + fabsf(delta[1]));
    // crossings::segment_intersection_t leaves out the ray's ends (t in (0, 1), in f64). In f32 an
    // end standing on a wall (a pixel on a facade line) rounds up to millimetres to either side,
    // and a crossing found there would screen every ray to that point: a crossing within
    // ENDPOINT_TOLERANCE_M of either end is the end.
    float length_m = hypotf(delta[0] * scene.metres_per_step_x, delta[1] * scene.metres_per_step_y);
    float endpoint_t = length_m > 0.0f ? ENDPOINT_TOLERANCE_M / length_m : 0.0f;
    float t_enter = 0.0f;
    CellHit hits[CELL_CROSSINGS];
    u8 order[CELL_CROSSINGS];
    int orig_cx = (int)scene.origin_cell_in_centre_x, orig_cy = (int)scene.origin_cell_in_centre_y;
    int cached_ox = 0x7fffffff, cached_oy = 0x7fffffff, cached_state = TILE_EMPTY;
    const ObstacleTile* cached_tile = 0;
    while (true) {
        float t_max[2];
        for (int axis = 0; axis < 2; axis++) {
            if (delta[axis] == 0.0f) {
                t_max[axis] = QM_INFINITY;
            } else {
                float boundary = (float)(cell[axis] + bound_off[axis]) * cell_steps;
                t_max[axis] = fabsf((boundary - start[axis]) / delta[axis]);
            }
        }
        bool last_cell = cell[0] == last[0] && cell[1] == last[1];
        float t_exit = last_cell ? 1.0f : fminf(fminf(t_max[0], t_max[1]), 1.0f);
        int col = orig_cx + cell[0], row = orig_cy + cell[1];
        int ox = col >> 7, oy = row >> 7;
        if (ox != cached_ox || oy != cached_oy) {
            cached_ox = ox;
            cached_oy = oy;
            int y = (int)scene.centre_y + oy;
            int rad = (int)scene.radius;
            if (y < 0 || y >= TILES_PER_AXIS) {
                cached_state = TILE_EMPTY;
                cached_tile = 0;
            } else if (ox < -rad || ox > rad || oy < -rad || oy > rad) {
                cached_state = TILE_NOT_READ;
                cached_tile = 0;
            } else {
                cached_tile = &scene.tiles[(oy + rad) * (2 * rad + 1) + ox + rad];
                cached_state = cached_tile->state;
            }
        }
        if (cached_state == TILE_NOT_READ) return FAILED_NOT_READ;
        const ObstacleTile* tile = cached_tile;
        int index = ((row & 127) << 7) | (col & 127);
        int state = cached_state;
        if (state == TILE_READ) {
            u32 run_begin, run_end;
            tile_cell_runs(scene, *tile, index, &run_begin, &run_end);
            if (run_begin < run_end) {
                float t0 = clampf(t_enter, 0.0f, 1.0f), t1 = clampf(t_exit, 0.0f, 1.0f);
                float t_lo_f = fmaxf(fmaxf(t_enter - CELL_WINDOW_SLACK, endpoint_t) - 1e-6f, 0.0f);
                float t_hi_f = fminf(fminf(t_exit + CELL_WINDOW_SLACK, 1.0f - endpoint_t) + 1e-6f, 1.0f);
                float p[2] = {start[0] + delta[0] * t0, start[1] + delta[1] * t0};
                float q[2] = {start[0] + delta[0] * t1, start[1] + delta[1] * t1};
                int low[2], high[2];
                float offset[2] = {tile->offset_x, tile->offset_y};
                for (int axis = 0; axis < 2; axis++) {
                    low[axis] = (int)floorf(fminf(p[axis], q[axis]) - pad - offset[axis]);
                    high[axis] = (int)ceilf(fmaxf(p[axis], q[axis]) + pad - offset[axis]);
                }
                u64 slot = (u64)(tile - scene.tiles);
                const u8* runs = tile_runs(scene, *tile);
                bool have_taken = false;
                CellHit taken;
                bool more = true;
                while (more) {
                    more = false;
                    int count = 0;
                    double next_t = 2.0;
                    // The cell's runs and their edges in one loop through the f32 filter, then the edges
                    // it kept, KEPT_EDGES at a time, through the exact test and index-sorted insertion;
                    // `edge_bounds_roof` runs only on emitted non-interior crossings.
                    u32 run_index = run_begin, next = 1, last_edge = 0, outline_index = 0;
                    int ax = 0, ay = 0;
                    while (run_index < run_end || next <= last_edge) {
                        u32 kept_edge[KEPT_EDGES], kept_outline[KEPT_EDGES];
                        int kept = 0;
                        while (kept < KEPT_EDGES) {
                            if (next > last_edge) {
                                if (run_index == run_end) break;
                                const u32* run = (const u32*)(runs + (u64)RUN_BYTES * run_index++);
                                outline_index = run[0];
                                u32 span_packed = run[1];
                                u32 first = tile_outline_first_vertex(scene, *tile, outline_index) + (span_packed & 0xffffu);
                                tile_vertex_local(scene, *tile, first, &ax, &ay);
                                next = first + 1;
                                last_edge = first + (span_packed >> 16);
                                if (next > last_edge) continue;
                            }
                            int bx, by;
                            tile_vertex_local(scene, *tile, next, &bx, &by);
                            bool meets = high[0] >= min(ax, bx) && max(ax, bx) >= low[0] && high[1] >= min(ay, by) && max(ay, by) >= low[1];
                            if (meets
                                && !segment_surely_missed(start[0], start[1], delta[0], delta[1], offset[0] + (float)ax, offset[1] + (float)ay,
                                                          offset[0] + (float)bx, offset[1] + (float)by, t_lo_f, t_hi_f)) {
                                kept_edge[kept] = next - 1;
                                kept_outline[kept] = outline_index;
                                kept++;
                            }
                            ax = bx;
                            ay = by;
                            next++;
                        }
                        for (int k = 0; k < kept; k++) {
                            u32 edge = kept_edge[k];
                            int ex, ey, fx, fy;
                            tile_vertex_local(scene, *tile, edge, &ex, &ey);
                            tile_vertex_local(scene, *tile, edge + 1, &fx, &fy);
                            float a[2] = {offset[0] + (float)ex, offset[1] + (float)ey};
                            float b[2] = {offset[0] + (float)fx, offset[1] + (float)fy};
                            double t;
                            bool u_interior;
                            if (!segment_intersection_t(start[0], start[1], delta[0], delta[1], a[0], a[1], b[0], b[1], &t, &u_interior)
                                || t < t_enter - CELL_WINDOW_SLACK || t > t_exit + CELL_WINDOW_SLACK
                                || !(t > endpoint_t && t < 1.0f - endpoint_t)) continue;
                            CellHit hit;
                            hit.t = t;
                            hit.footprint_id = tile_outline_footprint_id(scene, *tile, kept_outline[k]);
                            hit.edge = edge;
                            hit.outline_index = kept_outline[k] | (u_interior ? 0x80000000u : 0u);
                            if (have_taken && !cell_hit_before(taken, hit)) continue;
                            // Insert in order via byte indices, keeping the first CELL_CROSSINGS.
                            u8 entry;
                            int at = count;
                            if (count == CELL_CROSSINGS) {
                                more = true;
                                u8 last_entry = order[count - 1];
                                if (!cell_hit_before(hit, hits[last_entry])) {
                                    if (hit.t < next_t) next_t = hit.t;
                                    continue;
                                }
                                if (hits[last_entry].t < next_t) next_t = hits[last_entry].t;
                                entry = last_entry;
                                at = --count;
                            } else {
                                entry = (u8)count;
                            }
                            hits[entry] = hit;
                            while (at > 0 && cell_hit_before(hit, hits[order[at - 1]])) {
                                order[at] = order[at - 1];
                                at--;
                            }
                            order[at] = entry;
                            count++;
                        }
                    }
                    for (int k = 0; k < count; k++) {
                        const CellHit& h = hits[order[k]];
                        u32 outline_idx = h.outline_index & 0x7fffffffu;
                        bool u_interior = (h.outline_index & 0x80000000u) != 0u;
                        OutlineMeta outline = tile_outline_meta(scene, *tile, outline_idx);
                        Crossing crossing;
                        crossing.t = h.t;
                        crossing.height_dm = outline.height_dm;
                        crossing.footprint_id = h.footprint_id;
                        crossing.edge = (slot << 32) | (u64)h.edge;
                        crossing.bounds_roof = outline.kind == OUTLINE_KIND_WALL
                            ? 0
                            : (u_interior ? 1
                                          : edge_bounds_roof(scene, *tile, outline_idx, outline, h.edge, start[0], start[1],
                                                             delta[0], delta[1]));
                        crossing.tie = (k + 1 < count ? hits[order[k + 1]].t : next_t) == h.t;
                        u32 failed = take(crossing);
                        if (failed) return failed;
                    }
                    if (count > 0) {
                        taken = hits[order[count - 1]];
                        have_taken = true;
                    }
                }
            }
        }
        if (last_cell) break;
        t_enter = t_exit;
        // The axis chosen by a branch, not as an index: an array indexed at run time lives in local
        // memory.
        if (cell[0] != last[0] && (cell[1] == last[1] || t_max[0] < t_max[1])) cell[0] += step[0];
        else cell[1] += step[1];
    }
    return 0;
}

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
    return (u16)(bytes[0] | (bytes[1] << 8));
}

struct Outline {
    u64 footprint_id;
    u32 first_vertex;
    float height_m;
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

__device__ Outline tile_outline(const ObstacleScene& scene, const ObstacleTile& tile, u32 index) {
    const u8* record = tile_outlines(scene, tile) + (u64)OUTLINE_BYTES * index;
    Outline outline;
    u64 square = read_u32(tile_squares(scene, tile) + 4 * (u32)record[10]);
    outline.footprint_id = (square << 32) | (u64)read_u32(record + 4);
    outline.first_vertex = read_u32(record);
    outline.height_m = (float)read_u16(record + 8) * 0.1f;
    outline.kind = record[11] & 3;
    return outline;
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
// for a read tile.
__device__ int scene_locate(const ObstacleScene& scene, i64 cell_x, i64 cell_y, const ObstacleTile** tile, int* index) {
    i64 column = scene.origin_cell_in_centre_x + cell_x;
    i64 row = scene.origin_cell_in_centre_y + cell_y;
    i64 offset_x = div_euclid_i64(column, CELLS_PER_SIDE);
    i64 offset_y = div_euclid_i64(row, CELLS_PER_SIDE);
    i64 y = (i64)scene.centre_y + offset_y;
    if (y < 0 || y >= TILES_PER_AXIS) return TILE_EMPTY;
    if (offset_x < -scene.radius || offset_x > scene.radius || offset_y < -scene.radius || offset_y > scene.radius) return TILE_NOT_READ;
    const ObstacleTile& slot = scene.tiles[(offset_y + scene.radius) * (2 * scene.radius + 1) + offset_x + scene.radius];
    if (slot.state == TILE_READ) {
        *tile = &slot;
        *index = (int)(rem_euclid_i64(row, CELLS_PER_SIDE) * CELLS_PER_SIDE + rem_euclid_i64(column, CELLS_PER_SIDE));
    }
    return slot.state;
}

// A crossing this near a ray's end is the end (walk_crossings).
#define ENDPOINT_TOLERANCE_M 0.001f

// crossings::segment_intersection_t; false for no crossing.
__device__ __forceinline__ bool segment_intersection_t(double sx, double sy, double dx, double dy, double ax, double ay, double bx, double by, double* t_out) {
    double ex = bx - ax, ey = by - ay;
    double denominator = dx * ey - dy * ex;
    if (denominator == 0.0f) return false;
    double wx = ax - sx, wy = ay - sy;
    double t_numerator = wx * ey - wy * ex;
    double u_numerator = wx * dy - wy * dx;
    double sign = denominator > 0.0 ? 1.0 : -1.0;
    double extent = fabs(denominator);
    // Reject clear misses before two slow divisions. Eight double epsilons include rounding
    // at u = 0 or 1; possible hits keep the original t division and endpoint comparisons.
    double slack = extent * 1.7763568394002505e-15;
    if (sign * t_numerator < -slack || sign * t_numerator > extent + slack
        || sign * u_numerator < -slack || sign * u_numerator > extent + slack) return false;
    double t = t_numerator / denominator;
    // Interior edge hits need no u division; at its endpoints retain the original rounding.
    bool within_edge = sign * u_numerator > slack && sign * u_numerator < extent - slack;
    if (!within_edge) {
        double u = u_numerator / denominator;
        within_edge = u >= 0.0 && u <= 1.0;
    }
    if (t > 0.0f && t < 1.0f && within_edge) {
        *t_out = t;
        return true;
    }
    return false;
}

// One crossing of the ray with a building ring or a wall (ray.rs Crossing), and the edge it was
// found on (the tile slot and the edge's first vertex): the order of crossings at one t. `tie`: the
// next crossing handed on has the same t.
struct Crossing {
    double t;
    float height_m;
    u64 footprint_id;
    u64 edge;
    int bounds_roof;
    int tie;
};

// crossings::edge_bounds_roof: a tangent vertex screens as a top but does not enter its ring.
__device__ double vertex_side(const ObstacleScene& scene, const ObstacleTile& tile, u32 vertex,
                              double sx, double sy, double dx, double dy) {
    int x, y;
    tile_vertex_local(scene, tile, vertex, &x, &y);
    return dx * (tile.offset_y + (double)y - sy) - dy * (tile.offset_x + (double)x - sx);
}

__device__ bool edge_bounds_roof(const ObstacleScene& scene, const ObstacleTile& tile, u32 outline_index,
                                 const Outline& outline, u32 edge, double sx, double sy, double dx, double dy) {
    if (outline.kind == OUTLINE_KIND_WALL) return false;
    double a = vertex_side(scene, tile, edge, sx, sy, dx, dy);
    double b = vertex_side(scene, tile, edge + 1, sx, sy, dx, dy);
    if (a != 0.0 && b != 0.0) return true;
    u32 end = outline_index + 1 < tile.outline_count
        ? tile_outline(scene, tile, outline_index + 1).first_vertex : tile.vertex_count;
    u32 distinct = end - outline.first_vertex - 1;
    u32 vertex = ((a == 0.0 ? edge : edge + 1) - outline.first_vertex) % distinct;
    double before = 0.0, after = 0.0;
    for (u32 offset = 1; offset < distinct; offset++) {
        if (before == 0.0) before = vertex_side(scene, tile, outline.first_vertex + (vertex + distinct - offset) % distinct, sx, sy, dx, dy);
        if (after == 0.0) after = vertex_side(scene, tile, outline.first_vertex + (vertex + offset) % distinct, sx, sy, dx, dy);
        if (before != 0.0 && after != 0.0) break;
    }
    return before * after < 0.0;
}

__device__ __forceinline__ bool crossing_before(const Crossing& a, const Crossing& b) {
    if (a.t != b.t) return a.t < b.t;
    if (a.footprint_id != b.footprint_id) return a.footprint_id < b.footprint_id;
    return a.edge < b.edge;
}

// Crossings of one cell sorted at a time; a cell with more inside its stretch of the ray is walked
// again for the next ones.
#define CELL_CROSSINGS 16
// A cell takes the crossings within this fraction of the ray beyond its own stretch: an edge found
// from two cells at its crossing is dropped again by the stream (f32's t of one point agrees to a
// few 1e-7).
#define CELL_WINDOW_SLACK 1e-6f

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
    i64 cell[2] = {(i64)floorf(sx / cell_steps), (i64)floorf(sy / cell_steps)};
    i64 last[2] = {(i64)floorf(ex / cell_steps), (i64)floorf(ey / cell_steps)};
    i64 step[2] = {delta[0] >= 0.0f ? 1 : -1, delta[1] >= 0.0f ? 1 : -1};
    float pad = 1e-5f * (1.0f + fabsf(delta[0]) + fabsf(delta[1]));
    // crossings::segment_intersection_t leaves out the ray's ends (t in (0, 1), in f64). In f32 an
    // end standing on a wall (a pixel on a facade line) rounds up to millimetres to either side,
    // and a crossing found there would screen every ray to that point: a crossing within
    // ENDPOINT_TOLERANCE_M of either end is the end.
    float length_m = hypotf(delta[0] * scene.metres_per_step_x, delta[1] * scene.metres_per_step_y);
    float endpoint_t = length_m > 0.0f ? ENDPOINT_TOLERANCE_M / length_m : 0.0f;
    float t_enter = 0.0f;
    Crossing found[CELL_CROSSINGS];
    while (true) {
        float t_max[2];
        for (int axis = 0; axis < 2; axis++) {
            if (delta[axis] == 0.0f) {
                t_max[axis] = QM_INFINITY;
            } else {
                float boundary = (float)(cell[axis] + (delta[axis] >= 0.0f ? 1 : 0)) * cell_steps;
                t_max[axis] = fabsf((boundary - start[axis]) / delta[axis]);
            }
        }
        bool last_cell = cell[0] == last[0] && cell[1] == last[1];
        float t_exit = last_cell ? 1.0f : fminf(fminf(t_max[0], t_max[1]), 1.0f);
        const ObstacleTile* tile = 0;
        int index = 0;
        int state = scene_locate(scene, cell[0], cell[1], &tile, &index);
        if (state == TILE_NOT_READ) return FAILED_NOT_READ;
        if (state == TILE_READ) {
            u32 run_begin, run_end;
            tile_cell_runs(scene, *tile, index, &run_begin, &run_end);
            if (run_begin < run_end) {
                float t0 = clampf(t_enter, 0.0f, 1.0f), t1 = clampf(t_exit, 0.0f, 1.0f);
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
                Crossing taken;
                bool more = true;
                while (more) {
                    more = false;
                    int count = 0;
                    for (u32 run_index = run_begin; run_index < run_end; run_index++) {
                        const u8* run = runs + (u64)RUN_BYTES * run_index;
                        u32 outline_index = read_u32(run);
                        u32 first_edge = read_u16(run + 4), edge_count = read_u16(run + 6);
                        Outline outline = tile_outline(scene, *tile, outline_index);
                        u32 first = outline.first_vertex + first_edge;
                        int ax, ay;
                        tile_vertex_local(scene, *tile, first, &ax, &ay);
                        for (u32 next = first + 1; next <= first + edge_count; next++) {
                            int bx, by;
                            tile_vertex_local(scene, *tile, next, &bx, &by);
                            bool meets = high[0] >= min(ax, bx) && max(ax, bx) >= low[0] && high[1] >= min(ay, by) && max(ay, by) >= low[1];
                            double t;
                            if (meets && segment_intersection_t(start[0], start[1], delta[0], delta[1],
                                                                offset[0] + (float)ax, offset[1] + (float)ay,
                                                                offset[0] + (float)bx, offset[1] + (float)by, &t)
                                && t >= t_enter - CELL_WINDOW_SLACK && t <= t_exit + CELL_WINDOW_SLACK
                                && t > endpoint_t && t < 1.0f - endpoint_t) {
                                Crossing crossing;
                                crossing.t = t;
                                crossing.height_m = outline.height_m;
                                crossing.bounds_roof = edge_bounds_roof(scene, *tile, outline_index, outline, next - 1,
                                                                        start[0], start[1], delta[0], delta[1]);
                                crossing.footprint_id = outline.footprint_id;
                                crossing.edge = (slot << 32) | (u64)(next - 1);
                                if (!have_taken || crossing_before(taken, crossing)) {
                                    // Insert in order, keeping the first CELL_CROSSINGS.
                                    int at = count;
                                    if (count == CELL_CROSSINGS) {
                                        more = true;
                                        if (!crossing_before(crossing, found[count - 1])) at = -1;
                                        else at = --count;
                                    }
                                    if (at >= 0) {
                                        while (at > 0 && crossing_before(crossing, found[at - 1])) {
                                            found[at] = found[at - 1];
                                            at--;
                                        }
                                        found[at] = crossing;
                                        count++;
                                    }
                                }
                            }
                            ax = bx;
                            ay = by;
                        }
                    }
                    for (int k = 0; k < count; k++) {
                        found[k].tie = k + 1 < count && found[k + 1].t == found[k].t;
                        u32 failed = take(found[k]);
                        if (failed) return failed;
                    }
                    if (count > 0) {
                        taken = found[count - 1];
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

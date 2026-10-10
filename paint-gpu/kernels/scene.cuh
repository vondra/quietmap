// The tiles of a square's neighbourhood as the device reads them, in f32: the ground
// (popup/src/scene.rs, tiles/src/terrain.rs) and the buildings and walls (popup/src/obstacles/
// {mod,crossings,tile}.rs, tiles/src/obstacles), whose crossings with a ray are walked in order.
// Files are uploaded as stored. Positions are metres
// of the square's frame; the host turns the tiles' absolute Mercator and node coordinates into
// offsets from the frame's origin (device.rs), so no f32 ever holds a world coordinate.

#define TILES_PER_AXIS 4096
#define HEIGHT_MISSING 65535
#define PERCENT_MAX 100
#define CELL_STEPS 256
#define CELLS_PER_SIDE 128
#define CELLS (CELLS_PER_SIDE * CELLS_PER_SIDE)
#define OBSTACLES_HEADER_BYTES 32
#define RUN_BYTES 8
#define OUTLINE_BYTES 20
#define VERTEX_BYTES 4
#define OUTLINE_KIND_WALL 2

#define TILE_NOT_READ 0
#define TILE_EMPTY 1
#define TILE_READ 2

// One terrain tile of the neighbourhood (tiles::terrain::Terrain): its 4-byte nodes in
// `TerrainScene::nodes`, its node column at the frame's origin and per metre east, and its rows'
// metres north of the origin in `TerrainScene::row_north_m` (descending: row 0 is the northernmost).
struct TerrainTile {
    i32 state;
    u32 rows;
    u32 columns;
    u32 pad;
    float column_at_origin;
    float columns_per_metre;
    u64 nodes_offset;
    u64 rows_offset;
};

// popup::scene::Ground of a square: the frame origin's tile coordinates relative to the square's
// tile, the tiles per metre, and the tiles by offset from the square.
struct TerrainScene {
    float tile_x_at_origin;
    float tile_y_at_origin;
    float tiles_per_metre_east;
    float tiles_per_metre_north;
    u32 centre_x;
    u32 centre_y;
    i64 radius;
    const TerrainTile* tiles;
    const u8* nodes;
    const float* row_north_m;
};

struct GroundSample {
    float height_m;
    float ground_factor;
};

// terrain::Terrain::sample at a point `east_m`, `north_m` of the frame; false outside the window
// or at a node without data.
__device__ bool terrain_sample(const TerrainScene& scene, const TerrainTile& tile, float east_m, float north_m, GroundSample* out) {
    float column_position = tile.column_at_origin + east_m * tile.columns_per_metre;
    if (column_position > -1e-6f) column_position = fmaxf(column_position, 0.0f);
    float column = floorf(column_position);
    float column_fraction = column_position - column;
    const float* rows = scene.row_north_m + tile.rows_offset;
    int row_count = (int)tile.rows;
    // The last row at or north of the point, as the Rust finds it on Mercator y: a guess from the
    // mean spacing, then a step or two.
    float spacing = row_count > 1 ? (rows[0] - rows[row_count - 1]) / (float)(row_count - 1) : 1.0f;
    int row = clampi((int)floorf((rows[0] - north_m) / spacing), 0, row_count - 1);
    while (row > 0 && north_m > rows[row]) row--;
    while (row + 1 < row_count && north_m <= rows[row + 1]) row++;
    if (north_m > rows[0] + 1e-4f || column < 0.0f) return false;
    float row_fraction = row + 1 < row_count ? (rows[row] - north_m) / (rows[row] - rows[row + 1]) : 0.0f;
    u32 r = (u32)row, c = (u32)column;
    u32 next_row = r + (row_fraction > 0.0f ? 1u : 0u);
    u32 next_column = c + (column_fraction > 0.0f ? 1u : 0u);
    if (next_row >= tile.rows || next_column >= tile.columns) return false;
    const u8* nodes = scene.nodes + tile.nodes_offset;
    u32 corner_row[4] = {r, r, next_row, next_row};
    u32 corner_column[4] = {c, next_column, c, next_column};
    u32 record[4];
    for (int k = 0; k < 4; k++) record[k] = *(const u32*)(nodes + 4 * ((u64)corner_row[k] * tile.columns + corner_column[k]));
    int nearest = (row_fraction >= 0.5f ? 2 : 0) + (column_fraction >= 0.5f ? 1 : 0);
    if (((record[nearest] >> 24) & 0xff) > PERCENT_MAX) return false;
    for (int k = 0; k < 4; k++) {
        if ((record[k] & 0xffff) == HEIGHT_MISSING || ((record[k] >> 16) & 0xff) > PERCENT_MAX) return false;
    }
    float weights[4] = {
        (1.0f - row_fraction) * (1.0f - column_fraction),
        (1.0f - row_fraction) * column_fraction,
        row_fraction * (1.0f - column_fraction),
        row_fraction * column_fraction,
    };
    float height = 0.0f, imperviousness = 0.0f;
    for (int k = 0; k < 4; k++) {
        height += weights[k] * (-500.0f + (float)(record[k] & 0xffff) * 0.2f);
        imperviousness += weights[k] * (float)((record[k] >> 16) & 0xff);
    }
    out->height_m = height;
    out->ground_factor = 1.0f - imperviousness * 0.01f;
    return true;
}

// scene::Ground::at; 0 or a failure.
__device__ u32 ground_at(const TerrainScene& scene, float east_m, float north_m, GroundSample* out) {
    i64 dx = (i64)floorf(scene.tile_x_at_origin + east_m * scene.tiles_per_metre_east);
    i64 dy = (i64)floorf(scene.tile_y_at_origin - north_m * scene.tiles_per_metre_north);
    if (dx < -scene.radius || dx > scene.radius || dy < -scene.radius || dy > scene.radius) return FAILED_NOT_READ;
    const TerrainTile& tile = scene.tiles[(dy + scene.radius) * (2 * scene.radius + 1) + dx + scene.radius];
    if (tile.state == TILE_NOT_READ) return FAILED_NOT_READ;
    if (tile.state == TILE_EMPTY) {
        out->height_m = 0.0f;
        out->ground_factor = 0.0f;
        return 0;
    }
    return terrain_sample(scene, tile, east_m, north_m, out) ? 0 : FAILED_NO_TERRAIN;
}

// One obstacles tile of the neighbourhood (popup::obstacles::SceneTile): where its file lies in the
// blob, its counts, its local origin in scene steps, and its cells' tallest outline.
struct ObstacleTile {
    i32 state;
    u32 outline_count;
    u32 vertex_count;
    u32 run_count;
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
    u32 vertex_count;
    float height_m;
    int kind;
};

__device__ __forceinline__ const u8* tile_cells(const ObstacleScene& scene, const ObstacleTile& tile) {
    return scene.blob + tile.base + OBSTACLES_HEADER_BYTES;
}
__device__ __forceinline__ const u8* tile_runs(const ObstacleScene& scene, const ObstacleTile& tile) {
    return tile_cells(scene, tile) + 4 * (CELLS + 1);
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
    outline.footprint_id = (u64)read_u32(record) | ((u64)read_u32(record + 4) << 32);
    outline.first_vertex = read_u32(record + 8);
    outline.vertex_count = read_u16(record + 12);
    outline.height_m = (float)read_u16(record + 14) * 0.1f;
    outline.kind = record[16];
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
__device__ __forceinline__ bool segment_intersection_t(float sx, float sy, float dx, float dy, float ax, float ay, float bx, float by, float* t_out) {
    float ex = bx - ax, ey = by - ay;
    float denominator = dx * ey - dy * ex;
    if (denominator == 0.0f) return false;
    float wx = ax - sx, wy = ay - sy;
    float t = (wx * ey - wy * ex) / denominator;
    float u = (wx * dy - wy * dx) / denominator;
    if (t > 0.0f && t < 1.0f && u >= 0.0f && u <= 1.0f) {
        *t_out = t;
        return true;
    }
    return false;
}

// One crossing of the ray with a building ring or a wall (ray.rs Crossing), and the edge it was
// found on (the tile slot and the edge's first vertex): the order of crossings at one t. `tie`: the
// next crossing handed on has the same t.
struct Crossing {
    float t;
    float height_m;
    u64 footprint_id;
    u64 edge;
    int building;
    int tie;
};

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
            const u8* cells = tile_cells(scene, *tile);
            u32 run_begin = read_u32(cells + 4 * index), run_end = read_u32(cells + 4 * (index + 1));
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
                            float t;
                            if (meets && segment_intersection_t(start[0], start[1], delta[0], delta[1],
                                                                offset[0] + (float)ax, offset[1] + (float)ay,
                                                                offset[0] + (float)bx, offset[1] + (float)by, &t)
                                && t >= t_enter - CELL_WINDOW_SLACK && t <= t_exit + CELL_WINDOW_SLACK
                                && t > endpoint_t && t < 1.0f - endpoint_t) {
                                Crossing crossing;
                                crossing.t = t;
                                crossing.height_m = outline.height_m;
                                crossing.building = outline.kind != OUTLINE_KIND_WALL;
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
        int axis;
        if (cell[0] == last[0]) axis = 1;
        else if (cell[1] == last[1] || t_max[0] < t_max[1]) axis = 0;
        else axis = 1;
        cell[axis] += step[axis];
    }
    return 0;
}

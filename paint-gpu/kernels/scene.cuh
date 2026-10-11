// The tiles of a square's neighbourhood as the device reads them, in f32: the ground
// (popup/src/scene.rs, tiles/src/terrain.rs) and the buildings and walls (popup/src/obstacles/
// {mod,crossings,tile}.rs, tiles/src/obstacles), whose crossings with a ray are walked in order.
// Terrain is uploaded as its decoded nodes, the obstacles files as stored. Positions are metres
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

// One terrain tile of the neighbourhood (tiles::terrain::Terrain): its 4-byte nodes and its
// crowns' heights (a byte a node) in `TerrainScene::nodes`, its node column at the frame's origin
// and per metre east, and its rows' metres north of the origin in `TerrainScene::row_north_m`
// (descending: row 0 is the northernmost); its fine terrain (tiles::terrain_fine), if any, as i16
// detail codes row-major over the window six times as fine, in `nodes` too (offset 0: none, as
// the first tile's nodes come first).
struct TerrainTile {
    i32 state;
    u32 rows;
    u32 columns;
    i32 row_shift;              // the scene row of this tile's row 0 (local row = scene row - shift)
    float column_at_origin;
    float columns_per_metre;
    u64 nodes_offset;
    u64 canopy_offset;
    u64 rows_offset;
    u64 fine_offset;
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
    // The lattice rows over the whole scene, metres north of the origin (descending), and a
    // column coordinate whose integer values are the lattice columns (any tile's numbering: tiles
    // differ by whole columns): popup::scene::Ground's row table and column lines.
    const float* scene_rows;
    u32 scene_row_count;
    float column_at_origin;
    float columns_per_metre;
    u32 has_fine;               // whether any tile has fine terrain
};

struct GroundSample {
    float height_m;
    float ground_factor;
    float forest_cover;         // in [0, 1], of the nearest node
    float canopy_m;             // the crowns' height above the ground, of the nearest node
};

// The trees of node (row, column) of a read tile into `out`; false where it has no forest data.
__device__ bool tile_trees(const TerrainScene& sc, const TerrainTile& tile, u32 row, u32 column, GroundSample* out) {
    u64 index = (u64)row * tile.columns + column;
    u32 forest = *(const u32*)(sc.nodes + tile.nodes_offset + 4 * index) >> 24;
    out->forest_cover = (float)forest * 0.01f;
    out->canopy_m = (float)sc.nodes[tile.canopy_offset + index];
    return forest <= PERCENT_MAX;
}

// The ground of an ocean tile: sea level, hard water, no trees (tiles::terrain::OCEAN).
__device__ void ocean(GroundSample* out) {
    out->height_m = 0.0f;
    out->ground_factor = 0.0f;
    out->forest_cover = 0.0f;
    out->canopy_m = 0.0f;
}

// The fine detail (m) at fractional node coordinates of a tile's fine window (tiles::terrain_fine::
// FineTerrain::at): its codes are i16 in DETAIL_STEP_M steps.
__device__ float fine_detail_at(const TerrainScene& sc, const TerrainTile& tile, float row, float column) {
    u32 rows = FINE_PER_NODE * (tile.rows - 1) + 1, columns = FINE_PER_NODE * (tile.columns - 1) + 1;
    // A point a hair north of the window (terrain_sample's slack for f32) reads the first row.
    u32 r = min((u32)fmaxf(floorf(row), 0.0f), rows - 2), c = min((u32)fmaxf(floorf(column), 0.0f), columns - 2);
    float fr = row - (float)r, fc = column - (float)c;
    const i16* detail = (const i16*)(sc.nodes + tile.fine_offset);
    u64 at = (u64)r * columns + c;
    float sum = (1.0f - fr) * ((1.0f - fc) * (float)detail[at] + fc * (float)detail[at + 1])
        + fr * ((1.0f - fc) * (float)detail[at + columns] + fc * (float)detail[at + columns + 1]);
    return sum * DETAIL_STEP_M;
}

// terrain::Terrain::sample_at: the ground at row `r` and column `c` of a read tile and the
// fractions south and east within that cell, the trees of the nearest node (one half: south or
// east); false outside the window or at a node without data.
__device__ bool cell_sample(const TerrainScene& scene, const TerrainTile& tile, u32 r, float row_fraction, u32 c,
                            float column_fraction, GroundSample* out) {
    u32 next_row = r + (row_fraction > 0.0f ? 1u : 0u);
    u32 next_column = c + (column_fraction > 0.0f ? 1u : 0u);
    if (next_row >= tile.rows || next_column >= tile.columns) return false;
    const u8* nodes = scene.nodes + tile.nodes_offset;
    u32 corner_row[4] = {r, r, next_row, next_row};
    u32 corner_column[4] = {c, next_column, c, next_column};
    u32 record[4];
    for (int k = 0; k < 4; k++) record[k] = *(const u32*)(nodes + 4 * ((u64)corner_row[k] * tile.columns + corner_column[k]));
    int nearest = (row_fraction >= 0.5f ? 2 : 0) + (column_fraction >= 0.5f ? 1 : 0);
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
    return tile_trees(scene, tile, corner_row[nearest], corner_column[nearest], out);
}

// terrain::Terrain::sample at a point `east_m`, `north_m` of the frame, the tile's fine detail added
// where it has one (popup::scene::Ground::at); false outside the window or at a node without data.
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
    if (!cell_sample(scene, tile, (u32)row, row_fraction, (u32)column, column_fraction, out)) return false;
    if (tile.fine_offset) {
        out->height_m += fine_detail_at(scene, tile, FINE_PER_NODE * ((float)row + row_fraction), FINE_PER_NODE * column_position);
    }
    return true;
}

// The tile of the neighbourhood at offset (dx, dy) from the square, or null outside the radius.
__device__ const TerrainTile* terrain_tile(const TerrainScene& scene, i64 dx, i64 dy) {
    if (dx < -scene.radius || dx > scene.radius || dy < -scene.radius || dy > scene.radius) return 0;
    return &scene.tiles[(dy + scene.radius) * (2 * scene.radius + 1) + dx + scene.radius];
}

// Metres north of the origin of row line `k` of the lattice `scale` times as fine as the one
// arc-second, the northernmost 0: on the 1" rows, evenly between them (popup::scene::Ground::
// row_line_y).
__device__ __forceinline__ float row_line_north(const TerrainScene& sc, int scale, int k) {
    int row = k / scale, sub = k - row * scale;
    float north = sc.scene_rows[row];
    return sub == 0 ? north : north + (sc.scene_rows[row + 1] - north) * (float)sub / (float)scale;
}

// The lines of the lattice `scale` times as fine as the one arc-second a ray crosses, in order
// from its source (popup::scene::Ground::walk): column lines at the integer values of u, row lines
// at row_line_north.
struct LatticeWalk {
    float u_s, du;          // the column coordinate at the source, and its change to the receiver
    float n_s, dn;          // metres north at the source, and the change
    float next_column, column_step;
    int next_row, row_step; // the scene row line ahead (outside the lines: none)
    int north;              // the scene row at or north of the point, in the direction of travel
    int scale, lines;
};

// The walk from (sx, sy) to (rx, ry) from fraction `from` on. A start exactly on a row line
// belongs to the stretch the ray heads into.
__device__ void walk_start(const TerrainScene& sc, int scale, float sx, float sy, float rx, float ry, float from, LatticeWalk* w) {
    w->scale = scale;
    w->lines = scale * ((int)sc.scene_row_count - 1) + 1;
    w->u_s = (float)scale * (sc.column_at_origin + sx * sc.columns_per_metre);
    w->du = (float)scale * (rx - sx) * sc.columns_per_metre;
    w->n_s = sy;
    w->dn = ry - sy;
    float u = w->u_s + from * w->du, n = w->n_s + from * w->dn;
    if (w->du > 0.0f) {
        w->next_column = floorf(u) + 1.0f;
        w->column_step = 1.0f;
    } else {
        w->next_column = ceilf(u) - 1.0f;
        w->column_step = -1.0f;
    }
    // Rows north of the start: north_m > n heading north, north_m >= n heading south.
    int lo = 0, hi = w->lines;
    while (lo < hi) {
        int mid = (lo + hi) / 2;
        float line = row_line_north(sc, scale, mid);
        bool north = w->dn < 0.0f ? line >= n : line > n;
        if (north) lo = mid + 1; else hi = mid;
    }
    w->north = lo - 1;
    w->next_row = w->dn < 0.0f ? lo : lo - 1;
    w->row_step = w->dn < 0.0f ? 1 : -1;
}

// The next crossing strictly before fraction `to`: its fraction t, and whether it lies on a row
// line (`row` its scene row on the walk's lattice) or a column line; false past it.
__device__ bool walk_next(const TerrainScene& sc, LatticeWalk* w, float to, float* t, bool* on_row, int* row) {
    float t_column = w->du != 0.0f ? (w->next_column - w->u_s) / w->du : QM_INFINITY;
    float t_row = (w->dn != 0.0f && w->next_row >= 0 && w->next_row < w->lines)
        ? (row_line_north(sc, w->scale, w->next_row) - w->n_s) / w->dn : QM_INFINITY;
    *t = fminf(t_column, t_row);
    if (!(*t < to)) return false;
    *on_row = t_row <= t_column;
    *row = w->next_row;
    if (t_row <= t_column) {
        w->north = w->dn < 0.0f ? w->next_row : w->next_row - 1;
        w->next_row += w->row_step;
    }
    if (t_column <= t_row) w->next_column += w->column_step;
    return true;
}

// Node (row, column) of a read tile, false outside its window or without data.
__device__ bool tile_node(const TerrainScene& sc, const TerrainTile& tile, int row, int column, float* height, float* imperviousness) {
    if (row < 0 || column < 0 || row >= (int)tile.rows || column >= (int)tile.columns) return false;
    u32 record = *(const u32*)(sc.nodes + tile.nodes_offset + 4 * ((u64)row * tile.columns + (u64)column));
    if ((record & 0xffff) == HEIGHT_MISSING || ((record >> 16) & 0xff) > PERCENT_MAX) return false;
    *height = -500.0f + (float)(record & 0xffff) * 0.2f;
    *imperviousness = (float)((record >> 16) & 0xff);
    return true;
}

// The ground at a walk's vertex in `tile`: between two nodes of a row line or a column line, the
// node of no weight not read; false where a weighed node is outside the window or has no data.
__device__ bool tile_edge_sample(const TerrainScene& sc, const TerrainTile& tile, float east_m, float north_m, bool on_row,
                                 int scene_row, int north, GroundSample* out) {
    int r0, c0, r1, c1;
    float f;
    float column = tile.column_at_origin + east_m * tile.columns_per_metre;
    if (on_row) {
        r0 = r1 = scene_row - tile.row_shift;
        float west = floorf(column);
        c0 = (int)west;
        c1 = c0 + 1;
        f = column - west;
    } else {
        c0 = c1 = (int)rintf(column);
        r0 = north - tile.row_shift;
        r1 = r0 + 1;
        float y0 = sc.scene_rows[north], y1 = sc.scene_rows[north + 1];
        f = clampf((y0 - north_m) / (y0 - y1), 0.0f, 1.0f);
    }
    float h0 = 0.0f, i0 = 0.0f, h1 = 0.0f, i1 = 0.0f;
    if (f < 1.0f && !tile_node(sc, tile, r0, c0, &h0, &i0)) return false;
    if (f > 0.0f && !tile_node(sc, tile, r1, c1, &h1, &i1)) return false;
    if (f <= 0.0f) { h1 = h0; i1 = i0; }
    if (f >= 1.0f) { h0 = h1; i0 = i1; }
    out->height_m = h0 + f * (h1 - h0);
    out->ground_factor = 1.0f - (i0 + f * (i1 - i0)) * 0.01f;
    return f < 0.5f ? tile_trees(sc, tile, (u32)r0, (u32)c0, out) : tile_trees(sc, tile, (u32)r1, (u32)c1, out);
}

// A walk's vertex this close to a tile's edge (in tiles, about 10 cm) lies on it: f32's error is
// millimetres, and no lattice line that is not the edge comes within a metre of it.
#define ON_TILE_EDGE 1e-5f

// The ground at a walk's vertex (popup::scene::Ground::fill_profile): from the tile holding it,
// or, for a vertex on the north or west edge of an ocean tile (the equator, every 32nd tile
// meridian: lattice lines), from the land tile across that edge, which holds the seam's nodes;
// 0 or a failure.
__device__ u32 walk_sample(const TerrainScene& sc, float east_m, float north_m, bool on_row, int scene_row,
                           int north, GroundSample* out) {
    float tile_x = sc.tile_x_at_origin + east_m * sc.tiles_per_metre_east;
    float tile_y = sc.tile_y_at_origin - north_m * sc.tiles_per_metre_north;
    i64 dx = (i64)floorf(tile_x), dy = (i64)floorf(tile_y);
    const TerrainTile* tile = terrain_tile(sc, dx, dy);
    if (!tile || tile->state == TILE_NOT_READ) return FAILED_NOT_READ;
    if (tile->state == TILE_READ) {
        return tile_edge_sample(sc, *tile, east_m, north_m, on_row, scene_row, north, out) ? 0 : FAILED_NO_TERRAIN;
    }
    const TerrainTile* across[2] = {
        tile_y - (float)dy < ON_TILE_EDGE ? terrain_tile(sc, dx, dy - 1) : 0,
        tile_x - (float)dx < ON_TILE_EDGE ? terrain_tile(sc, dx - 1, dy) : 0,
    };
    for (int k = 0; k < 2; k++) {
        if (across[k] && across[k]->state == TILE_READ
            && tile_edge_sample(sc, *across[k], east_m, north_m, on_row, scene_row, north, out)) return 0;
    }
    ocean(out);
    return 0;
}

// scene::Ground::at, the 1/6" height where the tile has one; 0 or a failure.
__device__ u32 ground_at(const TerrainScene& scene, float east_m, float north_m, GroundSample* out) {
    i64 dx = (i64)floorf(scene.tile_x_at_origin + east_m * scene.tiles_per_metre_east);
    i64 dy = (i64)floorf(scene.tile_y_at_origin - north_m * scene.tiles_per_metre_north);
    if (dx < -scene.radius || dx > scene.radius || dy < -scene.radius || dy > scene.radius) return FAILED_NOT_READ;
    const TerrainTile& tile = scene.tiles[(dy + scene.radius) * (2 * scene.radius + 1) + dx + scene.radius];
    if (tile.state == TILE_NOT_READ) return FAILED_NOT_READ;
    if (tile.state == TILE_EMPTY) {
        ocean(out);
        return 0;
    }
    return terrain_sample(scene, tile, east_m, north_m, out) ? 0 : FAILED_NO_TERRAIN;
}

// Whether a tile under the stretch from (ax, ay) to (bx, by) has fine terrain: the tiles of its
// box's corners (popup::scene::Ground::fine_near).
__device__ bool fine_near(const TerrainScene& sc, float ax, float ay, float bx, float by) {
    float xs[2] = {fminf(ax, bx), fmaxf(ax, bx)}, ys[2] = {fminf(ay, by), fmaxf(ay, by)};
    for (int i = 0; i < 2; i++) {
        for (int j = 0; j < 2; j++) {
            const TerrainTile* tile = terrain_tile(sc, (i64)floorf(sc.tile_x_at_origin + xs[i] * sc.tiles_per_metre_east),
                                                   (i64)floorf(sc.tile_y_at_origin - ys[j] * sc.tiles_per_metre_north));
            if (tile && tile->state == TILE_READ && tile->fine_offset) return true;
        }
    }
    return false;
}

// The stretches of the ray from (sx, sy) to (rx, ry) by lattice (popup::scene::Ground::
// fill_profile): within FINE_ENDS_M of either end the 1/6" lattice where a tile under that stretch
// has fine terrain, the 1" one elsewhere, neighbours on one lattice joined; their ends into `ends`
// (the last 1) and their lattices' scales into `scales`.
__device__ void stretches_of(const TerrainScene& sc, float sx, float sy, float rx, float ry, float* ends, int* scales) {
    float length = hypotf(rx - sx, ry - sy);
    if (length <= 2.0f * FINE_ENDS_M) {
        scales[0] = fine_near(sc, sx, sy, rx, ry) ? FINE_PER_NODE : 1;
        return;
    }
    float end = FINE_ENDS_M / length;
    float ax = sx + end * (rx - sx), ay = sy + end * (ry - sy);
    float bx = sx + (1.0f - end) * (rx - sx), by = sy + (1.0f - end) * (ry - sy);
    int candidate_scales[3] = {fine_near(sc, sx, sy, ax, ay) ? FINE_PER_NODE : 1, 1,
                               fine_near(sc, bx, by, rx, ry) ? FINE_PER_NODE : 1};
    float candidate_ends[3] = {end, 1.0f - end, 1.0f};
    int count = 0;
    for (int k = 0; k < 3; k++) {
        if (count > 0 && scales[count - 1] == candidate_scales[k]) {
            ends[count - 1] = candidate_ends[k];
        } else {
            scales[count] = candidate_scales[k];
            ends[count] = candidate_ends[k];
            count++;
        }
    }
}

// The ground at a vertex of the fine walk (popup::scene::Ground::fine_sample): the 1" tile's
// bilinear height, ground factor and trees at the vertex's place in its 1" cell, taken from its fine
// indices (the scene's fine row `scene_row` for a row line, else the column line between fine rows
// `north` and the next), plus the tile's fine detail on the line, a node of no weight not read; a
// vertex on an ocean tile's north or west edge reads the land tile across it, as walk_sample does;
// 0 or a failure.
__device__ u32 fine_walk_sample(const TerrainScene& sc, float east_m, float north_m, bool on_row, int scene_row, int north,
                                GroundSample* out) {
    float tile_x = sc.tile_x_at_origin + east_m * sc.tiles_per_metre_east;
    float tile_y = sc.tile_y_at_origin - north_m * sc.tiles_per_metre_north;
    i64 dx = (i64)floorf(tile_x), dy = (i64)floorf(tile_y);
    const TerrainTile* tile = terrain_tile(sc, dx, dy);
    if (!tile || tile->state == TILE_NOT_READ) return FAILED_NOT_READ;
    if (tile->state == TILE_EMPTY) {
        // Only a 1" line is ever a tile's edge; a finer one can pass within ON_TILE_EDGE of an edge
        // inside the ocean tile, which the CPU reads as ocean.
        bool coarse_row = on_row && scene_row % FINE_PER_NODE == 0;
        bool coarse_column = !on_row
            && (int)rintf(FINE_PER_NODE * (sc.column_at_origin + east_m * sc.columns_per_metre)) % FINE_PER_NODE == 0;
        const TerrainTile* across[2] = {
            coarse_row && tile_y - (float)dy < ON_TILE_EDGE ? terrain_tile(sc, dx, dy - 1) : 0,
            coarse_column && tile_x - (float)dx < ON_TILE_EDGE ? terrain_tile(sc, dx - 1, dy) : 0,
        };
        tile = 0;
        for (int k = 0; k < 2 && !tile; k++) {
            if (across[k] && across[k]->state == TILE_READ) tile = across[k];
        }
        if (!tile) {
            ocean(out);
            return 0;
        }
    }
    float column = FINE_PER_NODE * (tile->column_at_origin + east_m * tile->columns_per_metre);
    int r0, c0;
    float f;
    if (on_row) {
        r0 = scene_row - FINE_PER_NODE * tile->row_shift;
        float west = floorf(column);
        c0 = (int)west;
        f = column - west;
    } else {
        c0 = (int)rintf(column);
        r0 = north - FINE_PER_NODE * tile->row_shift;
        float y0 = row_line_north(sc, FINE_PER_NODE, north), y1 = row_line_north(sc, FINE_PER_NODE, north + 1);
        f = clampf((y0 - north_m) / (y0 - y1), 0.0f, 1.0f);
    }
    int rows = FINE_PER_NODE * ((int)tile->rows - 1) + 1, columns = FINE_PER_NODE * ((int)tile->columns - 1) + 1;
    int r1 = on_row ? r0 : r0 + 1, c1 = on_row ? c0 + 1 : c0;
    if (r0 < 0 || c0 < 0 || r0 >= rows || c0 >= columns || (f > 0.0f && (r1 >= rows || c1 >= columns))) return FAILED_NO_TERRAIN;
    // The vertex's place in its 1" cell from its fine indices: on its own line exactly, along it by
    // the fraction (a vertex on a cell's midline takes the CPU's trees).
    float row_position = (on_row ? (float)r0 : (float)r0 + f) / (float)FINE_PER_NODE;
    float column_position = (on_row ? (float)c0 + f : (float)c0) / (float)FINE_PER_NODE;
    float cell_row = floorf(row_position), cell_column = floorf(column_position);
    if (!cell_sample(sc, *tile, (u32)cell_row, row_position - cell_row, (u32)cell_column, column_position - cell_column, out)) {
        return FAILED_NO_TERRAIN;
    }
    if (tile->fine_offset) {
        const i16* detail = (const i16*)(sc.nodes + tile->fine_offset);
        float d0 = f < 1.0f ? (float)detail[(u64)r0 * columns + c0] : 0.0f;
        float d1 = f > 0.0f ? (float)detail[(u64)r1 * columns + c1] : 0.0f;
        if (f <= 0.0f) d1 = d0;
        if (f >= 1.0f) d0 = d1;
        out->height_m += (d0 + f * (d1 - d0)) * DETAIL_STEP_M;
    }
    return 0;
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
        // The axis chosen by a branch, not as an index: an array indexed at run time lives in local
        // memory.
        if (cell[0] != last[0] && (cell[1] == last[1] || t_max[0] < t_max[1])) cell[0] += step[0];
        else cell[1] += step[1];
    }
    return 0;
}

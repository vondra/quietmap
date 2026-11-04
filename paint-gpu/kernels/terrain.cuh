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
#define CELL_WORDS (CELLS / 64)
#define RUN_BYTES 8
#define OUTLINE_BYTES 12
#define VERTEX_BYTES 4
#define OUTLINE_KIND_WALL 2

#define TILE_NOT_READ 0
#define TILE_EMPTY 1
#define TILE_READ 2

// One terrain tile of the neighbourhood (tiles::terrain::Terrain): its 4-byte nodes and its
// crowns' heights (a byte a node) in `TerrainScene::nodes`, its node column at the frame's origin
// and per metre east, its rows those of the scene's row table from `row_shift` (row 0 the
// northernmost); its fine terrain (tiles::terrain_fine), if any, as i16
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
    u64 fine_offset;
};

// popup::scene::Ground of a square: the frame origin's tile coordinates relative to the square's
// tile, the tiles per metre, and the tiles by offset from the square.
struct TerrainScene {
    float tile_x_at_origin;
    float tile_y_at_origin;
    float tiles_per_metre_east;
    float tiles_per_metre_north;
    i64 radius;
    const TerrainTile* tiles;
    const u8* nodes;
    // The lattice rows over the whole scene, metres north of the origin (descending), and a
    // column coordinate whose integer values are the lattice columns (any tile's numbering: tiles
    // differ by whole columns): popup::scene::Ground's row table and column lines.
    const float* scene_rows;
    u32 scene_row_count;
    float column_at_origin;
    float columns_per_metre;
    u32 has_fine;               // whether any tile has fine terrain
    i64 equator_row;            // the scene row on the equator, -1 for none
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
    row = fmaxf(row, 0.0f);
    column = fmaxf(column, 0.0f);
    u32 r = min((u32)floorf(row), rows - 2), c = min((u32)floorf(column), columns - 2);
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
    const float* rows = scene.scene_rows + tile.row_shift;
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

// Metres north of the origin of row line `k` of the lattice `scale` (1 or FINE_PER_NODE) times as
// fine as the one arc-second, the northernmost 0: on the 1" rows, evenly between them (popup::scene::
// Ground::row_line_y). The scale is one of two constants: a division by a variable takes some twenty
// instructions, by a constant a multiply.
__device__ __forceinline__ float row_line_north(const TerrainScene& sc, int scale, int k) {
    if (scale == 1) return sc.scene_rows[k];
    int row = k / FINE_PER_NODE, sub = k - row * FINE_PER_NODE;
    float north = sc.scene_rows[row];
    return sub == 0 ? north : north + (sc.scene_rows[row + 1] - north) * (float)sub / (float)FINE_PER_NODE;
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

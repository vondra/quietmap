// Terrain sampling along coarse and fine lattice walks.

// A walk's vertex on a lattice line this close to a tile's edge (in tiles, 1-6 cm) lies on it, as
// the CPU's exact test finds (f32's error is millimetres): a column line on a tile meridian (every
// 32nd on the 1" lattice, every 16th on the 1/6"; any other passes a sixteenth of a fine column or
// more from it), or a row line on the equator, the one parallel that is a tile's edge (five 1" rows
// pass within 2 to 9 cm of one: -25.32, -6.14, -4.92, 27.22 and 63.70 degrees).
#define ON_TILE_EDGE 1e-5f

// Whether a walk's vertex on row line `scene_row` of the lattice `scale` times as fine as the one
// arc-second lies on the equator.
__device__ __forceinline__ bool on_equator(const TerrainScene& sc, bool on_row, int scene_row, int scale) {
    return on_row && sc.equator_row >= 0 && scene_row == scale * sc.equator_row;
}

// The ground at a walk's vertex (popup::scene::Ground::fill_profile): from the tile holding it,
// or, for a vertex on the north or west edge of an ocean tile (the equator, a tile meridian on a
// lattice column), from the land tile across that edge, which holds the seam's nodes;
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
        on_equator(sc, on_row, scene_row, 1) && tile_y - (float)dy < ON_TILE_EDGE ? terrain_tile(sc, dx, dy - 1) : 0,
        !on_row && tile_x - (float)dx < ON_TILE_EDGE ? terrain_tile(sc, dx - 1, dy) : 0,
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
        const TerrainTile* across[2] = {
            on_equator(sc, on_row, scene_row, FINE_PER_NODE) && tile_y - (float)dy < ON_TILE_EDGE ? terrain_tile(sc, dx, dy - 1) : 0,
            !on_row && tile_x - (float)dx < ON_TILE_EDGE ? terrain_tile(sc, dx - 1, dy) : 0,
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

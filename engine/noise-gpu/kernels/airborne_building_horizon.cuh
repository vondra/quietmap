// Device construction of the receiver-local vector-building horizon.
#pragma once

#if !defined(BUILDING_ENV_INDEX_COUNT) || !defined(BUILDING_ENV_GRID_GEOMETRY) || \
    !defined(BUILDING_ENV_GRID_LAYOUT) || !defined(BUILDING_ENV_CELL_STARTS) || \
    !defined(BUILDING_ENV_EDGE_REFS) || !defined(BUILDING_ENV_EDGES) || \
    !defined(BUILDING_ENV_EDGE_IS_BUILDING) || !defined(BUILDING_ENV_DEM_META) || \
    !defined(BUILDING_ENV_DEM_ELEVATION) || !defined(BUILDING_ENV_DEM_COLS) || \
    !defined(BUILDING_ENV_DEM_ROWS) || !defined(BUILDING_ENV_DIRECTIONS) || \
    !defined(BUILDING_GRID_GEOMETRY_STRIDE) || !defined(BUILDING_GRID_LAYOUT_STRIDE)
#error "airborne building environment ABI must be injected by build.rs"
#endif
#if !defined(BUILDING_LOCAL_MAX_M_D) || !defined(BUILDING_FIRST_RANGE_BREAK_M_D) || \
    !defined(BUILDING_RANGE_GROWTH_D) || !defined(BUILDING_MIN_EDGE_RANGE_M_D) || \
    !defined(M_LAT)
#error "airborne building geometry constants must be injected by build.rs"
#endif

__device__ __forceinline__ float building_origin_to_segment_distance(
    float x0, float y0, float x1, float y1)
{
    float ex = x1 - x0;
    float ey = y1 - y0;
    float length_sq = ex * ex + ey * ey;
    float t = length_sq > 0.0f
        ? fminf(fmaxf(-(x0 * ex + y0 * ey) / length_sq, 0.0f), 1.0f)
        : 0.0f;
    return hypotf(x0 + t * ex, y0 + t * ey);
}

__device__ __forceinline__ float building_wrap_pi(float angle) {
    angle = fmodf(angle, (float)TAU_D);
    if (angle > (float)PI_D) return angle - (float)TAU_D;
    if (angle <= -(float)PI_D) return angle + (float)TAU_D;
    return angle;
}

__device__ __forceinline__ float building_intersection_range(
    float ray_dx, float ray_dy, float x0, float y0, float x1, float y1)
{
    float ex = x1 - x0;
    float ey = y1 - y0;
    float denominator = ray_dx * ey - ray_dy * ex;
    if (denominator == 0.0f) return 0.0f;
    float t = (x0 * ey - y0 * ex) / denominator;
    float u = (x0 * ray_dy - y0 * ray_dx) / denominator;
    if (!(t > 0.0f && t < 1.0f && u >= 0.0f && u <= 1.0f)) return 0.0f;
    return t * (float)BUILDING_LOCAL_MAX_M_D;
}

// Halo bilinear at clamped lattice coordinates (`rf`, `cf`), the second half of
// `building_dem_elevation`; the terrain march tables its rows' `rf` per sample.
__device__ __forceinline__ double building_dem_elevation_at(
    const unsigned long long* environment, double rf, double cf)
{
    const float* elevation =
        reinterpret_cast<const float*>(environment[BUILDING_ENV_DEM_ELEVATION]);
    unsigned long long cols = environment[BUILDING_ENV_DEM_COLS];
    unsigned long long rows = environment[BUILDING_ENV_DEM_ROWS];
    unsigned long long r0 = min((unsigned long long)floor(rf), rows - 2);
    unsigned long long c0 = min((unsigned long long)floor(cf), cols - 2);
    double fr = rf - (double)r0;
    double fc = cf - (double)c0;
    unsigned long long base = r0 * cols + c0;
    double v0 = (double)elevation[base]
        + fc * ((double)elevation[base + 1] - (double)elevation[base]);
    double v1 = (double)elevation[base + cols]
        + fc * ((double)elevation[base + cols + 1] - (double)elevation[base + cols]);
    return v0 + fr * (v1 - v0);
}

__device__ __forceinline__ double building_dem_lattice_row(
    const unsigned long long* environment, double lat)
{
    const double* meta = reinterpret_cast<const double*>(environment[BUILDING_ENV_DEM_META]);
    unsigned long long rows = environment[BUILDING_ENV_DEM_ROWS];
    return fmin(fmax((lat - meta[0]) * meta[2], 0.0), (double)(rows - 1));
}

__device__ __forceinline__ double building_dem_lattice_col(
    const unsigned long long* environment, double lon)
{
    const double* meta = reinterpret_cast<const double*>(environment[BUILDING_ENV_DEM_META]);
    unsigned long long cols = environment[BUILDING_ENV_DEM_COLS];
    return fmin(fmax((lon - meta[1]) * meta[2], 0.0), (double)(cols - 1));
}

__device__ __forceinline__ double building_dem_elevation(
    const unsigned long long* environment, double lat, double lon)
{
    return building_dem_elevation_at(environment,
                                     building_dem_lattice_row(environment, lat),
                                     building_dem_lattice_col(environment, lon));
}

// The inner pixel row a latitude inside the tile bbox reads, else -1: the latitude half of
// `building_tile_elevation`, which the terrain march tables per receiver row.
__device__ __forceinline__ int building_tile_row(const double* bbox, double lat) {
    if (!(lat >= bbox[0] && lat <= bbox[1])) return -1;
    double lat_fraction = (bbox[1] - lat) / (bbox[1] - bbox[0]);
    return (int)floor(fmin(fmax(lat_fraction * TPX, 0.0), (double)(TPX - 1)));
}

__device__ __forceinline__ double building_tile_elevation(
    const unsigned long long* environment,
    const float* inner_elevation,
    const double* bbox,
    double lat,
    double lon)
{
    if (lat >= bbox[0] && lat <= bbox[1] && lon >= bbox[2] && lon <= bbox[3]) {
        double lat_fraction = (bbox[1] - lat) / (bbox[1] - bbox[0]);
        double lon_fraction = (lon - bbox[2]) / (bbox[3] - bbox[2]);
        int py = (int)floor(fmin(fmax(lat_fraction * TPX, 0.0), (double)(TPX - 1)));
        int px = (int)floor(fmin(fmax(lon_fraction * TPX, 0.0), (double)(TPX - 1)));
        return (double)inner_elevation[py * TPX + px];
    }
    return building_dem_elevation(environment, lat, lon);
}

__device__ __forceinline__ int building_range_band(float range_m) {
    float range_break_m = (float)BUILDING_FIRST_RANGE_BREAK_M_D;
    for (int band = 0; band < BUILDING_LOCAL_BANDS - 1; band++) {
        if (range_m <= range_break_m) return band;
        range_break_m *= (float)BUILDING_RANGE_GROWTH_D;
    }
    return BUILDING_LOCAL_BANDS - 1;
}

__device__ __forceinline__ unsigned short building_tangent_floor(double tangent) {
    const double f32_max = 3.4028234663852886e38;
    float tangent_f32 = (float)fmin(fmax(tangent, -f32_max), f32_max);
    unsigned short bits = (unsigned short)(__float_as_uint(tangent_f32) >> 16);
    double decoded = (double)__uint_as_float((unsigned int)bits << 16);
    if (decoded > tangent) {
        bits = signbit(tangent_f32) ? (unsigned short)(bits + 1) : (unsigned short)(bits - 1);
    }
    return bits;
}

// The direction arc an obstacle grid cell occupies from the receiver, and with it the lowest
// aircraft tangent in that local-cell proxy. The cell lies inside the disk around its centre,
// so one `atan2` and one `asin` bound the arc; a cell that wraps the receiver falls back to the
// record's minimum over all directions.
__device__ __forceinline__ bool building_cell_cannot_screen(
    float cell_top_rel_alt_m, const float* group_floor, float record_floor,
    float x_lo, float y_lo, float step_x, float step_y)
{
    float gap_x = fmaxf(fmaxf(x_lo, -(x_lo + step_x)), 0.0f);
    float gap_y = fmaxf(fmaxf(y_lo, -(y_lo + step_y)), 0.0f);
    float nearest_m = fmaxf(hypotf(gap_x, gap_y) - CELL_RANGE_MARGIN_M, 0.0f);
    float centre_x = x_lo + 0.5f * step_x;
    float centre_y = y_lo + 0.5f * step_y;
    float centre_m = hypotf(centre_x, centre_y);
    float circumradius_m = 0.5f * hypotf(step_x, step_y);
    float lowest = record_floor;
    if (centre_m > circumradius_m) {
        float delta = asinf(fminf(circumradius_m / centre_m, 1.0f)) + BOUND_ANGLE_MARGIN;
        float angle = atan2f(centre_y, centre_x);
        int first = (int)floorf((angle - delta) * BOUND_GROUPS_PER_TAU);
        int last = (int)floorf((angle + delta) * BOUND_GROUPS_PER_TAU);
        if (last - first <= BOUND_GROUPS - 1) {
            lowest = BOUND_FLOAT_MAX;
            for (int g = first; g <= last; g++) {
                int group = g % BOUND_GROUPS;
                lowest = fminf(lowest, group_floor[(group < 0) ? group + BOUND_GROUPS : group]);
            }
        }
    }
    return lowest >= 0.0f
        && horizon_edge_cannot_screen(cell_top_rel_alt_m, nearest_m, lowest);
}

// Absolute coordinates and DEM addressing stay f64. Local metre geometry is
// f32 because the emitted roofline is bfloat16 plus centimetre range; the tile
// parity contract guards this deliberate reduction on complete encoded output.
extern "C" __global__ void airborne_building_horizon_build(
    const unsigned long long* __restrict__ environment,
    const double* __restrict__ receiver_lat_lon,
    const float* __restrict__ receiver_altitude,
    const float* __restrict__ inner_elevation,
    const double* __restrict__ tile_bbox,
    const unsigned int* __restrict__ pixel_of_record,
    const unsigned char* __restrict__ enabled,
    const unsigned long long* __restrict__ bounds,
    int records,
    float* __restrict__ best_tangent,
    unsigned int* __restrict__ entries)
{
    int record = (int)((long long)blockIdx.x * blockDim.x + threadIdx.x);
    if (record >= records || enabled[record] == 0) return;
    // Roofs below the lowest aircraft this receiver sees IN THEIR OWN DIRECTION are skipped a
    // grid cell at a time (`horizon_edge_cannot_screen`, nearest local-cell point and tallest
    // referenced roof; non-negative floors only). An edge may be named from several
    // supercover cells, but its copy in the cell containing the actual ray crossing has the
    // necessary terrain, height, range and direction bounds. Pruning another copy cannot
    // remove that one; SPEC §12.4 gives the proof and complete byte-domain evidence.
    const float* floor_of_record = reinterpret_cast<const float*>(bounds[BOUNDS_FLOOR_OF_RECORD]);
    const float* all_group_floor = reinterpret_cast<const float*>(bounds[BOUNDS_GROUP_FLOOR]);
    const float* cell_top = reinterpret_cast<const float*>(bounds[BOUNDS_CELL_TOP]);
    const long long* cell_top_rect = reinterpret_cast<const long long*>(bounds[BOUNDS_CELL_TOP_RECT]);
    float record_floor = floor_of_record[record];
    bool prune = record_floor > BOUND_NO_FLOOR;
    unsigned int pixel = pixel_of_record[record];
    int py = pixel >> TPX_SHIFT;
    int px = pixel & TPX_MASK;
    double receiver_lat = receiver_lat_lon[py];
    double receiver_lon = receiver_lat_lon[TPX + px];
    double query_m_per_deg_lon = receiver_lat_lon[2 * TPX + py];
    double receiver_alt_m = (double)receiver_altitude[pixel];
    float receiver_alt = (float)receiver_alt_m;
    const float* group_floor = all_group_floor
        + ((long long)(py / LOWEST_SOURCE_TANGENT_BLOCK_PX) * BOUND_BLOCKS_PER_AXIS
           + px / LOWEST_SOURCE_TANGENT_BLOCK_PX) * BOUND_GROUPS;
    const double* directions =
        reinterpret_cast<const double*>(environment[BUILDING_ENV_DIRECTIONS]);
    const double* grid_geometry =
        reinterpret_cast<const double*>(environment[BUILDING_ENV_GRID_GEOMETRY]);
    const unsigned long long* grid_layout =
        reinterpret_cast<const unsigned long long*>(environment[BUILDING_ENV_GRID_LAYOUT]);
    const unsigned int* cell_starts =
        reinterpret_cast<const unsigned int*>(environment[BUILDING_ENV_CELL_STARTS]);
    const unsigned int* edge_refs =
        reinterpret_cast<const unsigned int*>(environment[BUILDING_ENV_EDGE_REFS]);
    const float* edges = reinterpret_cast<const float*>(environment[BUILDING_ENV_EDGES]);
    const unsigned char* edge_is_building =
        reinterpret_cast<const unsigned char*>(environment[BUILDING_ENV_EDGE_IS_BUILDING]);
    const float sector_width = (float)(TAU_D / BUILDING_LOCAL_SECTORS);

    for (unsigned long long index = 0; index < environment[BUILDING_ENV_INDEX_COUNT]; index++) {
        const double* geometry = grid_geometry + index * BUILDING_GRID_GEOMETRY_STRIDE;
        const unsigned long long* layout = grid_layout + index * BUILDING_GRID_LAYOUT_STRIDE;
        double index_m_per_deg_lon = geometry[2];
        double cell_m = geometry[3];
        double receiver_x = (receiver_lon - geometry[1]) * index_m_per_deg_lon;
        double receiver_y = (receiver_lat - geometry[0]) * M_LAT;
        double radius_x = BUILDING_LOCAL_MAX_M_D * index_m_per_deg_lon / query_m_per_deg_lon;
        double radius_y = BUILDING_LOCAL_MAX_M_D * M_LAT / AIRCRAFT_M_LAT;
        long long cols = (long long)layout[0];
        long long rows = (long long)layout[1];
        long long cx0 = (long long)floor((receiver_x - radius_x - geometry[4]) / cell_m);
        long long cx1 = (long long)floor((receiver_x + radius_x - geometry[4]) / cell_m);
        long long cy0 = (long long)floor((receiver_y - radius_y - geometry[5]) / cell_m);
        long long cy1 = (long long)floor((receiver_y + radius_y - geometry[5]) / cell_m);
        if (cx1 < 0 || cy1 < 0 || cx0 > cols - 1 || cy0 > rows - 1) continue;
        cx0 = cx0 < 0 ? 0 : cx0;
        cy0 = cy0 < 0 ? 0 : cy0;
        cx1 = cx1 > cols - 1 ? cols - 1 : cx1;
        cy1 = cy1 > rows - 1 ? rows - 1 : cy1;
        double longitude_scale = query_m_per_deg_lon / index_m_per_deg_lon;
        double latitude_scale = AIRCRAFT_M_LAT / M_LAT;
        unsigned long long starts_base = layout[2];
        unsigned long long refs_base = layout[3];
        unsigned long long edges_base = layout[4];
        const long long* rect = cell_top_rect + index * 4;
        float rect_x0 = (float)((geometry[4] - receiver_x) * longitude_scale);
        float rect_y0 = (float)((geometry[5] - receiver_y) * latitude_scale);
        float step_x = (float)(cell_m * longitude_scale);
        float step_y = (float)(cell_m * latitude_scale);

        for (long long cy = cy0; cy <= cy1; cy++) {
            unsigned long long row = (unsigned long long)cy * (unsigned long long)cols;
            for (long long cx = cx0; cx <= cx1; cx++) {
                unsigned long long cell = row + (unsigned long long)cx;
                unsigned int lo = cell_starts[starts_base + cell];
                unsigned int hi = cell_starts[starts_base + cell + 1];
                // An empty cell already costs one compare; the direction bound is only worth
                // forming for a cell that would otherwise be walked edge by edge.
                if (lo == hi) continue;
                // Only cells the tops kernel covered carry a roof bound; the rectangle is the
                // whole tile's roof reach, so this is every cell a receiver of it can visit.
                bool cell_prune =
                    prune && cx >= rect[0] && cx <= rect[1] && cy >= rect[2] && cy <= rect[3];
                float cell_top_rel_alt_m =
                    cell_prune ? cell_top[starts_base + cell] - receiver_alt : 0.0f;
                if (cell_prune && building_cell_cannot_screen(
                        cell_top_rel_alt_m, group_floor, record_floor,
                        rect_x0 + (float)cx * step_x, rect_y0 + (float)cy * step_y,
                        step_x, step_y)) continue;
                for (unsigned int ref_slot = lo; ref_slot < hi; ref_slot++) {
                    unsigned long long edge = edges_base + edge_refs[refs_base + ref_slot];
                    if (edge_is_building[edge] == 0) continue;
                    const float* source_edge = edges + edge * 5;
                    float x0 = (float)(((double)source_edge[0] - receiver_x) * longitude_scale);
                    float y0 = (float)(((double)source_edge[1] - receiver_y) * latitude_scale);
                    float x1 = (float)(((double)source_edge[2] - receiver_x) * longitude_scale);
                    float y1 = (float)(((double)source_edge[3] - receiver_y) * latitude_scale);
                    if (building_origin_to_segment_distance(x0, y0, x1, y1)
                        > (float)BUILDING_LOCAL_MAX_M_D) continue;
                    float angle0 = atan2f(y0, x0);
                    float angle1 = angle0 + building_wrap_pi(atan2f(y1, x1) - angle0);
                    float lo_angle = fminf(angle0, angle1);
                    float hi_angle = fmaxf(angle0, angle1);
                    long long first = (long long)ceilf(lo_angle / sector_width - 0.5f);
                    long long last = (long long)floorf(hi_angle / sector_width - 0.5f);
                    for (long long unwrapped_sector = first;
                         unwrapped_sector <= last;
                         unwrapped_sector++) {
                        int sector = (int)(unwrapped_sector % BUILDING_LOCAL_SECTORS);
                        if (sector < 0) sector += BUILDING_LOCAL_SECTORS;
                        double sin_angle_d = directions[2 * sector];
                        double cos_angle_d = directions[2 * sector + 1];
                        float range_m = building_intersection_range(
                            (float)cos_angle_d * (float)BUILDING_LOCAL_MAX_M_D,
                            (float)sin_angle_d * (float)BUILDING_LOCAL_MAX_M_D,
                            x0, y0, x1, y1);
                        if (range_m <= (float)BUILDING_MIN_EDGE_RANGE_M_D) continue;
                        // The crossing's own range and sector make the same test sharper than
                        // the cell rectangle could: it spares the DEM read behind every roof
                        // that is still below the lowest aircraft in exactly its direction.
                        if (cell_prune) {
                            float crossing_floor =
                                group_floor[sector / (BUILDING_LOCAL_SECTORS / BOUND_GROUPS)];
                            if (crossing_floor >= 0.0f
                                && horizon_edge_cannot_screen(
                                    cell_top_rel_alt_m, range_m, crossing_floor)) continue;
                        }
                        double edge_lat = receiver_lat
                            + sin_angle_d * (double)range_m / AIRCRAFT_M_LAT;
                        double edge_lon = receiver_lon
                            + cos_angle_d * (double)range_m / query_m_per_deg_lon;
                        double edge_rel_alt_m = building_tile_elevation(
                            environment, inner_elevation, tile_bbox, edge_lat, edge_lon)
                            + (double)source_edge[4] - receiver_alt_m;
                        float tangent = (float)(edge_rel_alt_m / (double)range_m);
                        int band = building_range_band(range_m);
                        unsigned long long entry =
                            ((unsigned long long)sector * BUILDING_LOCAL_BANDS + band)
                            * (unsigned long long)records + (unsigned int)record;
                        if ((entries[entry] & 0xffffu) == 0 || tangent > best_tangent[entry]) {
                            best_tangent[entry] = tangent;
                            entries[entry] =
                                (unsigned int)ceil((double)range_m * BUILDING_RANGE_SCALE_D);
                        }
                    }
                }
            }
        }
    }
}

extern "C" __global__ void airborne_building_horizon_pack(
    int records,
    const float* __restrict__ best_tangent,
    unsigned int* __restrict__ entries,
    unsigned short* __restrict__ local_max_tangent_bits)
{
    long long item = (long long)blockIdx.x * blockDim.x + threadIdx.x;
    long long count = (long long)records * BUILDING_LOCAL_SECTORS;
    if (item >= count) return;
    int record = (int)(item % records);
    int sector = (int)(item / records);
    float best = -BOUND_FLOAT_MAX;
    bool found = false;
    for (int band = 0; band < BUILDING_LOCAL_BANDS; band++) {
        unsigned long long entry =
            ((unsigned long long)sector * BUILDING_LOCAL_BANDS + band)
            * (unsigned long long)records + record;
        unsigned int range_q = entries[entry] & 0xffffu;
        if (range_q == 0) continue;
        float tangent = best_tangent[entry];
        entries[entry] = ((unsigned int)building_tangent_floor((double)tangent) << 16) | range_q;
        best = fmaxf(best, tangent);
        found = true;
    }
    local_max_tangent_bits[item] =
        found ? building_tangent_floor((double)best) : 0xffffu;
}

extern "C" __global__ void airborne_building_horizon_global_max(
    int records,
    const unsigned short* __restrict__ local_max_tangent_bits,
    unsigned short* __restrict__ global_max_tangent_bits)
{
    int record = (int)((long long)blockIdx.x * blockDim.x + threadIdx.x);
    if (record >= records) return;
    float best = -BOUND_FLOAT_MAX;
    bool found = false;
    for (int sector = 0; sector < BUILDING_LOCAL_SECTORS; sector++) {
        unsigned short bits = local_max_tangent_bits[(long long)sector * records + record];
        if (bits == 0xffffu) continue;
        best = fmaxf(best, __uint_as_float((unsigned int)bits << 16));
        found = true;
    }
    global_max_tangent_bits[record] = found ? building_tangent_floor((double)best) : 0xffffu;
}

extern "C" __global__ void airborne_building_horizon_mark_empty(
    int records,
    unsigned short* __restrict__ global_max_tangent_bits,
    unsigned short* __restrict__ local_max_tangent_bits)
{
    long long item = (long long)blockIdx.x * blockDim.x + threadIdx.x;
    long long count = (long long)records * BUILDING_LOCAL_SECTORS;
    if (item >= count) return;
    int record = (int)(item % records);
    int sector = (int)(item / records);
    local_max_tangent_bits[item] = 0xffffu;
    if (sector == 0) global_max_tangent_bits[record] = 0xffffu;
}

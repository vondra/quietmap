// Height-conditioned building-horizon pruning inputs, built on-device per tile: the lowest
// source tangent per receiver block AND azimuth group, a DEM max pyramid over the tile's
// terrain sampler, and from them the local roof-top proxy of every obstacle grid cell. Mirrors
// noise-compute
// `screening_bounds.rs` (`lowest_source_tangent`, `horizon_edge_cannot_screen`); the constants
// come from build.rs.
#pragma once

#if !defined(LOWEST_SOURCE_TANGENT_BLOCK_PX) || !defined(LOWEST_SOURCE_TANGENT_GROUPS) || \
    !defined(LOWEST_SOURCE_TANGENT_MARGIN_REL_D) || \
    !defined(LOWEST_SOURCE_TANGENT_MARGIN_ABS_D) || \
    !defined(LOWEST_SOURCE_TANGENT_RANGE_MARGIN_M_D) || \
    !defined(LOWEST_SOURCE_TANGENT_ANGLE_MARGIN_RAD_D) || !defined(BUILDING_ENV_CELL_MAX_H)
#error "airborne screening bound constants must be injected by build.rs"
#endif
// u64 slots of the per-tile bounds table `ScreeningBoundsDev::table` (device pointers).
#define BOUNDS_FLOOR_OF_RECORD 0
#define BOUNDS_GROUP_FLOOR 1
#define BOUNDS_CELL_TOP 2
#define BOUNDS_CELL_TOP_RECT 3
#define BOUND_BLOCKS_PER_AXIS (TPX / LOWEST_SOURCE_TANGENT_BLOCK_PX)
#define BOUND_BLOCK_THREADS (LOWEST_SOURCE_TANGENT_BLOCK_PX * LOWEST_SOURCE_TANGENT_BLOCK_PX)
#define BOUND_MARGIN_REL ((float)LOWEST_SOURCE_TANGENT_MARGIN_REL_D)
#define BOUND_MARGIN_ABS ((float)LOWEST_SOURCE_TANGENT_MARGIN_ABS_D)
#define BOUND_RANGE_MARGIN_M ((float)LOWEST_SOURCE_TANGENT_RANGE_MARGIN_M_D)
#define BOUND_ANGLE_MARGIN ((float)LOWEST_SOURCE_TANGENT_ANGLE_MARGIN_RAD_D)
#define BOUND_GROUPS LOWEST_SOURCE_TANGENT_GROUPS
#define BOUND_GROUPS_PER_TAU ((float)(BOUND_GROUPS / TAU_D))
#define BOUND_FLOAT_MAX 3.4028234663852886e38f
#define BOUND_NO_FLOOR (-BOUND_FLOAT_MAX)
// Lattice-cell slack around every DEM max query: the halo bilinear reads the lattice points
// within one cell of a sample, and sample positions are rounded far below one cell.
#define DEM_QUERY_MARGIN_CELLS 1.5
// Cover the f64 halo interpolation plus the f32 DEM + roof - receiver accumulation. One
// centimetre is over ten ulps even at the highest terrestrial DEM + allowed roof top.
#define DEM_MAX_ROUNDING_M 0.01f
// Metre slack on the local-cell nearest range (f32 projection is about 1e-4 m at 512 m).
#define CELL_RANGE_MARGIN_M 0.05f
// Receivers this close to the antimeridian have sub-segment projections that wrap in
// longitude, which the block bound below does not follow; they keep their full horizons.
#define ANTIMERIDIAN_FLOOR_FREE_DEG 179.8

// noise-compute `horizon_edge_cannot_screen`.
__device__ __forceinline__ bool horizon_edge_cannot_screen(
    float top_rel_alt_m, float nearest_range_m, float lowest_source_tangent)
{
    return top_rel_alt_m <= lowest_source_tangent * nearest_range_m;
}

// noise-compute `lowest_source_tangent` for one sub-segment and block, f32 like the kernel.
__device__ __forceinline__ float lowest_source_tangent_bound(
    float source_min_alt, float block_max_alt, float centre_to_locus, float half_diag)
{
    float numerator = source_min_alt - block_max_alt;
    return numerator >= 0.0f
        ? numerator / ((centre_to_locus + half_diag) * (1.0f + BOUND_MARGIN_REL) + BOUND_RANGE_MARGIN_M)
        : numerator / fmaxf((centre_to_locus - half_diag) * (1.0f - BOUND_MARGIN_REL) - BOUND_RANGE_MARGIN_M,
                            1.0e-3f);
}

// Order-preserving float→unsigned key, so `atomicMin` on unsigned words is a float minimum.
// A minimum is order-independent, so the reduction stays bit-deterministic.
__device__ __forceinline__ unsigned int float_min_key(float value) {
    unsigned int bits = __float_as_uint(value);
    return (bits & 0x80000000u) ? ~bits : (bits | 0x80000000u);
}
__device__ __forceinline__ float float_from_min_key(unsigned int key) {
    return __uint_as_float((key & 0x80000000u) ? (key & 0x7fffffffu) : ~key);
}

// Shortest signed turn between two directions, so an arc between two endpoint angles is the
// one the segment itself spans and never its 2π complement.
__device__ __forceinline__ float bound_wrap_pi(float angle) {
    angle = fmodf(angle, (float)TAU_D);
    if (angle > (float)PI_D) return angle - (float)TAU_D;
    if (angle <= -(float)PI_D) return angle + (float)TAU_D;
    return angle;
}

// Max over the pyramid cells covering lattice rows `rf_lo..rf_hi` and columns `cf_lo..cf_hi`
// (level-0 fractional coordinates, clamped to the halo like the sampler clamps). `layout`
// holds `(offset, rows, cols)` per level; the level whose cell is at least the span reads at
// most a 3×3 window.
__device__ __forceinline__ float dem_pyramid_max(
    const float* pyramid, const unsigned int* layout, int levels,
    double rf_lo, double rf_hi, double cf_lo, double cf_hi)
{
    long long rows0 = layout[1], cols0 = layout[2];
    long long r0 = min(max((long long)floor(rf_lo), 0LL), rows0 - 1);
    long long r1 = min(max((long long)ceil(rf_hi), 0LL), rows0 - 1);
    long long c0 = min(max((long long)floor(cf_lo), 0LL), cols0 - 1);
    long long c1 = min(max((long long)ceil(cf_hi), 0LL), cols0 - 1);
    long long span = max(r1 - r0, c1 - c0);
    int level = 0;
    while (level < levels - 1 && (1LL << level) < span) level++;
    const float* base = pyramid + layout[3 * level];
    long long cols = layout[3 * level + 2];
    float best = -BOUND_FLOAT_MAX;
    for (long long r = r0 >> level; r <= (r1 >> level); r++) {
        for (long long c = c0 >> level; c <= (c1 >> level); c++) {
            best = fmaxf(best, base[r * cols + c]);
        }
    }
    return best;
}

// Level 0 of the tile's DEM max pyramid: the halo lattice, raised around the tile by the inner
// pixels `building_tile_elevation` prefers inside the tile bbox (nearest pixel), so every value
// that sampler can return at a point is bounded by the lattice points within one cell of it.
extern "C" __global__ void airborne_dem_pyramid_level0(
    const unsigned long long* __restrict__ environment,
    const float* __restrict__ inner_elevation,
    const double* __restrict__ tile_bbox,
    float* __restrict__ level0)
{
    const double* meta = reinterpret_cast<const double*>(environment[BUILDING_ENV_DEM_META]);
    const float* elevation =
        reinterpret_cast<const float*>(environment[BUILDING_ENV_DEM_ELEVATION]);
    long long cols = (long long)environment[BUILDING_ENV_DEM_COLS];
    long long rows = (long long)environment[BUILDING_ENV_DEM_ROWS];
    long long i = (long long)blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= rows * cols) return;
    float value = elevation[i];
    double cell_deg = 1.0 / meta[2];
    double lat = meta[0] + (double)(i / cols) * cell_deg;
    double lon = meta[1] + (double)(i % cols) * cell_deg;
    double lat_lo = lat - cell_deg, lat_hi = lat + cell_deg;
    double lon_lo = lon - cell_deg, lon_hi = lon + cell_deg;
    if (lat_hi >= tile_bbox[0] && lat_lo <= tile_bbox[1]
        && lon_hi >= tile_bbox[2] && lon_lo <= tile_bbox[3]) {
        double lat_span = tile_bbox[1] - tile_bbox[0];
        double lon_span = tile_bbox[3] - tile_bbox[2];
        int py0 = max((int)floor((tile_bbox[1] - lat_hi) / lat_span * TPX) - 1, 0);
        int py1 = min((int)floor((tile_bbox[1] - lat_lo) / lat_span * TPX) + 1, TPX - 1);
        int px0 = max((int)floor((lon_lo - tile_bbox[2]) / lon_span * TPX) - 1, 0);
        int px1 = min((int)floor((lon_hi - tile_bbox[2]) / lon_span * TPX) + 1, TPX - 1);
        for (int py = py0; py <= py1; py++) {
            for (int px = px0; px <= px1; px++) {
                value = fmaxf(value, inner_elevation[py * TPX + px]);
            }
        }
    }
    level0[i] = value;
}

// One pyramid level: each cell is the max of its up to four children one level down.
extern "C" __global__ void airborne_dem_pyramid_reduce(
    float* __restrict__ pyramid,
    unsigned int source_offset, unsigned int source_rows, unsigned int source_cols,
    unsigned int target_offset, unsigned int target_rows, unsigned int target_cols)
{
    unsigned int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= target_rows * target_cols) return;
    unsigned int r = 2 * (i / target_cols), c = 2 * (i % target_cols);
    const float* source = pyramid + source_offset;
    float best = source[r * source_cols + c];
    if (c + 1 < source_cols) best = fmaxf(best, source[r * source_cols + c + 1]);
    if (r + 1 < source_rows) {
        best = fmaxf(best, source[(r + 1) * source_cols + c]);
        if (c + 1 < source_cols) best = fmaxf(best, source[(r + 1) * source_cols + c + 1]);
    }
    pyramid[target_offset + i] = best;
}

// Lowest source tangent per receiver block and azimuth group: the minimum over this tile's
// near sub-segments whose source point can lie in that group, of noise-compute
// `lowest_source_tangent` (f32 after the f64 coordinate subtraction, exactly like
// `airborne_sel`; the margins absorb the rounding). One thread block per pixel block. A
// sub-segment's source point is its closest point to the receiver, so it lies on the segment
// and its direction lies in the arc the segment's endpoints span, widened by the direction a
// receiver anywhere in the block can add.
extern "C" __global__ void airborne_lowest_source_tangent(
    const double* __restrict__ rll, const float* __restrict__ rxa,
    const double* __restrict__ sll, const float* __restrict__ sf,
    const int* __restrict__ near_idx, int near_base, int near_count, int nreg,
    float* __restrict__ group_floor)
{
    // One group minimum per warp: 256 threads folding into 32 shared words would serialise on
    // the same addresses, and a minimum is associative, so each warp keeps its own row and the
    // rows are folded once at the end.
    __shared__ float shared[BOUND_BLOCK_THREADS];
    __shared__ unsigned int shared_group[BOUND_BLOCK_THREADS / 32][BOUND_GROUPS];
    __shared__ float metric_projection_ratio;
    const int B = LOWEST_SOURCE_TANGENT_BLOCK_PX;
    int px0 = (blockIdx.x % BOUND_BLOCKS_PER_AXIS) * B;
    int py0 = (blockIdx.x / BOUND_BLOCKS_PER_AXIS) * B;
    int t = threadIdx.x;
    unsigned int* warp_group = shared_group[t / 32];
    for (int g = t % 32; g < BOUND_GROUPS; g += 32) {
        warp_group[g] = float_min_key(BOUND_FLOAT_MAX);
    }
    shared[t] = rxa[(py0 + t / B) * TPX + px0 + t % B];
    __syncthreads();
    for (int stride = BOUND_BLOCK_THREADS / 2; stride > 0; stride >>= 1) {
        if (t < stride) shared[t] = fmaxf(shared[t], shared[t + stride]);
        __syncthreads();
    }
    float block_max_alt = shared[0];
    __syncthreads();

    double lat_c = 0.5 * (rll[py0] + rll[py0 + B - 1]);
    double lon_c = 0.5 * (rll[TPX + px0] + rll[TPX + px0 + B - 1]);
    double mpdl = rll[2 * TPX + py0 + B / 2];
    double half_diag = 0.0;
    for (int corner = 0; corner < 4; corner++) {
        double dx = (rll[TPX + px0 + (corner & 1) * (B - 1)] - lon_c) * mpdl;
        double dy = (rll[py0 + (corner >> 1) * (B - 1)] - lat_c) * MLAT;
        half_diag = fmax(half_diag, hypot(dx, dy));
    }
    float hd = (float)half_diag;
    if (t == 0) {
        // For row/centre longitude-scale ratio q, the sharp maximum along-line shift from
        // anisotropic projection is |q - 1/q|/2 times the perpendicular line distance. Use
        // the actual rows in this block instead of spending the global 0.1% numeric margin
        // as a distance allowance; the final `(1 + margin)` below also rounds this value up.
        double worst = 0.0;
        for (int row = 0; row < B; row++) {
            double q = rll[2 * TPX + py0 + row] / mpdl;
            worst = fmax(worst, 0.5 * fabs(q - 1.0 / q));
        }
        metric_projection_ratio = (float)worst;
    }
    __syncthreads();

    for (int j = t; j < near_count; j += BOUND_BLOCK_THREADS) {
        int s = near_idx[near_base + j];
        const float* f = sf + s * 12;
        float x0 = (float)((sll[nreg + s] - lon_c) * mpdl);
        float y0 = (float)((sll[s] - lat_c) * MLAT);
        float dx = (float)((double)f[1] * mpdl);
        float dy = f[2];
        float len_sq = dx * dx + dy * dy;
        float x1 = x0 + dx, y1 = y0 + dy;
        float segment_d;
        float source_min_alt;
        bool direction_ambiguous = len_sq <= 2.0e-6f;
        if (direction_ambiguous) {
            // `airborne_sel` keeps the start point at <= 1e-6. This wider threshold also
            // covers a row-scaled segment that straddles that cutoff; in the ambiguous band,
            // bound both physical endpoints and conservatively supply every direction group.
            // The start distance can exceed the distance to this tiny segment by its length,
            // so an endpoint arc alone is not exact when it lies on the block half-diagonal.
            segment_d = hypotf(x0, y0);
            source_min_alt = fminf(f[0], f[0] + f[3]);
        } else {
            float t_c = -(x0 * dx + y0 * dy) / len_sq;
            float u = fminf(fmaxf(t_c, 0.0f), 1.0f);
            segment_d = hypotf(x0 + u * dx, y0 + u * dy);
            // In one metric a receiver inside the block moves the projected physical closest
            // point by at most hd / segment_length. Receiver rows use slightly different
            // longitude scales; `metric_projection_ratio` is the exact worst coefficient over
            // this block's rows. The perpendicular line distance of a displaced receiver is
            // at most the centre's finite-segment distance plus hd. Bound altitude over that
            // combined interval; using the whole segment's lower endpoint would let a remote
            // runway endpoint poison an otherwise high source.
            float metric_shift = (segment_d + hd) * metric_projection_ratio;
            float half_t = ((hd + metric_shift + BOUND_RANGE_MARGIN_M) / sqrtf(len_sq))
                         * (1.0f + BOUND_MARGIN_REL);
            float t_lo = fminf(fmaxf(t_c - half_t, 0.0f), 1.0f);
            float t_hi = fminf(fmaxf(t_c + half_t, 0.0f), 1.0f);
            source_min_alt = fminf(f[0] + t_lo * f[3], f[0] + t_hi * f[3]);
        }
        float bound = lowest_source_tangent_bound(
            source_min_alt, block_max_alt, segment_d, hd);
        unsigned int key = float_min_key(bound);
        // Direction arc of the segment from the block centre, widened by the angle a receiver
        // elsewhere in the block can see the same point under.
        float clearance = direction_ambiguous ? -1.0f : segment_d - hd;
        int first = 0, last = BOUND_GROUPS - 1;
        if (clearance > 0.0f) {
            float a0 = atan2f(y0, x0);
            float a1 = a0 + bound_wrap_pi(atan2f(y1, x1) - a0);
            float delta = asinf(fminf(hd / clearance, 1.0f)) + BOUND_ANGLE_MARGIN;
            float lo = fminf(a0, a1) - delta;
            float hi = fmaxf(a0, a1) + delta;
            if (hi - lo < (float)TAU_D) {
                first = (int)floorf(lo * BOUND_GROUPS_PER_TAU);
                last = (int)floorf(hi * BOUND_GROUPS_PER_TAU);
                if (last - first > BOUND_GROUPS - 1) last = first + BOUND_GROUPS - 1;
            }
        }
        for (int g = first; g <= last; g++) {
            int group = g % BOUND_GROUPS;
            atomicMin(&warp_group[(group < 0) ? group + BOUND_GROUPS : group], key);
        }
    }
    __syncthreads();
    if (t < BOUND_GROUPS) {
        unsigned int key = shared_group[0][t];
        for (int warp = 1; warp < BOUND_BLOCK_THREADS / 32; warp++) {
            key = min(key, shared_group[warp][t]);
        }
        float value = float_from_min_key(key);
        group_floor[(long long)blockIdx.x * BOUND_GROUPS + t] =
            value - fabsf(value) * BOUND_MARGIN_REL - BOUND_MARGIN_ABS;
    }
}

// Per-record floor: the minimum of its block's group floors for an exact-path pixel, none
// (-inf) where far sub-segments also query (the coarse lattice), where no near sub-segment
// exists, or beside the antimeridian. The roof scan reads this both as the "may prune" flag
// and as the floor for a cell that wraps every direction.
extern "C" __global__ void airborne_screening_floor(
    const unsigned int* __restrict__ pixel_of_record,
    const unsigned char* __restrict__ lattice_axis,
    const double* __restrict__ rll,
    const float* __restrict__ group_floor,
    int records, int near_count,
    float* __restrict__ floor_of_record)
{
    int record = (int)((long long)blockIdx.x * blockDim.x + threadIdx.x);
    if (record >= records) return;
    unsigned int pixel = pixel_of_record[record];
    int py = pixel >> TPX_SHIFT;
    int px = pixel & TPX_MASK;
    float lowest = BOUND_NO_FLOOR;
    if (near_count > 0 && !(lattice_axis[py] && lattice_axis[px])
        && fabs(rll[TPX + px]) < ANTIMERIDIAN_FLOOR_FREE_DEG) {
        int block = (py / LOWEST_SOURCE_TANGENT_BLOCK_PX) * BOUND_BLOCKS_PER_AXIS
                    + px / LOWEST_SOURCE_TANGENT_BLOCK_PX;
        lowest = BOUND_FLOAT_MAX;
        for (int group = 0; group < BOUND_GROUPS; group++) {
            lowest = fminf(lowest, group_floor[(long long)block * BOUND_GROUPS + group]);
        }
    }
    floor_of_record[record] = lowest;
}

// Local roof-top bound per obstacle grid cell: the DEM max around the cell plus its tallest
// referenced edge, or -inf for an empty cell. The neighbourhood scanner can test duplicate
// copies of an edge from other cells, but its supercover cell at the actual crossing provides
// the authoritative bound; pruning another copy cannot remove that one (SPEC §12.4).
extern "C" __global__ void airborne_building_cell_tops(
    const unsigned long long* __restrict__ environment,
    const float* __restrict__ pyramid, const unsigned int* __restrict__ layout, int levels,
    int index, long long cx0, long long cy0, long long cx_count, long long cy_count,
    float* __restrict__ cell_top)
{
    long long i = (long long)blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= cx_count * cy_count) return;
    long long cx = cx0 + i % cx_count;
    long long cy = cy0 + i / cx_count;
    const double* geometry = reinterpret_cast<const double*>(environment[BUILDING_ENV_GRID_GEOMETRY])
        + index * BUILDING_GRID_GEOMETRY_STRIDE;
    const unsigned long long* grid_layout =
        reinterpret_cast<const unsigned long long*>(environment[BUILDING_ENV_GRID_LAYOUT])
        + index * BUILDING_GRID_LAYOUT_STRIDE;
    const unsigned int* cell_starts =
        reinterpret_cast<const unsigned int*>(environment[BUILDING_ENV_CELL_STARTS]);
    const float* cell_max_h = reinterpret_cast<const float*>(environment[BUILDING_ENV_CELL_MAX_H]);
    const double* meta = reinterpret_cast<const double*>(environment[BUILDING_ENV_DEM_META]);
    unsigned long long slot = grid_layout[2] + (unsigned long long)(cy * (long long)grid_layout[0] + cx);
    float top = -BOUND_FLOAT_MAX;
    if (cell_starts[slot + 1] > cell_starts[slot]) {
        double cell_m = geometry[3];
        double x_lo = geometry[4] + (double)cx * cell_m;
        double y_lo = geometry[5] + (double)cy * cell_m;
        double lat_lo = geometry[0] + y_lo / M_LAT;
        double lat_hi = geometry[0] + (y_lo + cell_m) / M_LAT;
        double lon_lo = geometry[1] + x_lo / geometry[2];
        double lon_hi = geometry[1] + (x_lo + cell_m) / geometry[2];
        float dem = dem_pyramid_max(
            pyramid, layout, levels,
            (lat_lo - meta[0]) * meta[2] - DEM_QUERY_MARGIN_CELLS,
            (lat_hi - meta[0]) * meta[2] + DEM_QUERY_MARGIN_CELLS,
            (lon_lo - meta[1]) * meta[2] - DEM_QUERY_MARGIN_CELLS,
            (lon_hi - meta[1]) * meta[2] + DEM_QUERY_MARGIN_CELLS);
        top = dem + DEM_MAX_ROUNDING_M + cell_max_h[slot];
    }
    cell_top[slot] = top;
}

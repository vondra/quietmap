// GPU airborne aircraft-noise kernels (Doc 29 NPD plus receiver screening).
// Port of the CPU per-pixel path `aircraft::segment_sel_at_pixel_energy` →
// `segment_energy_kernel<false>` (noise-compute/src/emission/aircraft/).
//
// The kernels share one per-(seg, receiver) physics fn `airborne_sel`:
//   airborne_exact_screened  — NEAR: one thread per receiver pixel.
//   airborne_coarse_screened — FAR: one thread per far subsegment, accumulating
//                              onto a host-expanded coarse receiver lattice.
// Their compact pointer table supplies the device-built terrain and vector-
// building horizons without exceeding cudarc's launch-argument limit.
//
// The inherited Doc 29 arithmetic and packed-horizon queries stay fp32 apart
// from absolute lat/lon subtraction. CPU-reference parity is enforced on the
// complete encoded tiles, where the accepted difference is one 0.5 dB quantum.
//
// cudarc tuple launch caps ~12 args, so inputs are packed into a few buffers:
//   rll  f64[3*TPX]  = receiver lat[0..TPX] | lon[TPX..2TPX] | m_per_deg_lon[2TPX..3TPX]
//   rxa  f32[TPX*TPX]= receiver elevation per pixel (row-major py*TPX+px)
//   sll  f64[2*N]    = per-seg start_lat[s] | start_lon[N+s]  (absolute coords)
//   sf   f32[12*N]   = per-seg [start_alt,d_lon,sdy,sdz,dv,d_bar,di_a,di_b,di_c,
//                               reach_sq,tcut_start,tcut_end] (stride 12)
//   si   i32[4*N]    = per-seg [inst,class_idx,is_dep,period] (stride 4)
//   npd  f32[2*NC*(NB+1)] = NPD SEL LUT, approach[0..] | departure[NC*(NB+1)..]

// `build.rs` passes -DTPX from raster_reader::TILE_PX, exactly as it does for
// surface CUDA kernel. This guard is what makes that injection WIN: unguarded, the
// hand-copied 512 below silently shadowed it, so a TILE_PX change would have
// forked the two kernels with no compile error — the same silent-out-of-bounds
// hazard the 2026-08-04 audit fixed in the surface kernel and missed here. The fallback
// exists only for a bare `nvcc kernels/airborne.cu` syntax check.
#ifndef TPX
#define TPX 512
#endif
// log2(TPX). Derived, not copied: a mismatch would index the wrong receiver row.
#define TPX_SHIFT (31 - __builtin_clz(TPX))
#define TPX_MASK (TPX - 1)              // pix & TPX_MASK = px
#ifndef AIRCRAFT_M_LAT
#error "AIRCRAFT_M_LAT must be injected from aircraft/doc29.rs"
#endif
#define MLAT AIRCRAFT_M_LAT
#define LN10 2.302585092994046
#define LOG10_2 0.3010299956639812
#define FT_PER_M 3.28084
#define FARFIELD_M 7620.0f              // AIRCRAFT_FAR_FIELD_THRESHOLD_M
#define SEL_FLOOR 20.0f
// NPD_NC = NUM_CLASSES, injected by build.rs from the generated
// profiles_generated.rs so the LUT stride can never drift from the Rust
// upload; a hardcoded 14 mis-stepped every departure lookup when the
// 15th class landed.
#ifndef NPD_NC
#error "NPD_NC must be passed by build.rs (-DNPD_NC=<NUM_CLASSES>)"
#endif
#define NPD_NB 128                      // NPD_LUT_BINS
#define NPD_LOG_MIN 2.0f                // log10(100 ft)
#define NPD_INV_STEP (128.0f / 3.5f)    // NPD_LUT_BINS / (LOG_MAX-LOG_MIN)
#define INST_WING 0
#define INST_PROP 2

// Receiver-screening geometry and transfer slots are injected from their Rust
// authorities by build.rs. A bare nvcc invocation must supply the same defines.
#if !defined(TERRAIN_SECTORS) || !defined(TERRAIN_BANDS) || \
    !defined(BUILDING_LOCAL_SECTORS) || !defined(BUILDING_LOCAL_BANDS) || \
    !defined(TAN_SCALE_D) || !defined(TERRAIN_RANGE_SCALE_D) || \
    !defined(BUILDING_RANGE_SCALE_D)
#error "airborne horizon constants must be injected by build.rs"
#endif
#define PI_D 3.14159265358979323846264338327950288
#define HALF_PI_D 1.57079632679489661923132169163975144
#define TAU_D 6.28318530717958647692528676655900577
#if !defined(DIFFRACTION_SLOPE_D) || !defined(DIFFRACTION_GRAZING_DB_D) || \
    !defined(DIFFRACTION_CAP_DB_D)
#error "airborne diffraction constants must be injected by build.rs"
#endif

// u64 screening table slots, built by AirborneGpu::upload_receiver_screening.
#if !defined(SCREEN_RECORDS) || !defined(SCREEN_NREG) || \
    !defined(SCREEN_NEAR_BASE) || !defined(SCREEN_NEAR_COUNT) || \
    !defined(SCREEN_FAR0_BASE) || !defined(SCREEN_FAR0_COUNT) || \
    !defined(SCREEN_FAR1_BASE) || !defined(SCREEN_FAR1_COUNT) || \
    !defined(SCREEN_FAR2_BASE) || !defined(SCREEN_FAR2_COUNT) || \
    !defined(SCREEN_RECORD_OF_PIXEL) || \
    !defined(SCREEN_TERRAIN_ENTRIES) || !defined(SCREEN_TERRAIN_MAX_SIN_SQ) || \
    !defined(SCREEN_BUILDING_GLOBAL_MAX_TAN_Q) || \
    !defined(SCREEN_BUILDING_LOCAL_ENTRIES) || \
    !defined(SCREEN_BUILDING_LOCAL_MAX_TAN_Q)
#error "airborne screening ABI slots must be injected by build.rs"
#endif

// noise_compute fast_exp_f64, fp32 internals (copy of the surface-kernel fexp). ~1e-6 drift.
__device__ __forceinline__ float fexpf_nc(float x) {
    x = fminf(fmaxf(x, -87.0f), 88.0f);
    float n = roundf(x * 1.4426950408889634f);          // 1/ln2
    float r = x - n * 0.6931471805599453f;              // ln2
    float r2 = r * r;
    float poly = 1.0f + r + r2 * (0.5f + r * (1.0f/6.0f + r * (1.0f/24.0f + r * (1.0f/120.0f))));
    return poly * exp2f(n);
}

// doc29.rs fast_atan: Padé [3/2], max err ~0.003 rad.
__device__ __forceinline__ float fast_atan_small(float x) {
    float x2 = x * x;
    return x * (1.0f + 0.1827f * x2) / (1.0f + 0.5124f * x2);
}
__device__ __forceinline__ float fast_atan(float x) {
    if (fabsf(x) > 1.0f) {
        float s = (x >= 0.0f) ? 1.0f : -1.0f;
        return s * 1.5707963267948966f - fast_atan_small(1.0f / x);
    }
    return fast_atan_small(x);
}

#include "airborne_screening.cuh"
#include "airborne_building_horizon.cuh"
#include "airborne_terrain_horizon.cuh"

// doc29.rs fast_delta_f — ΔF finite-segment correction (Padé atan, log2 trick).
__device__ __forceinline__ float fast_delta_f(float q_m, float slen, float d_bar) {
    if (slen < 1.0f || d_bar < 1.0f) return 0.0f;
    float a1 = -q_m / d_bar;
    float a2 = -(q_m - slen) / d_bar;
    float g1 = a1 / (1.0f + a1 * a1) + fast_atan(a1);
    float g2 = a2 / (1.0f + a2 * a2) + fast_atan(a2);
    float f = (g2 - g1) * 0.3183098861837907f;          // 1/π
    return (10.0f * (float)LOG10_2) * log2f(fmaxf(f, 1e-15f));
}

// doc29.rs fast_lateral_attenuation — Λ = Γ(l)×Λ(β), Wing-mounted jets only.
__device__ __forceinline__ float fast_lat_atten(float rel_alt, float lateral_sq, int inst) {
    if (inst != INST_WING) return 0.0f;
    float lateral_m = sqrtf(lateral_sq); // only the Wing path needs it — skip the sqrt otherwise
    float beta = fast_atan(rel_alt / fmaxf(lateral_m, 0.01f)) * 57.29577951308232f; // →deg
    if (!(beta >= 0.0f && beta <= 50.0f)) return (beta < 0.0f) ? 10.857f : 0.0f;
    float gamma = (lateral_m <= 914.0f)
        ? 1.089f * (1.0f - fexpf_nc(-0.00274f * lateral_m)) : 1.0f;
    float lambda_beta = 1.137f - 0.0229f * beta + 9.72f * fexpf_nc(-0.142f * beta);
    return gamma * lambda_beta;
}

// npd.rs fast_npd_lookup against the per-class SEL LUT (NPD_NB+1 entries/class).
__device__ __forceinline__ float npd_lookup(const float* lut_base, int cls, float log_d) {
    const float* lut = lut_base + cls * (NPD_NB + 1);
    float t = fmaxf((log_d - NPD_LOG_MIN) * NPD_INV_STEP, 0.0f);
    int idx = min((int)t, NPD_NB - 1);
    float frac = t - (float)idx;
    return lut[idx] + frac * (lut[idx + 1] - lut[idx]);
}

// The receiver-relative start offsets and the projected segment direction: the ONLY f64
// arithmetic, one lat/lon → metre subtraction and the row's metres-per-degree product,
// rounded to f32 exactly as `prepare_row`/`segment_sel_at_pixel` do on the CPU. `ay` and
// `sdx` depend on the receiver ROW alone, so the callers form them once per row and share.
__device__ __forceinline__ float airborne_row_offset_north(double start_lat, double rx_lat) {
    return (float)((start_lat - rx_lat) * MLAT);
}
__device__ __forceinline__ float airborne_row_segment_dx(float d_lon, double mpdl) {
    return (float)((double)d_lon * mpdl);
}
__device__ __forceinline__ float airborne_offset_east(double start_lon, double rx_lon, double mpdl) {
    return (float)((start_lon - rx_lon) * mpdl);
}

// Shared per-(sub-seg, receiver) physics — the body of segment_energy_kernel<false>.
// Returns true + the SEL (dB) if the seg contributes at the receiver, false if any
// gate rejects. `f` = sf + s*12; `ax`/`ay`/`sdx` come from the row helpers above.
__device__ __forceinline__ bool airborne_sel(
    float ax, float ay, float sdx, const float* f, int cls, int is_dep, int inst,
    float rx_elev, const float* npd, const float* npd_dep, int pixel,
    const unsigned long long* screen, float* sel_out)
{
    float sdy = f[2], sdz = f[3], sz1 = f[0];
    float seg_len_sq = sdx * sdx + sdy * sdy;
    float inv_lsq = (seg_len_sq > 1e-6f) ? (1.0f / seg_len_sq) : 0.0f;
    float slen = fmaxf(sqrtf(seg_len_sq), 1.0f);

    float t = -(ax * sdx + ay * sdy) * inv_lsq;
    float cpx = ax + t * sdx, cpy = ay + t * sdy;
    float lateral_sq = cpx * cpx + cpy * cpy;
    float rel_alt = sz1 + t * sdz - rx_elev;
    float slant_sq = lateral_sq + rel_alt * rel_alt;

    if (slant_sq > f[9]) return false;                   // reach_sq
    if (t < 0.0f) { if (sz1 + t * sdz < f[10]) return false; }       // terrain_start_cut
    else if (t > 1.0f && sz1 + t * sdz < f[11]) return false;        // terrain_end_cut

    float d_p_m = sqrtf(slant_sq);
    float d_ft = fmaxf(d_p_m * (float)FT_PER_M, 100.0f);
    float log_d = log2f(d_ft) * (float)LOG10_2;
    float sel_npd = npd_lookup(is_dep ? npd_dep : npd, cls, log_d);
    float dv = f[4];

    float sel;
    float lambda = 0.0f;
    if (d_p_m > FARFIELD_M) {                            // CFFK fast path: only ΔF
        if (sel_npd + dv < SEL_FLOOR) return false;
        sel = sel_npd + dv + fast_delta_f(t * slen, slen, f[5]);
        if (sel < SEL_FLOOR) return false;
    } else {
        float df = fast_delta_f(t * slen, slen, f[5]);
        lambda = fast_lat_atten(rel_alt, lateral_sq, inst);
        float di = 0.0f;
        if (inst != INST_PROP) {
            float ra = fmaxf(rel_alt, 0.0f);
            float u2 = (ra * ra) / fmaxf(slant_sq, 1e-12f);
            float v2 = 1.0f - u2;
            float x = f[6] * v2 + u2;                    // di_a
            float den = f[8] * (4.0f * u2 * v2) + (v2 - u2) * (v2 - u2);  // di_c
            if (den > 0.0f && x > 0.0f)
                di = (10.0f * (float)LOG10_2) * (f[7] * log2f(x) - log2f(den));  // di_b
        }
        sel = sel_npd + dv + di - lambda + df;
        if (sel < SEL_FLOOR) return false;
    }

    float physical_t = fminf(fmaxf(t, 0.0f), 1.0f);
    float diffraction_db = receiver_screening_db(
        screen, pixel,
        cpx, cpy, rel_alt, slant_sq,
        ax + physical_t * sdx, ay + physical_t * sdy,
        sz1 + physical_t * sdz - rx_elev);
    if (d_p_m > FARFIELD_M) {
        sel -= diffraction_db;
    } else {
        sel -= fmaxf(diffraction_db - lambda, 0.0f);
    }
    if (sel < SEL_FLOOR) return false;
    *sel_out = sel;
    return true;
}

// Fine pixel sampled by coarse node i of an n-node lattice (CoarseLattice::coarse_pixel).
__device__ __forceinline__ int coarse_pixel(int n, int i) {
    return (i * (TPX - 1) + (n - 1) / 2) / (n - 1);
}

// Threads per exact-kernel block: a block is one run of a receiver row, so every thread
// shares `rx_lat`/`mpdl` and the row-only f64 values are staged once per block, not per pixel.
#define EXACT_BLOCK 256
#if TPX % EXACT_BLOCK != 0
#error "an exact-kernel block must stay inside one receiver row"
#endif

// Screened production NEAR: one tile per launch, thread per receiver pixel.
// The table supplies nreg + this tile's slice of the GPU-classified near CSR,
// as well as every receiver-horizon array used by `airborne_sel`. Launched with
// EXACT_BLOCK threads: each block stages the row-constant `ay`/`sdx` of a chunk of
// sub-segments cooperatively, then every thread only forms its own `ax` in f64 — three of
// the five f64 operations per pair gone, bit for bit the same operands and results.
extern "C" __global__ void airborne_exact_screened(
    const double* __restrict__ rll, const float* __restrict__ rxa,
    const double* __restrict__ sll, const float* __restrict__ sf, const int* __restrict__ si,
    const float* __restrict__ npd, const float* __restrict__ w,
    const int* __restrict__ near_idx, const unsigned long long* __restrict__ screen,
    float* __restrict__ out)
{
    __shared__ float row_ay[EXACT_BLOCK];
    __shared__ float row_sdx[EXACT_BLOCK];
    int pix = blockIdx.x * EXACT_BLOCK + threadIdx.x;
    int py = pix >> TPX_SHIFT, px = pix & TPX_MASK;
    double rx_lat = rll[py], rx_lon = rll[TPX + px], mpdl = rll[2 * TPX + py];
    float rx_elev = rxa[pix];
    const float* npd_dep = npd + NPD_NC * (NPD_NB + 1);
    float e[3] = {0.0f, 0.0f, 0.0f};
    int nreg = (int)screen[SCREEN_NREG];
    int base = (int)screen[SCREEN_NEAR_BASE];
    int count = (int)screen[SCREEN_NEAR_COUNT];
    for (int chunk = 0; chunk < count; chunk += EXACT_BLOCK) {
        int n = min(EXACT_BLOCK, count - chunk);
        __syncthreads();
        if ((int)threadIdx.x < n) {
            int s = near_idx[base + chunk + threadIdx.x];
            row_ay[threadIdx.x] = airborne_row_offset_north(sll[s], rx_lat);
            row_sdx[threadIdx.x] = airborne_row_segment_dx(sf[s * 12 + 1], mpdl);
        }
        __syncthreads();
        for (int j = 0; j < n; j++) {
            int s = near_idx[base + chunk + j];
            int cls = si[s * 4 + 1];
            float ax = airborne_offset_east(sll[nreg + s], rx_lon, mpdl);
            float sel;
            if (airborne_sel(ax, row_ay[j], row_sdx[j], sf + s * 12, cls, si[s*4+2], si[s*4+0],
                             rx_elev, npd, npd_dep, pix, screen, &sel)) {
                e[si[s * 4 + 3]] += fexpf_nc(sel * (float)LN10 * 0.1f) * w[cls];
            }
        }
    }
    out[pix * 3 + 0] = e[0]; out[pix * 3 + 1] = e[1]; out[pix * 3 + 2] = e[2];
}

// Coarse kernel shape: one block per (lattice row, part). The block stages the row-constant
// `ay`/`sdx` of a chunk of far sub-segments in shared memory exactly like the exact kernel,
// then thread (node j, lane l) folds the chunk's sub-segments l, l+lanes, … onto node j;
// the lanes of a node are summed in fixed order at the end. No atomics: run-to-run the same
// bits, one row projection per sub-segment instead of one per (sub-segment, node).
#define COARSE_BLOCK 256
#ifndef COARSE_TARGET_BLOCKS
#error "COARSE_TARGET_BLOCKS must be injected by build.rs (airborne.rs)"
#endif
// Parts each row's far list is cut into: enough (row, part) blocks to fill the device even on
// the 5×5 lattice. The host sizes the partial buffer with the same rule.
__device__ __forceinline__ int coarse_parts(int n) {
    return (COARSE_TARGET_BLOCKS + n - 1) / n;
}

// Screened production FAR: one tile + level per launch. `far_st` remains the classify pass's
// interleaved (segment,tile) array; this tile's base/count comes from the table and the tile
// word is skipped. `partial[(node * parts + part) * 3 + period]` is folded by
// `airborne_coarse_reduce_parts`.
extern "C" __global__ void airborne_coarse_screened(
    const double* __restrict__ rll, const float* __restrict__ rxa,
    const double* __restrict__ sll, const float* __restrict__ sf, const int* __restrict__ si,
    const float* __restrict__ npd, const float* __restrict__ w,
    const int* __restrict__ far_st, int level, int n,
    const unsigned long long* __restrict__ screen, float* __restrict__ partial)
{
    __shared__ float row_ay[COARSE_BLOCK];
    __shared__ float row_sdx[COARSE_BLOCK];
    __shared__ float sum[3][COARSE_BLOCK];
    int parts = coarse_parts(n);
    int ci = blockIdx.x / parts;
    int part = blockIdx.x % parts;
    int lanes = COARSE_BLOCK / n;
    int t = (int)threadIdx.x;
    int cj = t % n, lane = t / n;
    bool active = t < n * lanes;
    int base_slot = (level == 0) ? SCREEN_FAR0_BASE
                  : (level == 1) ? SCREEN_FAR1_BASE : SCREEN_FAR2_BASE;
    int count_slot = (level == 0) ? SCREEN_FAR0_COUNT
                   : (level == 1) ? SCREEN_FAR1_COUNT : SCREEN_FAR2_COUNT;
    int count = (int)screen[count_slot];
    int base = (int)screen[base_slot];
    int nreg = (int)screen[SCREEN_NREG];
    const float* npd_dep = npd + NPD_NC * (NPD_NB + 1);
    int py = coarse_pixel(n, ci);
    int px = coarse_pixel(n, cj);
    int pixel = py * TPX + px;
    double rx_lat = rll[py], rx_lon = rll[TPX + px], mpdl = rll[2 * TPX + py];
    float rx_elev = rxa[pixel];
    int j_begin = (int)((long long)count * part / parts);
    int j_end = (int)((long long)count * (part + 1) / parts);
    float e0 = 0.0f, e1 = 0.0f, e2 = 0.0f;
    for (int chunk = j_begin; chunk < j_end; chunk += COARSE_BLOCK) {
        int chunk_n = min(COARSE_BLOCK, j_end - chunk);
        __syncthreads();
        if (t < chunk_n) {
            int s = far_st[2 * (base + chunk + t)];
            row_ay[t] = airborne_row_offset_north(sll[s], rx_lat);
            row_sdx[t] = airborne_row_segment_dx(sf[s * 12 + 1], mpdl);
        }
        __syncthreads();
        if (!active) continue;
        for (int k = lane; k < chunk_n; k += lanes) {
            int s = far_st[2 * (base + chunk + k)];
            const float* f = sf + s * 12;
            int cls = si[s*4+1], is_dep = si[s*4+2], inst = si[s*4+0], period = si[s*4+3];
            float ax = airborne_offset_east(sll[nreg + s], rx_lon, mpdl);
            float sel;
            if (airborne_sel(ax, row_ay[k], row_sdx[k], f, cls, is_dep, inst, rx_elev, npd,
                             npd_dep, pixel, screen, &sel)) {
                // The original kernel's `atomicAdd` could not fuse the weight multiply into its
                // add, so the energy stays a rounded product (`__fmul_rn` never contracts) and
                // the node sum adds it: the same per-pair rounding, only in a fixed order.
                float energy = __fmul_rn(fexpf_nc(sel * (float)LN10 * 0.1f), w[cls]);
                if (period == 0) e0 += energy; else if (period == 1) e1 += energy; else e2 += energy;
            }
        }
    }
    sum[0][t] = e0;
    sum[1][t] = e1;
    sum[2][t] = e2;
    __syncthreads();
    if (t < n) {
        float s0 = 0.0f, s1 = 0.0f, s2 = 0.0f;
        for (int l = 0; l < lanes; l++) {
            s0 += sum[0][l * n + t];
            s1 += sum[1][l * n + t];
            s2 += sum[2][l * n + t];
        }
        long long slot = ((long long)(ci * n + t) * parts + part) * 3;
        partial[slot] = s0;
        partial[slot + 1] = s1;
        partial[slot + 2] = s2;
    }
}

// Fold the per-part sums of every node in part order.
extern "C" __global__ void airborne_coarse_reduce_parts(
    const float* __restrict__ partial, int nodes, int parts, float* __restrict__ out_coarse)
{
    int item = blockIdx.x * blockDim.x + threadIdx.x;
    if (item >= nodes * 3) return;
    int node = item / 3, period = item % 3;
    float total = 0.0f;
    for (int part = 0; part < parts; part++) total += partial[(node * parts + part) * 3 + period];
    out_coarse[item] = total;
}

// ─── M4 classify on the GPU: replace the per-tile O(nreg) candidate gate that ran
// single-threaded on each rayon worker (the wall that capped airborne GPU util — the
// device sat idle waiting for the CPU to decide near/far per seg). Thread per (tile,seg):
// recompute the SAME best-case-slant gate as `classify_tile` (airborne.rs) and counting-sort
// each seg into its tile's near / far[level] bucket entirely on device, so the O(nreg) loop
// never touches the CPU — only a tile×4 counts array crosses PCIe. The screened near/coarse
// kernels consume one tile's GPU-built CSR / far (seg,tile) slices at a time.
//
// Two passes (count then scatter) is the standard counting-sort: the gate is ~20 flops, cheap
// to recompute, and a per-thread temp would need tile·nreg ints (impossible at world scale).
// Slots are ranks, not atomic cursors, so the lists are in sub-segment order every run.
//
// meta_b f64[5*ntiles] per tile = [centre_lat, centre_lon, m_per_deg_lon, half_diag,
// tile_max_rx_alt]. Seg start_lat/lon from sll (f64 — the catastrophic-cancellation site);
// d_lon/sdy/sdz/start_alt/reach_sq from sf (f32 → promoted to f64 for the gate math, matching
// classify_tile's f64 arithmetic; measure parity and escalate to an f64 segment buffer only if
// borderline near/far/drop flips exceed tolerance — the physics kernel re-gates on reach_sq so a
// false-admit is merely wasted work, and near↔far flips at the 500 m boundary differ only by
// exact-vs-coarse interpolation, ≪0.5 dB).

#define NEAR_SLANT_SQ_D 250000.0        // NEAR_SLANT_M² (500²), airborne.rs:29
#define COARSE_BAND0 2000.0             // COARSE_BAND_M[0], airborne.rs:30
#define COARSE_BAND1 8000.0             // COARSE_BAND_M[1]

// Shared gate (mirrors classify_tile's body): bucket 0=near, 1/2/3=far level 0/1/2, -1=dropped.
__device__ __forceinline__ int airborne_classify_bucket(
    const double* meta, double start_lat, double start_lon, const float* f)
{
    double centre_lat = meta[0], centre_lon = meta[1], mpdl = meta[2];
    double half_diag = meta[3], tile_max_rx_alt = meta[4];
    double x1 = (start_lon - centre_lon) * mpdl;
    double y1 = (start_lat - centre_lat) * MLAT;
    double dx = (double)f[1] * mpdl;             // d_lon · m_per_deg_lon
    double dy = (double)f[2];                    // sdy
    double len_sq = dx * dx + dy * dy;
    double min_d_sq;
    if (len_sq < 1.0) {
        min_d_sq = x1 * x1 + y1 * y1;
    } else {
        double t_num = -(x1 * dx + y1 * dy);
        if (t_num <= 0.0) {
            min_d_sq = x1 * x1 + y1 * y1;
        } else if (t_num >= len_sq) {
            min_d_sq = (x1 + dx) * (x1 + dx) + (y1 + dy) * (y1 + dy);
        } else {
            double cross = dx * y1 - dy * x1;
            min_d_sq = (cross * cross) / len_sq;
        }
    }
    double horiz = sqrt(min_d_sq) - half_diag;
    if (horiz < 0.0) horiz = 0.0;
    double sz1 = (double)f[0], sdz = (double)f[3];
    double seg_min_alt = fmin(sz1, sz1 + sdz);
    double rel_alt = seg_min_alt - tile_max_rx_alt;
    if (rel_alt < 0.0) rel_alt = 0.0;
    double best_slant_sq = horiz * horiz + rel_alt * rel_alt;
    if (best_slant_sq > (double)f[9]) return -1;             // reach_sq
    if (best_slant_sq < NEAR_SLANT_SQ_D) return 0;
    double best_slant = sqrt(best_slant_sq);
    if (best_slant < COARSE_BAND0) return 1;
    if (best_slant < COARSE_BAND1) return 2;
    return 3;
}

// Both classify passes run one block per (tile, chunk of CLASSIFY_CHUNK consecutive
// sub-segments). A sub-segment's slot is its rank among the accepted ones of its bucket —
// warp ballots inside the block, per-chunk offsets across chunks — so the near CSR and the
// far lists come out in ascending sub-segment order every run, which is what makes the
// per-pixel and per-node f32 sums bit-reproducible. No fill cursors, no atomics on slots.
#define CLASSIFY_CHUNK 512
#define CLASSIFY_WARPS (CLASSIFY_CHUNK / 32)

// This thread's rank within its bucket in the block, and (in `bucket_total`) the block's
// accepted count per bucket. Uniform control flow: every lane joins every ballot.
__device__ __forceinline__ int classify_rank_in_block(
    int b, int bucket_total[4], int (*warp_count)[CLASSIFY_WARPS])
{
    int lane = threadIdx.x & 31, warp = threadIdx.x >> 5;
    unsigned int lanes_before = (1u << lane) - 1u;
    int rank = 0;
    for (int bb = 0; bb < 4; bb++) {
        unsigned int mask = __ballot_sync(0xffffffffu, b == bb);
        if (lane == 0) warp_count[bb][warp] = __popc(mask);
        if (b == bb) rank = __popc(mask & lanes_before);
    }
    __syncthreads();
    int before = 0;
    for (int bb = 0; bb < 4; bb++) {
        int total = 0;
        for (int wv = 0; wv < CLASSIFY_WARPS; wv++) {
            int c = warp_count[bb][wv];
            if (bb == b && wv < warp) before += c;
            total += c;
        }
        bucket_total[bb] = total;
    }
    return before + rank;
}

// Pass 1: per (tile, chunk) block, count each bucket → chunk_counts[(ti*4+b)*nchunks+chunk]
// and the tile totals counts[ti*4+b] (pre-zeroed).
extern "C" __global__ void airborne_classify_count(
    const double* __restrict__ meta_b, const double* __restrict__ sll,
    const float* __restrict__ sf, int nreg, int ntiles, int nchunks,
    int* __restrict__ chunk_counts, int* __restrict__ counts)
{
    __shared__ int warp_count[4][CLASSIFY_WARPS];
    int ti = blockIdx.x / nchunks;
    int chunk = blockIdx.x % nchunks;
    int s = chunk * CLASSIFY_CHUNK + (int)threadIdx.x;
    int b = (s < nreg && ti < ntiles)
        ? airborne_classify_bucket(meta_b + (long)ti * 5, sll[s], sll[nreg + s], sf + (long)s * 12)
        : -1;
    int bucket_total[4];
    classify_rank_in_block(b, bucket_total, warp_count);
    if (threadIdx.x < 4) {
        chunk_counts[(ti * 4 + threadIdx.x) * nchunks + chunk] = bucket_total[threadIdx.x];
        atomicAdd(&counts[ti * 4 + threadIdx.x], bucket_total[threadIdx.x]);
    }
}

// Exclusive prefix over each (tile, bucket)'s chunk counts, in place.
extern "C" __global__ void airborne_classify_chunk_offsets(
    int ntiles, int nchunks, int* __restrict__ chunk_counts)
{
    int row = blockIdx.x * blockDim.x + threadIdx.x;
    if (row >= ntiles * 4) return;
    int* counts = chunk_counts + (long)row * nchunks;
    int running = 0;
    for (int chunk = 0; chunk < nchunks; chunk++) {
        int c = counts[chunk];
        counts[chunk] = running;
        running += c;
    }
}

// Pass 2: scatter each seg into the screened kernels' buffers at its deterministic slot.
// `off` = 4 blocks of (ntiles+1): block 0 = near CSR (off[ti..ti+1]), blocks 1/2/3 =
// far level base offset per tile; `chunk_counts` now holds the per-chunk offsets.
extern "C" __global__ void airborne_classify_scatter(
    const double* __restrict__ meta_b, const double* __restrict__ sll,
    const float* __restrict__ sf, const int* __restrict__ off,
    const int* __restrict__ chunk_counts, int nreg, int ntiles, int nchunks,
    int* __restrict__ near_idx,
    int* __restrict__ far_st0, int* __restrict__ far_st1, int* __restrict__ far_st2)
{
    __shared__ int warp_count[4][CLASSIFY_WARPS];
    int ti = blockIdx.x / nchunks;
    int chunk = blockIdx.x % nchunks;
    int s = chunk * CLASSIFY_CHUNK + (int)threadIdx.x;
    int b = (s < nreg && ti < ntiles)
        ? airborne_classify_bucket(meta_b + (long)ti * 5, sll[s], sll[nreg + s], sf + (long)s * 12)
        : -1;
    int bucket_total[4];
    int rank = classify_rank_in_block(b, bucket_total, warp_count);
    if (b < 0) return;
    int pos = chunk_counts[(ti * 4 + b) * nchunks + chunk] + rank;
    if (b == 0) {
        near_idx[off[ti] + pos] = s;
    } else {
        int lvl = b - 1;
        int t1 = ntiles + 1;
        pos += off[(lvl + 1) * t1 + ti];
        int* fst = (lvl == 0) ? far_st0 : (lvl == 1) ? far_st1 : far_st2;
        fst[2 * pos] = s;
        fst[2 * pos + 1] = ti;
    }
}

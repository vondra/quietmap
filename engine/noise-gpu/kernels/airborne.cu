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

// Shared per-(sub-seg, receiver) physics — the body of segment_energy_kernel<false>.
// Returns true + the SEL (dB) if the seg contributes at the receiver, false if any
// gate rejects. `f` = sf + s*12; the ONLY f64 is the lat/lon → metre subtraction.
__device__ __forceinline__ bool airborne_sel(
    double start_lat, double start_lon, const float* f, int cls, int is_dep, int inst,
    double rx_lat, double rx_lon, double mpdl, float rx_elev,
    const float* npd, const float* npd_dep, int pixel,
    const unsigned long long* screen, float* sel_out)
{
    float ax = (float)((start_lon - rx_lon) * mpdl);
    float ay = (float)((start_lat - rx_lat) * MLAT);
    float sdx = (float)((double)f[1] * mpdl);            // d_lon
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

// Screened production NEAR: one tile per launch, thread per receiver pixel.
// The table supplies nreg + this tile's slice of the GPU-classified near CSR,
// as well as every receiver-horizon array used by `airborne_sel`.
extern "C" __global__ void airborne_exact_screened(
    const double* __restrict__ rll, const float* __restrict__ rxa,
    const double* __restrict__ sll, const float* __restrict__ sf, const int* __restrict__ si,
    const float* __restrict__ npd, const float* __restrict__ w,
    const int* __restrict__ near_idx, const unsigned long long* __restrict__ screen,
    float* __restrict__ out)
{
    int pix = blockIdx.x * blockDim.x + threadIdx.x;
    if (pix >= TPX * TPX) return;
    int py = pix >> TPX_SHIFT, px = pix & TPX_MASK;
    double rx_lat = rll[py], rx_lon = rll[TPX + px], mpdl = rll[2 * TPX + py];
    float rx_elev = rxa[pix];
    const float* npd_dep = npd + NPD_NC * (NPD_NB + 1);
    float e[3] = {0.0f, 0.0f, 0.0f};
    int nreg = (int)screen[SCREEN_NREG];
    int base = (int)screen[SCREEN_NEAR_BASE];
    int count = (int)screen[SCREEN_NEAR_COUNT];
    for (int j = 0; j < count; j++) {
        int s = near_idx[base + j];
        int cls = si[s * 4 + 1];
        float sel;
        if (airborne_sel(sll[s], sll[nreg + s], sf + s * 12, cls, si[s*4+2], si[s*4+0],
                         rx_lat, rx_lon, mpdl, rx_elev, npd, npd_dep, pix, screen, &sel)) {
            e[si[s * 4 + 3]] += fexpf_nc(sel * (float)LN10 * 0.1f) * w[cls];
        }
    }
    out[pix * 3 + 0] = e[0]; out[pix * 3 + 1] = e[1]; out[pix * 3 + 2] = e[2];
}

// Screened production FAR: one tile + level per launch, thread per classified
// far entry. `far_st` remains the classify pass's interleaved (segment,tile)
// array; this tile's base/count comes from the table and the tile word is skipped.
extern "C" __global__ void airborne_coarse_screened(
    const double* __restrict__ rll, const float* __restrict__ rxa,
    const double* __restrict__ sll, const float* __restrict__ sf, const int* __restrict__ si,
    const float* __restrict__ npd, const float* __restrict__ w,
    const int* __restrict__ far_st, int level, int n,
    const unsigned long long* __restrict__ screen, float* __restrict__ out_coarse)
{
    int j = blockIdx.x * blockDim.x + threadIdx.x;
    int base_slot = (level == 0) ? SCREEN_FAR0_BASE
                  : (level == 1) ? SCREEN_FAR1_BASE : SCREEN_FAR2_BASE;
    int count_slot = (level == 0) ? SCREEN_FAR0_COUNT
                   : (level == 1) ? SCREEN_FAR1_COUNT : SCREEN_FAR2_COUNT;
    int count = (int)screen[count_slot];
    if (j >= count) return;
    int base = (int)screen[base_slot];
    int s = far_st[2 * (base + j)];
    int nreg = (int)screen[SCREEN_NREG];
    const float* npd_dep = npd + NPD_NC * (NPD_NB + 1);
    double start_lat = sll[s], start_lon = sll[nreg + s];
    const float* f = sf + s * 12;
    int cls = si[s*4+1], is_dep = si[s*4+2], inst = si[s*4+0], period = si[s*4+3];
    float gw = w[cls];
    for (int ci = 0; ci < n; ci++) {
        int py = coarse_pixel(n, ci);
        double rx_lat = rll[py], mpdl = rll[2 * TPX + py];
        for (int cj = 0; cj < n; cj++) {
            int px = coarse_pixel(n, cj);
            double rx_lon = rll[TPX + px];
            float rx_elev = rxa[py * TPX + px];
            float sel;
            if (airborne_sel(start_lat, start_lon, f, cls, is_dep, inst,
                             rx_lat, rx_lon, mpdl, rx_elev, npd, npd_dep,
                             py * TPX + px, screen, &sel)) {
                float energy = fexpf_nc(sel * (float)LN10 * 0.1f) * gw;
                atomicAdd(&out_coarse[(ci * n + cj) * 3 + period], energy);
            }
        }
    }
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

// Pass 1: count each tile's near / far[level] candidates → counts[ntiles*4] (pre-zeroed).
extern "C" __global__ void airborne_classify_count(
    const double* __restrict__ meta_b, const double* __restrict__ sll,
    const float* __restrict__ sf, int nreg, int ntiles, int* __restrict__ counts)
{
    long gid = (long)blockIdx.x * blockDim.x + threadIdx.x;
    if (gid >= (long)ntiles * nreg) return;
    int ti = (int)(gid / nreg);
    int s = (int)(gid % nreg);
    int b = airborne_classify_bucket(meta_b + (long)ti * 5, sll[s], sll[nreg + s], sf + (long)s * 12);
    if (b >= 0) atomicAdd(&counts[ti * 4 + b], 1);
}

// Pass 2: scatter each seg into the screened kernels' buffers. `off` = 4 blocks of (ntiles+1):
// block 0 = near CSR (off[ti..ti+1]), blocks 1/2/3 =
// far level base offset per tile. `fill[ntiles*4]` = per-(tile,bucket) atomic cursor (pre-zeroed).
extern "C" __global__ void airborne_classify_scatter(
    const double* __restrict__ meta_b, const double* __restrict__ sll,
    const float* __restrict__ sf, const int* __restrict__ off, int* __restrict__ fill,
    int nreg, int ntiles, int* __restrict__ near_idx,
    int* __restrict__ far_st0, int* __restrict__ far_st1, int* __restrict__ far_st2)
{
    long gid = (long)blockIdx.x * blockDim.x + threadIdx.x;
    if (gid >= (long)ntiles * nreg) return;
    int ti = (int)(gid / nreg);
    int s = (int)(gid % nreg);
    int b = airborne_classify_bucket(meta_b + (long)ti * 5, sll[s], sll[nreg + s], sf + (long)s * 12);
    if (b < 0) return;
    if (b == 0) {
        int pos = off[ti] + atomicAdd(&fill[ti * 4], 1);
        near_idx[pos] = s;
    } else {
        int lvl = b - 1;
        int t1 = ntiles + 1;
        int pos = off[(lvl + 1) * t1 + ti] + atomicAdd(&fill[ti * 4 + 1 + lvl], 1);
        int* fst = (lvl == 0) ? far_st0 : (lvl == 1) ? far_st1 : far_st2;
        fst[2 * pos] = s;
        fst[2 * pos + 1] = ti;
    }
}

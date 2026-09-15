// Analytical aircraft energy, adapted from the existing CUDA backend; constants supplied by canonical Rust.
#pragma once
#define MLAT AIRCRAFT_M_LAT
#define LN10 2.302585092994046
#define LOG10_2 0.3010299956639812
#define SEL_FLOOR 20.0f
#define NPD_INV_STEP (NPD_NB / (NPD_LOG_MAX - NPD_LOG_MIN))
#define INST_WING 0
#define INST_PROP 2
#define PI_D 3.14159265358979323846264338327950288
#define HALF_PI_D 1.57079632679489661923132169163975144
#define TAU_D 6.28318530717958647692528676655900577

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

// Preserve coordinate subtraction in f64 before the energy path rounds to f32.
// Discrete screening sectors separately retain the canonical f64 CPA geometry.
__device__ __forceinline__ float airborne_row_offset_north(double start_lat, double rx_lat) {
    return (float)((start_lat - rx_lat) * MLAT);
}
__device__ __forceinline__ float airborne_row_segment_dx(float d_lon, double mpdl) {
    return (float)((double)d_lon * mpdl);
}
__device__ __forceinline__ float airborne_offset_east(double start_lon, double rx_lon, double mpdl) {
    double delta = start_lon - rx_lon;
    if (delta > 180.0) delta -= 360.0;
    if (delta < -180.0) delta += 360.0;
    return (float)(delta * mpdl);
}

// Shared per-(sub-seg, receiver) physics — the body of segment_energy_kernel<false>.
// Returns true + the SEL (dB) if the seg contributes at the receiver, false if any
// gate rejects. `f` = sf + s*12; `ax`/`ay`/`sdx` come from the row helpers above.
__device__ __forceinline__ bool airborne_sel(
    float ax, float ay, float sdx, const float* f, int cls, int is_dep, int inst,
    float rx_elev, const float* npd, const float* npd_dep, int pixel,
    const AirborneScreen& screen, const float* screen_geometry, float* sel_out)
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

    float diffraction_db = receiver_screening_db(
        screen, pixel,
        screen_geometry[0], screen_geometry[1], screen_geometry[2], screen_geometry[3],
        screen_geometry[4], screen_geometry[5], screen_geometry[6]);
    if (d_p_m > FARFIELD_M) {
        sel -= diffraction_db;
    } else {
        sel -= fmaxf(diffraction_db - lambda, 0.0f);
    }
    if (sel < SEL_FLOOR) return false;
    *sel_out = sel;
    return true;
}


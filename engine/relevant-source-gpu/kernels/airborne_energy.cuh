// Shared Doc29 aircraft energy; scalar precision and screening belong to the caller.
#pragma once
#define MLAT AIRCRAFT_M_LAT
#define LN10 2.302585092994046
#define LOG10_2 0.3010299956639812
#define SEL_FLOOR 20.0f
#define NPD_INV_STEP (NPD_NB / (NPD_LOG_MAX - NPD_LOG_MIN))
#define INST_PROP 2
#define PI_D 3.14159265358979323846264338327950288
#define HALF_PI_D 1.57079632679489661923132169163975144
#define TAU_D 6.28318530717958647692528676655900577

template<typename Real>
__device__ __forceinline__ Real aircraft_fast_exp(Real x) {
    x = fmin(fmax(x, Real(-87)), Real(88));
    Real n = round(x * Real(1.4426950408889634));
    Real r = x - n * Real(0.6931471805599453);
    Real r2 = r * r;
    Real poly = Real(1) + r + r2 * (Real(0.5) + r * (Real(1)/Real(6) + r * (Real(1)/Real(24) + r * (Real(1)/Real(120)))));
    return poly * exp2(n);
}

template<typename Real>
__device__ __forceinline__ Real fast_atan_small(Real x) {
    Real x2 = x * x;
    return x * (Real(1) + Real(0.1827) * x2) / (Real(1) + Real(0.5124) * x2);
}
template<typename Real>
__device__ __forceinline__ Real fast_atan(Real x) {
    if (fabs(x) > Real(1)) {
        Real sign = x >= Real(0) ? Real(1) : Real(-1);
        return sign * Real(HALF_PI_D) - fast_atan_small(Real(1) / x);
    }
    return fast_atan_small(x);
}

#include "airborne_screening.cuh"

template<typename Real>
__device__ __forceinline__ Real fast_delta_f(Real q_m, Real slen, Real d_lambda) {
    if (slen < Real(1) || d_lambda < Real(1)) return Real(0);
    Real a1 = -q_m / d_lambda;
    Real a2 = -(q_m - slen) / d_lambda;
    Real g1 = a1 / (Real(1) + a1 * a1) + fast_atan(a1);
    Real g2 = a2 / (Real(1) + a2 * a2) + fast_atan(a2);
    Real fraction = (g2 - g1) * Real(0.3183098861837907);
    return (Real(10) * Real(LOG10_2)) * log2(fmax(fraction, Real(1e-15)));
}

template<typename Real>
__device__ __forceinline__ Real fast_lat_atten(Real relative_alt, Real lateral_sq) {
    Real lateral_m = sqrt(lateral_sq);
    Real beta = fast_atan(relative_alt / fmax(lateral_m, Real(0.01))) * Real(57.29577951308232);
    if (beta > Real(50)) return Real(0);
    Real gamma = lateral_m <= Real(914)
        ? Real(1.089) * (Real(1) - aircraft_fast_exp(Real(-0.00274) * lateral_m)) : Real(1);
    Real lambda = beta < Real(0) ? Real(10.857) : Real(1.137) - Real(0.0229) * beta + Real(9.72) * aircraft_fast_exp(Real(-0.142) * beta);
    return gamma * lambda;
}

template<typename Real>
__device__ __forceinline__ Real npd_row_lookup(const Real* base, int cls, int row, Real log_d) {
    const Real* lut = base + ((size_t)cls * NPD_NR + row) * (NPD_NB + 1);
    Real t = fmax((log_d - Real(NPD_LOG_MIN)) * Real(NPD_INV_STEP), Real(0));
    int index = min((int)t, NPD_NB - 1);
    Real fraction = t - Real(index);
    return lut[index] + fraction * (lut[index + 1] - lut[index]);
}

// Doc 29 Eq. 4-3: lerp between the bracketing power rows. w == 0 keeps the
// single-row read bit-exact (lo + 0 * (hi - lo) == lo for finite LUTs).
template<typename Real>
__device__ __forceinline__ Real npd_lookup(const Real* base, int cls, int row, Real w, Real log_d) {
    Real lo = npd_row_lookup(base, cls, row, log_d);
    Real hi = npd_row_lookup(base, cls, row + 1 < NPD_NR ? row + 1 : row, log_d);
    return lo + w * (hi - lo);
}

// Airborne input coordinates are stored f32; subtraction stays f64 before its existing f32 kernel.
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

template<typename Real, bool SCREENED, bool FLOOR = true>
__device__ __forceinline__ bool aircraft_sel(
    Real ax, Real ay, Real sdx, const Real* f, int cls, int departure, int installation, int power_row,
    Real receiver_altitude, const Real* npd, const Real* scaled_distance, int pixel,
    const AirborneScreen& screen, const float* screen_geometry, Real* sel_out, Real* free_sel_out = nullptr)
{
    Real sdy = f[2], sdz = f[3], start_altitude = f[0];
    Real length_sq = sdx * sdx + sdy * sdy;
    Real inverse = length_sq > Real(1e-6) ? Real(1) / length_sq : Real(0);
    Real length = fmax(sqrt(length_sq), Real(1));
    Real t = -(ax * sdx + ay * sdy) * inverse;
    Real cpx = ax + t * sdx, cpy = ay + t * sdy;
    Real lateral_sq = cpx * cpx + cpy * cpy;
    Real relative_altitude = start_altitude + t * sdz - receiver_altitude;
    Real slant_sq = lateral_sq + relative_altitude * relative_altitude;
    if (slant_sq > f[8]) return false;
    if (t < Real(0)) { if (start_altitude + t * sdz < f[9]) return false; }
    else if (t > Real(1) && start_altitude + t * sdz < f[10]) return false;

    Real distance = sqrt(slant_sq);
    Real feet = fmax(distance * Real(FT_PER_M), Real(100));
    Real log_d = log2(feet) * Real(LOG10_2);
    int operation_class = departure * NPD_NC + cls;
    Real sel_npd = npd_lookup(npd, operation_class, power_row, f[11], log_d) + f[12];
    if (FLOOR && sel_npd + f[4] + Real(0.4014) < Real(SEL_FLOOR)) return false;
    Real d_lambda = npd_lookup(scaled_distance, operation_class, power_row, f[11], log_d);
    Real finite = fast_delta_f(t * length, length, d_lambda);
    Real lambda = fast_lat_atten(relative_altitude, lateral_sq);
    Real installation_db = Real(0);
    if (installation != INST_PROP) {
        Real above = fmax(relative_altitude, Real(0));
        Real u2 = above * above / fmax(slant_sq, Real(1e-12));
        Real v2 = Real(1) - u2;
        Real x = f[5] * v2 + u2;
        Real denominator = f[7] * (Real(4) * u2 * v2) + (v2 - u2) * (v2 - u2);
        if (denominator > Real(0) && x > Real(0))
            installation_db = (Real(10) * Real(LOG10_2)) * (f[6] * log2(x) - log2(denominator));
    }
    Real sel = sel_npd + f[4] + installation_db - lambda + finite;
    if (FLOOR && sel < Real(SEL_FLOOR)) return false;
    if (free_sel_out) *free_sel_out = sel;
    if constexpr (SCREENED) {
        Real diffraction = Real(receiver_screening_db(screen, pixel,
            screen_geometry[0], screen_geometry[1], screen_geometry[2], screen_geometry[3],
            screen_geometry[4], screen_geometry[5], screen_geometry[6]));
        sel -= fmax(diffraction - lambda, Real(0));
        if (FLOOR && sel < Real(SEL_FLOOR)) return false;
    }
    *sel_out = sel;
    return true;
}

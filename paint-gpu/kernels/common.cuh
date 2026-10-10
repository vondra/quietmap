// The painter's device code in single precision: types, the units every part shares and small
// helpers. Every function cites the Rust it reproduces; the Rust (f64) is the reference, and
// `qm-paint-gpu check` measures this port against it on random pairs. Where f32 would cancel
// (path differences, arcs, absolute coordinates) the formulas are rewritten to stay exact to f32's
// precision; each such place says so.

typedef long long i64;
typedef unsigned long long u64;
typedef int i32;
typedef unsigned int u32;
typedef short i16;
typedef unsigned short u16;
typedef unsigned char u8;

#define QM_INFINITY (__int_as_float(0x7f800000))
#define QM_PI 3.14159265358979323846f
#define QM_TAU 6.28318530717958647692f
#define QM_LN_10 2.30258509299404568402f

// physics/src/bands.rs
#define BANDS 8
#define PERIODS 3
__constant__ float BAND_FREQUENCY_HZ[BANDS] = {63.0f, 125.0f, 250.0f, 500.0f, 1000.0f, 2000.0f, 4000.0f, 8000.0f};
#define SPEED_OF_SOUND_M_PER_S 340.0f

// Failures a pair reports instead of an answer; the host evaluates such a pair on the CPU.
#define FAILED_NOT_READ 1u      // a terrain or obstacles tile the CPU would refuse
#define FAILED_NO_TERRAIN 2u    // a terrain node without data
#define FAILED_CAPACITY 4u      // a per-thread buffer too small for this pair

// physics::bands::energy, amplitude
__device__ __forceinline__ float energy_of_level(float level_db) {
    return expf(level_db * (QM_LN_10 / 10.0f));
}
__device__ __forceinline__ float amplitude_of_level(float level_db) {
    return expf(level_db * (QM_LN_10 / 20.0f));
}

// f64::clamp
__device__ __forceinline__ float clampf(float value, float low, float high) {
    return fminf(fmaxf(value, low), high);
}

__device__ __forceinline__ int clampi(int value, int low, int high) {
    return value < low ? low : (value > high ? high : value);
}

// f64::rem_euclid
__device__ __forceinline__ float rem_euclidf(float value, float modulus) {
    float r = fmodf(value, modulus);
    return r < 0.0f ? r + fabsf(modulus) : r;
}
__device__ __forceinline__ i64 rem_euclid_i64(i64 value, i64 modulus) {
    i64 r = value % modulus;
    return r < 0 ? r + (modulus < 0 ? -modulus : modulus) : r;
}
__device__ __forceinline__ i64 div_euclid_i64(i64 value, i64 modulus) {
    i64 q = value / modulus;
    if (value % modulus < 0) {
        q = modulus > 0 ? q - 1 : q + 1;
    }
    return q;
}

// A point in the vertical propagation plane: horizontal distance from the source, altitude.
struct PlanePoint {
    float x;
    float z;
};

// |SO| + |OR| - |SR| of a triangle without the cancellation of the three lengths: with u = O - S,
// v = R - O and w = u + v, (|u| + |v|)^2 - |w|^2 = 2 (|u||v| - u.v) and |u||v| - u.v =
// (u x v)^2 / (|u||v| + u.v), so the excess is 2 (u x v)^2 / ((|u||v| + u.v)(|u| + |v| + |w|)).
// Exact to f32 when O lies ahead of S toward R (u.v > 0), as every diffraction point does; the
// direct difference otherwise (no cancellation there: the excess is large).
__device__ __forceinline__ float triangle_excess(PlanePoint s, PlanePoint o, PlanePoint r) {
    float ux = o.x - s.x, uz = o.z - s.z, vx = r.x - o.x, vz = r.z - o.z;
    float lu = sqrtf(ux * ux + uz * uz), lv = sqrtf(vx * vx + vz * vz);
    float wx = ux + vx, wz = uz + vz;
    float lw = sqrtf(wx * wx + wz * wz);
    float dot = ux * vx + uz * vz;
    if (dot <= 0.0f) return lu + lv - lw;
    float cross = ux * vz - uz * vx;
    return 2.0f * cross * cross / ((lu * lv + dot) * (lu + lv + lw));
}

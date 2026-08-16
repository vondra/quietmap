// Device mirror of noise-compute propagation::streaming_reduction.
//
// This header layers streaming decisions on the one package-2 transcendental
// contract. It must include, never copy, qm_shared_math.cuh. Every SRM-* label
// has the same expression order in Rust and CUDA.

#ifndef QM_STREAMING_REDUCTION_CUH
#define QM_STREAMING_REDUCTION_CUH

#include "qm_shared_math.cuh"

#ifndef BARRIER_ABI_VERSION
#error "BARRIER_ABI_VERSION must be injected from noise-gpu/src/lib.rs"
#endif
#ifndef BARRIER_STRIDE
#error "BARRIER_STRIDE must be injected from noise-gpu/src/lib.rs"
#endif
#ifndef SOURCE_SEGMENT_ABI_VERSION
#error "SOURCE_SEGMENT_ABI_VERSION must be injected from noise-gpu/src/lib.rs"
#endif
#ifndef SOURCE_SEGMENT_STRIDE
#error "SOURCE_SEGMENT_STRIDE must be injected from noise-gpu/src/lib.rs"
#endif
#ifndef LINE_KERNEL_ARGUMENT_COUNT
#error "LINE_KERNEL_ARGUMENT_COUNT must be injected from noise-gpu/src/lib.rs"
#endif

#if BARRIER_ABI_VERSION != 2 || BARRIER_STRIDE != 7
#error "stale barrier ABI"
#endif
#if SOURCE_SEGMENT_ABI_VERSION != 2 || SOURCE_SEGMENT_STRIDE != 5
#error "stale source-segment ABI"
#endif
#if LINE_KERNEL_ARGUMENT_COUNT != 12
#error "surface line launch must retain twelve physical arguments"
#endif

#ifdef __CUDACC__
#define SR_SQRT(x) __dsqrt_rn(x)
#define SR_TO_F32(x) __double2float_rn(x)
#else
#include <math.h>
#define SR_SQRT(x) sqrt(x)
#define SR_TO_F32(x) ((float)(x))
#endif

#define SR_WALL_SOURCE_TAG 0x8000000000000000ull
#define SR_WALL_OSM_ID_LIMIT 0x0000800000000000ll

typedef struct {
    double x;
    double y;
} qm_metric_vector;

enum qm_wedge_decision {
    QM_WEDGE_DOES_NOT_OWN = 0,
    QM_WEDGE_OWNS = 1,
    QM_WEDGE_NEAR_GUARDED_DEGENERATE = 2,
    QM_WEDGE_HARD_FAULT = 3,
};

QM_FN unsigned long long qm_barrier_candidate_tail_slot_offset(unsigned long long barrier_count) {
    const unsigned long long physical_rows = barrier_count > 0ull ? barrier_count : 1ull;
    return physical_rows * (unsigned long long)BARRIER_STRIDE;
}

QM_FN double qm_canonical_zero(double value) { return value == 0.0 ? 0.0 : value; }

QM_FN int qm_vector_is_finite(qm_metric_vector value) {
    return isfinite(value.x) && isfinite(value.y);
}

QM_FN int qm_vector_is_zero(qm_metric_vector value) {
    return value.x == 0.0 && value.y == 0.0;
}

QM_FN int qm_source_id_obstacle(unsigned long long ordinal, unsigned long long *out) {
    if (ordinal >= SR_WALL_SOURCE_TAG) {
        return 0;
    }
    *out = ordinal;
    return 1;
}

QM_FN int qm_source_id_wall(long long osm_id, unsigned short segment_bits,
                            unsigned long long *out) {
    if (osm_id < 0 || osm_id >= SR_WALL_OSM_ID_LIMIT) {
        return 0;
    }
    *out = SR_WALL_SOURCE_TAG | (((unsigned long long)osm_id) << 16) |
           (unsigned long long)segment_bits;
    return 1;
}

QM_FN double qm_orient(qm_metric_vector a, qm_metric_vector b) {
    // SRM-ORIENT-1
    const double positive = QM_MUL(a.x, b.y);
    const double negative = QM_MUL(a.y, b.x);
    return qm_canonical_zero(QM_SUB(positive, negative));
}

QM_FN double qm_dot(qm_metric_vector a, qm_metric_vector b) {
    // SRM-DOT-1
    const double x = QM_MUL(a.x, b.x);
    const double y = QM_MUL(a.y, b.y);
    return qm_canonical_zero(QM_ADD(x, y));
}

QM_FN int qm_same_ray(qm_metric_vector a, qm_metric_vector b, int *hard_fault) {
    if (!qm_vector_is_finite(a) || !qm_vector_is_finite(b)) {
        *hard_fault = 1;
        return 0;
    }
    if (qm_vector_is_zero(a) || qm_vector_is_zero(b)) {
        *hard_fault = 1;
        return 0;
    }
    *hard_fault = 0;
    return qm_orient(a, b) == 0.0 && qm_dot(a, b) > 0.0;
}

QM_FN int qm_direction_less(qm_metric_vector a, qm_metric_vector b, int *hard_fault) {
    if (!qm_vector_is_finite(a) || !qm_vector_is_finite(b) || qm_vector_is_zero(a) ||
        qm_vector_is_zero(b)) {
        *hard_fault = 1;
        return 0;
    }
    // SRM-DIRECTION-1
    const int a_upper = a.y > 0.0 || (a.y == 0.0 && a.x >= 0.0);
    const int b_upper = b.y > 0.0 || (b.y == 0.0 && b.x >= 0.0);
    *hard_fault = 0;
    if (a_upper != b_upper) {
        return a_upper;
    }
    const double turn = qm_orient(a, b);
    return turn != 0.0 && turn > 0.0;
}

QM_FN int qm_candidate_wedge_owns(qm_metric_vector a, qm_metric_vector b,
                                  qm_metric_vector node, float near_f32) {
    if (!qm_vector_is_finite(a) || !qm_vector_is_finite(b) ||
        !qm_vector_is_finite(node) || !isfinite(near_f32) || qm_vector_is_zero(node)) {
        return QM_WEDGE_HARD_FAULT;
    }
    if (qm_vector_is_zero(a) || qm_vector_is_zero(b)) {
        return near_f32 < 1.0f ? QM_WEDGE_NEAR_GUARDED_DEGENERATE : QM_WEDGE_HARD_FAULT;
    }

    const double turn = qm_orient(a, b);
    if (turn == 0.0) {
        const double alignment = qm_dot(a, b);
        if (alignment > 0.0) {
            int fault = 0;
            const int owns = qm_same_ray(a, node, &fault);
            return fault ? QM_WEDGE_HARD_FAULT
                         : (owns ? QM_WEDGE_OWNS : QM_WEDGE_DOES_NOT_OWN);
        }
        return alignment < 0.0 && near_f32 < 1.0f ? QM_WEDGE_NEAR_GUARDED_DEGENERATE
                                                  : QM_WEDGE_HARD_FAULT;
    }

    // SRM-WEDGE-1: physical endpoint a is inclusive, b is exclusive.
    if (turn < 0.0) {
        int fault = 0;
        const int node_is_start = qm_same_ray(a, node, &fault);
        if (fault) return QM_WEDGE_HARD_FAULT;
        const int node_is_end = qm_same_ray(b, node, &fault);
        if (fault) return QM_WEDGE_HARD_FAULT;
        if (node_is_start) return QM_WEDGE_OWNS;
        if (node_is_end) return QM_WEDGE_DOES_NOT_OWN;
    }
    const qm_metric_vector start = turn > 0.0 ? a : b;
    const qm_metric_vector end = turn > 0.0 ? b : a;
    int fault = 0;
    const int start_before_end = qm_direction_less(start, end, &fault);
    if (fault) return QM_WEDGE_HARD_FAULT;
    const int node_before_start = qm_direction_less(node, start, &fault);
    if (fault) return QM_WEDGE_HARD_FAULT;
    const int node_before_end = qm_direction_less(node, end, &fault);
    if (fault) return QM_WEDGE_HARD_FAULT;
    const int owns = start_before_end ? (!node_before_start && node_before_end)
                                      : (!node_before_start || node_before_end);
    return owns ? QM_WEDGE_OWNS : QM_WEDGE_DOES_NOT_OWN;
}

QM_FN int qm_origin_to_segment_distance_f32(qm_metric_vector a, qm_metric_vector b,
                                             float *out) {
    if (!qm_vector_is_finite(a) || !qm_vector_is_finite(b)) return 0;
    // SRM-NEAR-1
    const double vx = QM_SUB(b.x, a.x);
    const double vy = QM_SUB(b.y, a.y);
    const double vv = QM_ADD(QM_MUL(vx, vx), QM_MUL(vy, vy));
    const double projection = -QM_ADD(QM_MUL(a.x, vx), QM_MUL(a.y, vy));
    double t = vv > 0.0 ? QM_DIV(projection, vv) : 0.0;
    t = t < 0.0 ? 0.0 : (t > 1.0 ? 1.0 : t);
    const double qx = QM_ADD(a.x, QM_MUL(t, vx));
    const double qy = QM_ADD(a.y, QM_MUL(t, vy));
    const double distance = SR_SQRT(QM_ADD(QM_MUL(qx, qx), QM_MUL(qy, qy)));
    if (!isfinite(distance)) return 0;
    *out = SR_TO_F32(distance);
    return 1;
}

QM_FN int qm_radial_range_root(double d_perp, float near_f32, double *root, int *hard_fault) {
    const double b = (double)near_f32;
    if (!isfinite(d_perp) || !isfinite(b) || d_perp < 0.0) {
        *hard_fault = 1;
        return 0;
    }
    // SRM-ROOT-1
    if (d_perp >= b) {
        *hard_fault = 0;
        return 0;
    }
    const double radicand = QM_SUB(QM_MUL(b, b), QM_MUL(d_perp, d_perp));
    if (radicand < 0.0 || !isfinite(radicand)) {
        *hard_fault = 1;
        return 0;
    }
    *root = SR_SQRT(radicand);
    *hard_fault = 0;
    return 1;
}

QM_FN int qm_range_ordered(double node_distance, float near_f32, int *hard_fault) {
    const double near_value = (double)near_f32;
    if (!isfinite(node_distance) || !isfinite(near_value)) {
        *hard_fault = 1;
        return 0;
    }
    // SRM-RANGE-1
    *hard_fault = 0;
    return near_value >= 1.0 && QM_SUB(node_distance, near_value) > 1.0;
}

QM_FN int qm_total_f64(double value, unsigned long long *out) {
    if (!isfinite(value)) return 0;
    const unsigned long long bits = QM_BITS(qm_canonical_zero(value));
    // SRM-TOTAL-1
    *out = (bits >> 63) != 0ull ? ~bits : (bits ^ 0x8000000000000000ull);
    return 1;
}

#endif // QM_STREAMING_REDUCTION_CUH

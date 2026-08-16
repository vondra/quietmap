// Standalone sm_120 bitcheck for the model-v2 streaming decision helpers and
// the barrier/source ABI. Production scatter includes the same headers.

#include "qm_streaming_reduction.cuh"

#define SR_INPUT_WORDS 11
#define SR_OUTPUT_WORDS 10
#define SR_RECORD_WORDS (SR_INPUT_WORDS + SR_OUTPUT_WORDS)

extern "C" __global__ void qm_streaming_reduction_selftest(
    const unsigned long long *__restrict__ records,
    unsigned long long *__restrict__ outputs, int count) {
    const int index = blockIdx.x * blockDim.x + threadIdx.x;
    if (index >= count) return;
    const unsigned long long *record = &records[(unsigned long long)index * SR_RECORD_WORDS];
    unsigned long long *output = &outputs[(unsigned long long)index * SR_OUTPUT_WORDS];
    const qm_metric_vector a = {QM_FROM_BITS(record[0]), QM_FROM_BITS(record[1])};
    const qm_metric_vector b = {QM_FROM_BITS(record[2]), QM_FROM_BITS(record[3])};
    const qm_metric_vector node = {QM_FROM_BITS(record[4]), QM_FROM_BITS(record[5])};
    const double d_perp = QM_FROM_BITS(record[6]);
    const double node_distance = QM_FROM_BITS(record[7]);
    const long long osm_id = (long long)record[8];
    const unsigned short segment_bits = (unsigned short)record[9];

    output[0] = QM_BITS(qm_atan2(node.y, node.x));
    output[1] = QM_BITS(qm_wrap_pi(QM_SUB(qm_atan2(b.y, b.x), qm_atan2(a.y, a.x))));
    output[2] = QM_BITS(qm_orient(a, b));
    output[3] = QM_BITS(qm_dot(a, b));

    unsigned long long flags = 0;
    int fault = 0;
    if (qm_same_ray(a, b, &fault)) flags |= 1ull;
    if (fault) flags |= 1ull << 1;
    if (qm_direction_less(a, b, &fault)) flags |= 1ull << 2;
    if (fault) flags |= 1ull << 3;

    float near_f32 = __int_as_float(0x7fc00000);
    const int near_valid = qm_origin_to_segment_distance_f32(a, b, &near_f32);
    flags |= ((unsigned long long)qm_candidate_wedge_owns(a, b, node, near_f32)) << 4;
    if (near_valid) flags |= 1ull << 6;

    double root = 0.0;
    const int root_valid = qm_radial_range_root(d_perp, near_f32, &root, &fault);
    if (root_valid) flags |= 1ull << 7;
    if (fault) flags |= 1ull << 8;
    if (qm_range_ordered(node_distance, near_f32, &fault)) flags |= 1ull << 9;
    if (fault) flags |= 1ull << 10;

    unsigned long long total_key = 0;
    if (qm_total_f64(node_distance, &total_key)) flags |= 1ull << 11;
    unsigned long long source_id = 0;
    if (qm_source_id_wall(osm_id, segment_bits, &source_id)) flags |= 1ull << 12;

    output[4] = flags;
    output[5] = (unsigned long long)__float_as_uint(near_f32);
    output[6] = QM_BITS((double)near_f32);
    output[7] = root_valid ? QM_BITS(root) : 0ull;
    output[8] = total_key;
    output[9] = source_id;
}

// `barr` contains max(nbarr,1) poison rows followed by a u32 count and first
// candidate id. `seg` contains one stride-5 source row. The returned words make
// any stale stride or numeric source-id conversion visible.
extern "C" __global__ void qm_streaming_layout_selftest(
    const double *__restrict__ barr, const double *__restrict__ seg,
    unsigned long long *__restrict__ output, unsigned long long barrier_count) {
    if (blockIdx.x != 0 || threadIdx.x != 0) return;
    const unsigned long long tail_slot = qm_barrier_candidate_tail_slot_offset(barrier_count);
    const unsigned int *tail = (const unsigned int *)&barr[tail_slot];
    output[0] = tail_slot;
    output[1] = (unsigned long long)tail[0];
    output[2] = (unsigned long long)tail[1];
    output[3] = QM_BITS(seg[4]);
    output[4] = QM_BITS(barr[6]);
}

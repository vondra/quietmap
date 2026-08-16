// sm_120 replay of the fixed-size Rust H0 node/mask authority records.

#include "qm_h0_node_generator.cuh"

#define H0_INPUT_WORDS 11
#define H0_OUTPUT_WORDS 9
#define H0_RECORD_WORDS (H0_INPUT_WORDS + H0_OUTPUT_WORDS)
#define H0_NODE_WORDS 8
#define H0_FIXTURE_SCALAR_WORDS 7
#define H0_FIXTURE_OUTPUT_WORDS \
    (H0_FIXTURE_SCALAR_WORDS + V2_H0_NODE_CAP * H0_NODE_WORDS)

__device__ __forceinline__ void h0_hash_word(unsigned long long word,
                                              unsigned long long *left,
                                              unsigned long long *right) {
    *left ^= word;
    *left *= 0x00000100000001b3ull;
    *right += word + 0x9e3779b97f4a7c15ull;
    *right = (*right << 27) | (*right >> (64 - 27));
    *right *= 0x94d049bb133111ebull;
}

__device__ __forceinline__ void h0_hash_node(const qm_h0_node *node,
                                              unsigned long long *left,
                                              unsigned long long *right) {
    const unsigned long long words[8] = {
        QM_BITS(node->position_m.x), QM_BITS(node->position_m.y),
        QM_BITS(node->piece_fraction), QM_BITS(node->weight),
        QM_BITS(node->placement_distance_m), QM_BITS(node->node_x_m),
        QM_BITS(node->node_distance_m), (unsigned long long)(long long)node->arm,
    };
    for (int index = 0; index < 8; index++) h0_hash_word(words[index], left, right);
}

typedef struct {
    unsigned long long node_count;
    unsigned long long fault_bits;
    unsigned long long hash_left;
    unsigned long long hash_right;
    unsigned long long masks[2];
    unsigned long long candidate_visits;
    unsigned long long guarded_candidates;
    unsigned long long span_decision;
} h0_replay_result;

__device__ __forceinline__ void h0_store_node_words(
    const qm_h0_node *node, unsigned long long *words) {
    words[0] = QM_BITS(node->position_m.x);
    words[1] = QM_BITS(node->position_m.y);
    words[2] = QM_BITS(node->piece_fraction);
    words[3] = QM_BITS(node->weight);
    words[4] = QM_BITS(node->placement_distance_m);
    words[5] = QM_BITS(node->node_x_m);
    words[6] = QM_BITS(node->node_distance_m);
    words[7] = (unsigned long long)(long long)node->arm;
}

__device__ __forceinline__ void h0_evaluate_record(
    const unsigned long long *record, h0_replay_result *result,
    unsigned long long *node_words) {
    result->node_count = 0ull;
    result->fault_bits = 0ull;
    result->hash_left = 0ull;
    result->hash_right = 0ull;
    result->masks[0] = 0ull;
    result->masks[1] = 0ull;
    result->candidate_visits = 0ull;
    result->guarded_candidates = 0ull;
    result->span_decision = QM_SPAN_HARD_FAULT;
    if (node_words != 0) {
        for (int index = 0; index < V2_H0_NODE_CAP * H0_NODE_WORDS; index++)
            node_words[index] = 0ull;
    }
    const qm_metric_vector start_m = {QM_FROM_BITS(record[0]), QM_FROM_BITS(record[1])};
    const qm_metric_vector end_m = {QM_FROM_BITS(record[2]), QM_FROM_BITS(record[3])};
    const double emission_per_m = QM_FROM_BITS(record[4]);
    const double d_floor_m = record[5] == 0 ? V2_ROAD_D_FLOOR_M : V2_RAIL_D_FLOOR_M;
    const qm_metric_vector candidate_a = {QM_FROM_BITS(record[6]), QM_FROM_BITS(record[7])};
    const qm_metric_vector candidate_b = {QM_FROM_BITS(record[8]), QM_FROM_BITS(record[9])};
    const double height = QM_FROM_BITS(record[10]);
    float near_f32 = 0.0f;
    const int near_ok =
        qm_origin_to_segment_distance_f32(candidate_a, candidate_b, &near_f32);
    result->span_decision = (unsigned long long)qm_candidate_overlaps_source_span(
        start_m, end_m, candidate_a, candidate_b, near_ok ? near_f32 : NAN);
    if (!isfinite(height)) {
        result->fault_bits = QM_H0_FAULT_NONFINITE;
        return;
    }
    if (!near_ok) {
        result->fault_bits = QM_H0_FAULT_NONFINITE;
        return;
    }
    qm_h0_iterator iterator;
    if (!qm_h0_iterator_init(&iterator, start_m, end_m, emission_per_m, d_floor_m)) {
        result->fault_bits = iterator.fault_bits;
        return;
    }
    unsigned long long hash_left = 0xcbf29ce484222325ull;
    unsigned long long hash_right = 0x6a09e667f3bcc909ull;
    qm_h0_node node;
    double node_x[V2_H0_NODE_CAP];
    double node_distance[V2_H0_NODE_CAP];
    for (;;) {
        const int status = qm_h0_iterator_next(&iterator, &node);
        if (status == 0) break;
        if (status < 0) {
            result->fault_bits = iterator.fault_bits;
            return;
        }
        const int node_index = iterator.emitted - 1;
        h0_hash_node(&node, &hash_left, &hash_right);
        if (node_words != 0)
            h0_store_node_words(&node, &node_words[node_index * H0_NODE_WORDS]);
        node_x[node_index] = node.node_x_m;
        node_distance[node_index] = node.node_distance_m;
    }
    if (!qm_h0_mask_candidate_nodes(
            candidate_a, candidate_b, near_f32, node_x, node_distance,
            iterator.emitted, iterator.foot_m, iterator.direction,
            result->masks, &result->guarded_candidates)) {
        result->fault_bits = QM_H0_FAULT_INTERNAL;
        return;
    }
    result->hash_left = hash_left;
    result->hash_right = hash_right;
    result->node_count = (unsigned long long)iterator.emitted;
    result->candidate_visits = 1ull;
}

extern "C" __global__ void qm_h0_node_selftest(
    const unsigned long long *__restrict__ records,
    unsigned long long *__restrict__ outputs, int count) {
    const int record_index = blockIdx.x * blockDim.x + threadIdx.x;
    if (record_index >= count) return;
    const unsigned long long *record =
        &records[(unsigned long long)record_index * H0_RECORD_WORDS];
    unsigned long long *output =
        &outputs[(unsigned long long)record_index * H0_OUTPUT_WORDS];
    h0_replay_result result;
    h0_evaluate_record(record, &result, 0);
    output[0] = result.node_count;
    output[1] = result.fault_bits;
    output[2] = result.hash_left;
    output[3] = result.hash_right;
    output[4] = result.masks[0];
    output[5] = result.masks[1];
    output[6] = result.candidate_visits;
    output[7] = result.guarded_candidates;
    output[8] = result.span_decision;
}

extern "C" __global__ void qm_h0_node_fixture_selftest(
    const unsigned long long *__restrict__ inputs,
    unsigned long long *__restrict__ outputs, int count) {
    const int fixture_index = blockIdx.x * blockDim.x + threadIdx.x;
    if (fixture_index >= count) return;
    const unsigned long long *input =
        &inputs[(unsigned long long)fixture_index * H0_INPUT_WORDS];
    unsigned long long *output =
        &outputs[(unsigned long long)fixture_index * H0_FIXTURE_OUTPUT_WORDS];
    h0_replay_result result;
    h0_evaluate_record(input, &result, &output[H0_FIXTURE_SCALAR_WORDS]);
    output[0] = result.fault_bits;
    output[1] = result.node_count;
    output[2] = result.masks[0];
    output[3] = result.masks[1];
    output[4] = result.candidate_visits;
    output[5] = result.guarded_candidates;
    output[6] = result.span_decision;
}

extern "C" __global__ void qm_h0_fault_selftest(unsigned long long *output) {
    if (blockIdx.x != 0 || threadIdx.x != 0) return;
    double distance = 0.0;
    qm_metric_vector vector_3_4 = {3.0, 4.0};
    output[0] =
        (unsigned long long)qm_node_horizontal_range(vector_3_4, &distance);
    output[1] = QM_BITS(distance);
    qm_metric_vector zero_vector = {0.0, -0.0};
    output[2] =
        (unsigned long long)qm_node_horizontal_range(zero_vector, &distance);
    qm_metric_vector nan_vector = {QM_FROM_BITS(0x7ff8000000000000ull), 0.0};
    output[3] =
        (unsigned long long)qm_node_horizontal_range(nan_vector, &distance);
    qm_h0_iterator iterator;
    qm_metric_vector start_m = {-125.0, 0.0};
    qm_metric_vector end_m = {125.0, 0.0};
    output[4] = (unsigned long long)qm_h0_iterator_init(
        &iterator, start_m, end_m, 1.0, V2_RAIL_D_FLOOR_M);
    iterator.emitted = V2_H0_NODE_CAP;
    qm_h0_node node;
    output[5] = (unsigned long long)(long long)qm_h0_iterator_next(&iterator, &node);
    output[6] = (unsigned long long)iterator.fault_bits;
    output[7] =
        (unsigned long long)qm_h0_layout_valid(0, OUT_SLOTS_H0, 0ull, 7ull) |
        ((unsigned long long)qm_h0_layout_valid(1, OUT_SLOTS_H0, 1ull, 7ull) << 1) |
        ((unsigned long long)qm_h0_layout_valid(0, OUT_SLOTS_H0, 2ull, 14ull) << 2) |
        ((unsigned long long)qm_h0_layout_valid(1, OUT_SLOTS_H0, 17ull, 119ull) << 3);
    output[8] = (unsigned long long)qm_h0_layout_valid(
        0, OUT_SLOTS_H0, 2ull, 12ull);  // stale stride-6 tail poison
    output[9] = (unsigned long long)qm_h0_layout_valid(
        2, OUT_SLOTS_H0, 2ull, 14ull);  // invalid layer tag
    output[10] = (unsigned long long)qm_h0_layout_valid(
        0, OUT_SLOTS_H0 - 1, 2ull, 14ull);  // short exact-counter buffer
    output[11] = output[8] || output[9] || output[10];  // no poisoned output accepted

    qm_metric_vector local_frame, translated_frame;
    output[12] = (unsigned long long)qm_source_frame_vector_from_world(
        32.5, -8.25, 32.0, -8.5, 2048.0, &local_frame);
    output[13] = QM_BITS(local_frame.x);
    output[14] = QM_BITS(local_frame.y);
    output[15] = (unsigned long long)qm_source_frame_vector_from_world(
        64.5, 23.75, 64.0, 23.5, 2048.0, &translated_frame);
    output[16] = QM_BITS(translated_frame.x);
    output[17] = QM_BITS(translated_frame.y);
    output[18] = (unsigned long long)qm_source_frame_vector_from_world(
        1.0, 2.0, 0.0, 0.0, 0.0, &translated_frame);
    output[19] = (unsigned long long)qm_source_frame_vector_from_world(
        NAN, 2.0, 0.0, 0.0, 1.0, &translated_frame);

    output[20] = (unsigned long long)qm_source_frame_vector_from_world(
        50.100987654321, 14.300123456789, 50.100123456789,
        14.299987654321, 71432.123456789, &translated_frame);
    output[21] = QM_BITS(translated_frame.x);
    output[22] = QM_BITS(translated_frame.y);

    double node_x[V2_H0_NODE_CAP];
    double node_distance[V2_H0_NODE_CAP];
    for (int index = 0; index < V2_H0_NODE_CAP; index++) {
        node_x[index] = 0.0;
        node_distance[index] = 10.0;
    }
    unsigned long long mask[2] = {0ull, 0ull};
    unsigned long long guarded = 0ull;
    output[23] = (unsigned long long)qm_h0_mask_candidate_nodes(
        (qm_metric_vector){2.0, -1.0}, (qm_metric_vector){2.0, 1.0},
        2.0f, node_x, node_distance, V2_H0_NODE_CAP,
        (qm_metric_vector){10.0, 0.0}, (qm_metric_vector){0.0, 1.0},
        mask, &guarded);
    output[24] = mask[0];
    output[25] = mask[1];
    output[26] = guarded;
    output[27] = (unsigned long long)qm_h0_mask_candidate_nodes(
        (qm_metric_vector){0.0, 0.0}, (qm_metric_vector){0.0, 0.0},
        0.5f, node_x, node_distance, V2_H0_NODE_CAP,
        (qm_metric_vector){10.0, 0.0}, (qm_metric_vector){0.0, 1.0},
        mask, &guarded);
    output[28] = mask[0];
    output[29] = mask[1];
    output[30] = guarded;
    const unsigned long long degenerate_record[H0_INPUT_WORDS] = {
        QM_BITS(0.0), QM_BITS(0.0), QM_BITS(0.0), QM_BITS(0.0),
        QM_BITS(1.0), 0ull, QM_BITS(5.0), QM_BITS(-1.0),
        QM_BITS(5.0), QM_BITS(1.0), QM_BITS(12.0),
    };
    h0_replay_result degenerate_result;
    h0_evaluate_record(degenerate_record, &degenerate_result, 0);
    output[31] = degenerate_result.fault_bits;
    output[32] = degenerate_result.hash_left;
    output[33] = degenerate_result.hash_right;
    output[34] = degenerate_result.node_count;
}

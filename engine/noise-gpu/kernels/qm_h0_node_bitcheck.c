// Host-C replay of the Rust H0 node-generator and one-candidate mask authority.

#include <stdint.h>
#include <stdio.h>

#include "qm_h0_node_generator.cuh"

#define H0_INPUT_WORDS 11
#define H0_OUTPUT_WORDS 9
#define H0_RECORD_WORDS (H0_INPUT_WORDS + H0_OUTPUT_WORDS)
#define H0_EXPECTED_RECORDS 1000000ull
#define H0_NODE_WORDS 8
#define H0_FIXTURE_SCALAR_WORDS 7
#define H0_FIXTURE_OUTPUT_WORDS \
    (H0_FIXTURE_SCALAR_WORDS + V2_H0_NODE_CAP * H0_NODE_WORDS)
#define H0_FIXTURE_RECORD_WORDS (H0_INPUT_WORDS + H0_FIXTURE_OUTPUT_WORDS)
#define H0_FIXTURE_MAGIC 0x514d483046495831ull

static void hash_word(unsigned long long word, unsigned long long *left,
                      unsigned long long *right) {
    *left ^= word;
    *left *= 0x00000100000001b3ull;
    *right += word + 0x9e3779b97f4a7c15ull;
    *right = (*right << 27) | (*right >> (64 - 27));
    *right *= 0x94d049bb133111ebull;
}

static void hash_node(const qm_h0_node *node, unsigned long long *left,
                      unsigned long long *right) {
    const unsigned long long words[8] = {
        QM_BITS(node->position_m.x), QM_BITS(node->position_m.y),
        QM_BITS(node->piece_fraction), QM_BITS(node->weight),
        QM_BITS(node->placement_distance_m), QM_BITS(node->node_x_m),
        QM_BITS(node->node_distance_m), (unsigned long long)(long long)node->arm,
    };
    for (int index = 0; index < 8; index++) hash_word(words[index], left, right);
}

static void store_node_words(const qm_h0_node *node, unsigned long long *words) {
    words[0] = QM_BITS(node->position_m.x);
    words[1] = QM_BITS(node->position_m.y);
    words[2] = QM_BITS(node->piece_fraction);
    words[3] = QM_BITS(node->weight);
    words[4] = QM_BITS(node->placement_distance_m);
    words[5] = QM_BITS(node->node_x_m);
    words[6] = QM_BITS(node->node_distance_m);
    words[7] = (unsigned long long)(long long)node->arm;
}

static void evaluate(const unsigned long long *record, unsigned long long *output,
                     unsigned long long *node_words) {
    const qm_metric_vector start_m = {QM_FROM_BITS(record[0]), QM_FROM_BITS(record[1])};
    const qm_metric_vector end_m = {QM_FROM_BITS(record[2]), QM_FROM_BITS(record[3])};
    const double emission_per_m = QM_FROM_BITS(record[4]);
    const double d_floor_m = record[5] == 0 ? V2_ROAD_D_FLOOR_M : V2_RAIL_D_FLOOR_M;
    const qm_metric_vector candidate_a = {QM_FROM_BITS(record[6]), QM_FROM_BITS(record[7])};
    const qm_metric_vector candidate_b = {QM_FROM_BITS(record[8]), QM_FROM_BITS(record[9])};
    const double height = QM_FROM_BITS(record[10]);
    for (int index = 0; index < H0_OUTPUT_WORDS; index++) output[index] = 0ull;
    if (node_words != NULL) {
        for (int index = 0; index < V2_H0_NODE_CAP * H0_NODE_WORDS; index++)
            node_words[index] = 0ull;
    }
    float near_f32 = 0.0f;
    const int near_ok =
        qm_origin_to_segment_distance_f32(candidate_a, candidate_b, &near_f32);
    output[8] = (unsigned long long)qm_candidate_overlaps_source_span(
        start_m, end_m, candidate_a, candidate_b, near_ok ? near_f32 : NAN);
    if (!isfinite(height)) {
        output[1] = QM_H0_FAULT_NONFINITE;
        return;
    }
    if (!near_ok) {
        output[1] = QM_H0_FAULT_NONFINITE;
        return;
    }
    qm_h0_iterator iterator;
    if (!qm_h0_iterator_init(&iterator, start_m, end_m, emission_per_m, d_floor_m)) {
        output[1] = iterator.fault_bits;
        return;
    }

    unsigned long long left = 0xcbf29ce484222325ull;
    unsigned long long right = 0x6a09e667f3bcc909ull;
    unsigned long long masks[2] = {0ull, 0ull};
    unsigned long long guarded = 0ull;
    double node_x[V2_H0_NODE_CAP];
    double node_distance[V2_H0_NODE_CAP];
    qm_h0_node node;
    for (;;) {
        const int status = qm_h0_iterator_next(&iterator, &node);
        if (status == 0) break;
        if (status < 0) {
            output[1] = iterator.fault_bits;
            return;
        }
        hash_node(&node, &left, &right);
        const int node_index = iterator.emitted - 1;
        if (node_words != NULL)
            store_node_words(&node, &node_words[node_index * H0_NODE_WORDS]);
        node_x[node_index] = node.node_x_m;
        node_distance[node_index] = node.node_distance_m;
    }
    if (!qm_h0_mask_candidate_nodes(
            candidate_a, candidate_b, near_f32, node_x, node_distance,
            iterator.emitted, iterator.foot_m, iterator.direction, masks,
            &guarded)) {
        output[1] = QM_H0_FAULT_INTERNAL;
        return;
    }
    output[0] = (unsigned long long)iterator.emitted;
    output[2] = left;
    output[3] = right;
    output[4] = masks[0];
    output[5] = masks[1];
    output[6] = 1ull;
    output[7] = guarded;
}

static int direct_selfcheck(void) {
    unsigned long long scratch = 0ull;
    double root = 0.0;
    int hard_fault = 0;
    if (qm_barrier_candidate_tail_slot_offset(0ull) != 7ull ||
        !qm_source_id_obstacle(0ull, &scratch) ||
        !qm_source_id_wall(0ll, 0u, &scratch) ||
        !qm_radial_range_root(0.5, 2.0f, &root, &hard_fault) || hard_fault ||
        !qm_total_f64(-2.0, &scratch) ||
        !isfinite(qm_wrap_pi(qm_atan2(1.0, 1.0)))) {
        puts("FAIL: included streaming-helper contract");
        return 0;
    }
    double distance = 0.0;
    if (!qm_node_horizontal_range((qm_metric_vector){3.0, 4.0}, &distance) ||
        QM_BITS(distance) != QM_BITS(5.0)) {
        puts("FAIL: SRM-NODE-RANGE-1 direct 3-4-5 fixture");
        return 0;
    }
    if (qm_node_horizontal_range((qm_metric_vector){0.0, -0.0}, &distance) ||
        qm_node_horizontal_range((qm_metric_vector){NAN, 0.0}, &distance)) {
        puts("FAIL: SRM-NODE-RANGE-1 fault branches");
        return 0;
    }
    if (!qm_h0_layout_valid(0, OUT_SLOTS_H0, 0ull, 7ull) ||
        !qm_h0_layout_valid(1, OUT_SLOTS_H0, 1ull, 7ull) ||
        !qm_h0_layout_valid(0, OUT_SLOTS_H0, 2ull, 14ull) ||
        !qm_h0_layout_valid(1, OUT_SLOTS_H0, 17ull, 119ull) ||
        qm_h0_layout_valid(0, OUT_SLOTS_H0, 2ull, 12ull) ||
        qm_h0_layout_valid(2, OUT_SLOTS_H0, 2ull, 14ull) ||
        qm_h0_layout_valid(0, OUT_SLOTS_H0 - 1, 2ull, 14ull)) {
        puts("FAIL: H0 ABI/layout poison fixtures");
        return 0;
    }
    qm_metric_vector local_frame, translated_frame;
    if (!qm_source_frame_vector_from_world(
            32.5, -8.25, 32.0, -8.5, 2048.0, &local_frame) ||
        !qm_source_frame_vector_from_world(
            64.5, 23.75, 64.0, 23.5, 2048.0, &translated_frame) ||
        QM_BITS(local_frame.x) != QM_BITS(512.0) ||
        QM_BITS(local_frame.y) != QM_BITS(55270.0) ||
        QM_BITS(translated_frame.x) != QM_BITS(local_frame.x) ||
        QM_BITS(translated_frame.y) != QM_BITS(local_frame.y) ||
        qm_source_frame_vector_from_world(
            1.0, 2.0, 0.0, 0.0, 0.0, &translated_frame) ||
        qm_source_frame_vector_from_world(
            NAN, 2.0, 0.0, 0.0, 1.0, &translated_frame)) {
        puts("FAIL: SRM-FRAME-1 non-origin/fault fixtures");
        return 0;
    }
    qm_metric_vector discriminating_frame;
    const double fixture_latitude = 50.100987654321;
    const double fixture_longitude = 14.300123456789;
    const double fixture_receiver_latitude = 50.100123456789;
    const double fixture_receiver_longitude = 14.299987654321;
    const double fixture_mlon = 71432.123456789;
    const double forbidden_x = QM_SUB(
        QM_MUL(fixture_longitude, fixture_mlon),
        QM_MUL(fixture_receiver_longitude, fixture_mlon));
    const double forbidden_y = QM_SUB(
        QM_MUL(fixture_latitude, M_LAT),
        QM_MUL(fixture_receiver_latitude, M_LAT));
    if (!qm_source_frame_vector_from_world(
            fixture_latitude, fixture_longitude, fixture_receiver_latitude,
            fixture_receiver_longitude, fixture_mlon, &discriminating_frame) ||
        QM_BITS(discriminating_frame.x) != 0x402366bcbb5bdc1cull ||
        QM_BITS(discriminating_frame.y) != 0x4057e1d13a0c8f46ull ||
        QM_BITS(discriminating_frame.x) == QM_BITS(forbidden_x) ||
        QM_BITS(discriminating_frame.y) == QM_BITS(forbidden_y)) {
        puts("FAIL: SRM-FRAME-1 reassociation-killing fixture");
        return 0;
    }
    if (qm_candidate_overlaps_source_span(
            (qm_metric_vector){1.0, 0.0}, (qm_metric_vector){0.0, 1.0},
            (qm_metric_vector){1.0, 1.0}, (qm_metric_vector){-1.0, 1.0},
            2.0f) != QM_SPAN_OVERLAPS ||
        qm_candidate_overlaps_source_span(
            (qm_metric_vector){1.0, 0.0}, (qm_metric_vector){0.0, 1.0},
            (qm_metric_vector){0.0, 1.0}, (qm_metric_vector){-1.0, 0.0},
            2.0f) != QM_SPAN_OUTSIDE) {
        puts("FAIL: SRM-SPAN-1 direct fixtures");
        return 0;
    }
    qm_h0_iterator iterator;
    if (!qm_h0_iterator_init(&iterator, (qm_metric_vector){-125.0, 0.0},
                             (qm_metric_vector){125.0, 0.0}, 1.0,
                             V2_RAIL_D_FLOOR_M)) {
        puts("FAIL: H0 direct rail iterator init");
        return 0;
    }
    iterator.emitted = V2_H0_NODE_CAP;
    qm_h0_node node;
    if (qm_h0_iterator_next(&iterator, &node) >= 0 ||
        (iterator.fault_bits & QM_H0_FAULT_NODE_CAP) == 0u) {
        puts("FAIL: H0 live node-cap fault");
        return 0;
    }
    double synthetic_node_x[V2_H0_NODE_CAP];
    double synthetic_node_distance[V2_H0_NODE_CAP];
    for (int index = 0; index < V2_H0_NODE_CAP; index++) {
        synthetic_node_x[index] = 0.0;
        synthetic_node_distance[index] = 10.0;
    }
    unsigned long long mask[2] = {0ull, 0ull};
    unsigned long long guarded = 0ull;
    if (!qm_h0_mask_candidate_nodes(
            (qm_metric_vector){2.0, -1.0}, (qm_metric_vector){2.0, 1.0},
            2.0f, synthetic_node_x, synthetic_node_distance, V2_H0_NODE_CAP,
            (qm_metric_vector){10.0, 0.0}, (qm_metric_vector){0.0, 1.0},
            mask, &guarded) ||
        mask[0] != 0xffffffffffffffffull || mask[1] != 3ull || guarded != 0ull) {
        puts("FAIL: shared H0 mask loop second-word fixture");
        return 0;
    }
    if (!qm_h0_mask_candidate_nodes(
            (qm_metric_vector){0.0, 0.0}, (qm_metric_vector){0.0, 0.0},
            0.5f, synthetic_node_x, synthetic_node_distance, V2_H0_NODE_CAP,
            (qm_metric_vector){10.0, 0.0}, (qm_metric_vector){0.0, 1.0},
            mask, &guarded) ||
        mask[0] != 0xffffffffffffffffull || mask[1] != 3ull || guarded != 1ull) {
        puts("FAIL: near-guarded candidate must retain prior mask");
        return 0;
    }
    return 1;
}

static int check_complete_fixtures(const char *path) {
    FILE *dump = fopen(path, "rb");
    if (dump == NULL) {
        perror(path);
        return 0;
    }
    unsigned long long header[2];
    if (fread(header, sizeof(header[0]), 2, dump) != 2 ||
        header[0] != H0_FIXTURE_MAGIC || header[1] == 0ull) {
        puts("FAIL: invalid H0 direct-fixture header");
        fclose(dump);
        return 0;
    }
    unsigned long long record[H0_FIXTURE_RECORD_WORDS];
    unsigned long long output[H0_OUTPUT_WORDS];
    unsigned long long node_words[V2_H0_NODE_CAP * H0_NODE_WORDS];
    unsigned long long mismatches = 0ull;
    for (unsigned long long fixture = 0; fixture < header[1]; fixture++) {
        if (fread(record, sizeof(record[0]), H0_FIXTURE_RECORD_WORDS, dump) !=
            H0_FIXTURE_RECORD_WORDS) {
            printf("FAIL: short H0 direct fixture %llu\n", fixture);
            fclose(dump);
            return 0;
        }
        evaluate(record, output, node_words);
        const unsigned long long *expected = &record[H0_INPUT_WORDS];
        const unsigned long long scalar[H0_FIXTURE_SCALAR_WORDS] = {
            output[1], output[0], output[4], output[5], output[6], output[7], output[8],
        };
        for (int index = 0; index < H0_FIXTURE_SCALAR_WORDS; index++)
            mismatches += scalar[index] != expected[index];
        for (int index = 0; index < V2_H0_NODE_CAP * H0_NODE_WORDS; index++)
            mismatches += node_words[index] != expected[H0_FIXTURE_SCALAR_WORDS + index];
    }
    unsigned long long trailing = 0ull;
    const size_t trailing_words = fread(&trailing, sizeof(trailing), 1, dump);
    const int io_error = ferror(dump);
    fclose(dump);
    if (trailing_words != 0 || io_error || mismatches != 0ull) {
        printf("FAIL: H0 direct fixture mismatches=%llu trailing_words=%zu io_error=%d\n",
               mismatches, trailing_words, io_error);
        return 0;
    }
    printf("PASS: %llu H0 direct fixtures compare every node field\n", header[1]);
    return 1;
}

int main(int argc, char **argv) {
    if (argc != 3) {
        fprintf(stderr, "usage: %s <QM_H0_NODE_DUMP> <QM_H0_NODE_FIXTURE_DUMP>\n",
                argv[0]);
        return 2;
    }
    if (!direct_selfcheck()) return 1;
    FILE *dump = fopen(argv[1], "rb");
    if (dump == NULL) {
        perror(argv[1]);
        return 2;
    }
    unsigned long long record[H0_RECORD_WORDS];
    unsigned long long output[H0_OUTPUT_WORDS];
    unsigned long long mismatches[H0_OUTPUT_WORDS] = {0};
    unsigned long long first_record[H0_OUTPUT_WORDS] = {0};
    unsigned long long records = 0;
    size_t words;
    while ((words = fread(record, sizeof(record[0]), H0_RECORD_WORDS, dump)) ==
           H0_RECORD_WORDS) {
        evaluate(record, output, NULL);
        for (int index = 0; index < H0_OUTPUT_WORDS; index++) {
            if (output[index] != record[H0_INPUT_WORDS + index]) {
                if (mismatches[index] == 0) first_record[index] = records;
                mismatches[index]++;
            }
        }
        records++;
    }
    const int io_error = ferror(dump);
    fclose(dump);
    if (io_error || words != 0 || records != H0_EXPECTED_RECORDS) {
        printf("FAIL: records=%llu trailing_words=%zu io_error=%d\n", records, words,
               io_error);
        return 1;
    }
    unsigned long long total = 0;
    for (int index = 0; index < H0_OUTPUT_WORDS; index++) {
        total += mismatches[index];
        printf("field_%d mismatches %llu", index, mismatches[index]);
        if (mismatches[index] != 0) printf(" first_record %llu", first_record[index]);
        putchar('\n');
    }
    if (total != 0) {
        printf("FAIL: %llu H0 lane mismatches\n", total);
        return 1;
    }
    printf("PASS: %llu H0 node/mask records bit-identical\n", records);
    if (!check_complete_fixtures(argv[2])) return 1;
    return 0;
}

// Host-C replay of the Rust streaming-decision authority. The sm_120 selftest
// compiles the same header and consumes the same 21-word records on device.
//
/* Build with build.rs' generated ABI and without contraction:
 *   cc -O2 -std=c99 -ffp-contract=off -Wall -Wextra -Werror
 *      -include $OUT_DIR/qm_streaming_abi_generated.h
 *      -o qm_streaming_reduction_bitcheck qm_streaming_reduction_bitcheck.c -lm
 */

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "qm_streaming_reduction.cuh"

#define SR_INPUT_WORDS 11
#define SR_OUTPUT_WORDS 10
#define SR_RECORD_WORDS (SR_INPUT_WORDS + SR_OUTPUT_WORDS)
#define SR_EXPECTED_RECORDS 1000000ull

static float float_from_bits(uint32_t bits) {
    float value;
    memcpy(&value, &bits, sizeof(value));
    return value;
}

static int abi_selfcheck(void) {
    const unsigned long long counts[4] = {0ull, 1ull, 2ull, 17ull};
    const unsigned long long wants[4] = {7ull, 7ull, 14ull, 119ull};
    for (size_t i = 0; i < 4; i++) {
        if (qm_barrier_candidate_tail_slot_offset(counts[i]) != wants[i]) {
            printf("FAIL: barrier tail offset for %llu rows\n", counts[i]);
            return 0;
        }
    }
    unsigned long long source_id = 0;
    if (!qm_source_id_obstacle(0ull, &source_id) || source_id != 0ull ||
        !qm_source_id_obstacle(SR_WALL_SOURCE_TAG - 1ull, &source_id) ||
        source_id != SR_WALL_SOURCE_TAG - 1ull ||
        qm_source_id_obstacle(SR_WALL_SOURCE_TAG, &source_id)) {
        printf("FAIL: obstacle source-id namespace\n");
        return 0;
    }
    if (QM_BITS(qm_tan(0.0)) != QM_BITS(0.0)) {
        printf("FAIL: included shared-math contract\n");
        return 0;
    }
    return 1;
}

static void evaluate(const unsigned long long *record, unsigned long long *output) {
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

    float near_f32 = float_from_bits(0x7fc00000u);
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

    uint32_t near_bits;
    memcpy(&near_bits, &near_f32, sizeof(near_bits));
    output[4] = flags;
    output[5] = (unsigned long long)near_bits;
    output[6] = QM_BITS((double)near_f32);
    output[7] = root_valid ? QM_BITS(root) : 0ull;
    output[8] = total_key;
    output[9] = source_id;
}

int main(int argc, char **argv) {
    if (argc != 2) {
        fprintf(stderr, "usage: %s <QM_STREAMING_REDUCTION_DUMP>\n", argv[0]);
        return 2;
    }
    if (!abi_selfcheck()) {
        return 1;
    }
    FILE *dump = fopen(argv[1], "rb");
    if (dump == NULL) {
        perror(argv[1]);
        return 2;
    }

    static const char *const names[SR_OUTPUT_WORDS] = {
        "atan2", "wrap_pi", "orient", "dot", "flags", "near_f32",
        "near_widen", "radial_root", "total_f64", "source_id",
    };
    unsigned long long record[SR_RECORD_WORDS];
    unsigned long long output[SR_OUTPUT_WORDS];
    unsigned long long mismatches[SR_OUTPUT_WORDS] = {0};
    unsigned long long first_record[SR_OUTPUT_WORDS] = {0};
    unsigned long long first_got[SR_OUTPUT_WORDS] = {0};
    unsigned long long first_want[SR_OUTPUT_WORDS] = {0};
    unsigned long long records = 0;

    size_t words;
    while ((words = fread(record, sizeof(record[0]), SR_RECORD_WORDS, dump)) ==
           SR_RECORD_WORDS) {
        evaluate(record, output);
        for (size_t i = 0; i < SR_OUTPUT_WORDS; i++) {
            const unsigned long long want = record[SR_INPUT_WORDS + i];
            if (output[i] != want) {
                if (mismatches[i] == 0) {
                    first_record[i] = records;
                    first_got[i] = output[i];
                    first_want[i] = want;
                }
                mismatches[i]++;
            }
        }
        records++;
    }
    const int io_error = ferror(dump);
    fclose(dump);
    if (io_error || words != 0) {
        printf("FAIL: truncated or unreadable record after %llu complete records\n", records);
        return 1;
    }
    if (records != SR_EXPECTED_RECORDS) {
        printf("FAIL: read %llu records, expected %llu\n", records, SR_EXPECTED_RECORDS);
        return 1;
    }

    unsigned long long total_mismatches = 0;
    for (size_t i = 0; i < SR_OUTPUT_WORDS; i++) {
        total_mismatches += mismatches[i];
        printf("%-12s mismatches %llu", names[i], mismatches[i]);
        if (mismatches[i] != 0) {
            printf(" first_record %llu got %016llx want %016llx", first_record[i],
                   first_got[i], first_want[i]);
        }
        putchar('\n');
    }
    if (total_mismatches != 0) {
        printf("FAIL: %llu lane mismatches\n", total_mismatches);
        return 1;
    }
    printf("PASS: %llu records, every streaming decision bit identical\n", records);
    return 0;
}

// Cross-lane bit-identity harness for qm_shared_math.cuh — replays the CPU
// lane's dump through the HOST compilation of the same header and compares u64
// bit patterns.
//
// WHY A HOST COMPILE PROVES SOMETHING ABOUT THE DEVICE. The device and this
// binary compile the SAME text; the only difference is what QM_ADD/QM_SUB/
// QM_MUL/QM_DIV expand to — `__dadd_rn` and friends on the device, bare
// operators here. Both are IEEE-754 round-to-nearest-even on the same f64
// inputs, and IEEE arithmetic is deterministic, so an identical expression tree
// over identical inputs gives identical bits. What this harness actually tests
// is therefore the thing that CAN differ: whether the mirror's expression tree,
// branch thresholds, constants, and evaluation order match the Rust lane's,
// character by character. What it cannot test is the two device-only hazards —
// nvcc contracting an operator into an fma, and ptxas mis-scheduling — which is
// exactly what qm_shared_math_selftest.cu exists for, and why this box (no CUDA
// toolkit) hands that half to the GPU owner.
//
//   QM_SHARED_MATH_DUMP=/tmp/qm.bin cargo test -p noise-compute shared_math
//   gcc -O2 -std=c99 -ffp-contract=off -Wall -Wextra
//       -o /tmp/qm_bitcheck engine/noise-gpu/kernels/qm_shared_math_bitcheck.c
//   /tmp/qm_bitcheck /tmp/qm.bin
//
// -ffp-contract=off is NOT optional: gcc defaults to `fast` for C and would
// contract this side while the device mirror stays unfused.

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

#include "qm_shared_math.cuh"

// One dump record: the CPU lane's input and its result, for both functions.
#define QM_RECORD_WORDS 4

/// Every constant, pinned to the bit pattern printed in the fdlibm/musl source.
/// The Rust lane pins the same list; a table that parsed differently on one side
/// would otherwise show up as a flood of mismatches with no cause attached.
static int constants_match_published_bit_patterns(void) {
    static const unsigned long long atanhi[4] = {0x3FDDAC670561BB4FULL, 0x3FE921FB54442D18ULL,
                                                 0x3FEF730BD281F69BULL, 0x3FF921FB54442D18ULL};
    static const unsigned long long atanlo[4] = {0x3C7A2B7F222F65E2ULL, 0x3C81A62633145C07ULL,
                                                 0x3C7007887AF0CBBDULL, 0x3C91A62633145C07ULL};
    static const unsigned long long at[11] = {
        0x3FD555555555550DULL, 0xBFC999999998EBC4ULL, 0x3FC24924920083FFULL, 0xBFBC71C6FE231671ULL,
        0x3FB745CDC54C206EULL, 0xBFB3B0F2AF749A6DULL, 0x3FB10D66A0D03D51ULL, 0xBFADDE2D52DEFD9AULL,
        0x3FA97B4B24760DEBULL, 0xBFA2B4442C6A6C2FULL, 0x3F90AD3AE322DA11ULL};
    static const unsigned long long t[13] = {
        0x3FD5555555555563ULL, 0x3FC111111110FE7AULL, 0x3FABA1BA1BB341FEULL, 0x3F9664F48406D637ULL,
        0x3F8226E3E96E8493ULL, 0x3F6D6D22C9560328ULL, 0x3F57DBC8FEE08315ULL, 0x3F4344D8F2F26501ULL,
        0x3F3026F71A8D1068ULL, 0x3F147E88A03792A6ULL, 0x3F12B80F32F0A7E9ULL, 0xBEF375CBDB605373ULL,
        0x3EFB2A7074BF7AD4ULL};
    int bad = 0;
    for (int i = 0; i < 4; i++) {
        if (QM_BITS(QM_ATANHI[i]) != atanhi[i]) {
            printf("QM_ATANHI[%d] parsed as %016llx, want %016llx\n", i, QM_BITS(QM_ATANHI[i]),
                   atanhi[i]);
            bad++;
        }
        if (QM_BITS(QM_ATANLO[i]) != atanlo[i]) {
            printf("QM_ATANLO[%d] parsed as %016llx, want %016llx\n", i, QM_BITS(QM_ATANLO[i]),
                   atanlo[i]);
            bad++;
        }
    }
    for (int i = 0; i < 11; i++) {
        if (QM_BITS(QM_AT[i]) != at[i]) {
            printf("QM_AT[%d] parsed as %016llx, want %016llx\n", i, QM_BITS(QM_AT[i]), at[i]);
            bad++;
        }
    }
    for (int i = 0; i < 13; i++) {
        if (QM_BITS(QM_T[i]) != t[i]) {
            printf("QM_T[%d] parsed as %016llx, want %016llx\n", i, QM_BITS(QM_T[i]), t[i]);
            bad++;
        }
    }
    struct {
        const char *name;
        double value;
        unsigned long long want;
    } scalars[] = {
        {"QM_PIO4", QM_PIO4, 0x3FE921FB54442D18ULL},
        {"QM_PIO4_LO", QM_PIO4_LO, 0x3C81A62633145C07ULL},
        {"QM_PIO2_1", QM_PIO2_1, 0x3FF921FB54400000ULL},
        {"QM_PIO2_1T", QM_PIO2_1T, 0x3DD0B4611A626331ULL},
        {"QM_PIO2_2", QM_PIO2_2, 0x3DD0B4611A600000ULL},
        {"QM_PIO2_2T", QM_PIO2_2T, 0x3BA3198A2E037073ULL},
    };
    for (size_t i = 0; i < sizeof(scalars) / sizeof(scalars[0]); i++) {
        if (QM_BITS(scalars[i].value) != scalars[i].want) {
            printf("%s parsed as %016llx, want %016llx\n", scalars[i].name,
                   QM_BITS(scalars[i].value), scalars[i].want);
            bad++;
        }
    }
    const double pi = QM_FROM_BITS(0x400921fb54442d18ull);
    if (QM_BITS(qm_atan2(-0.0, -1.0)) != QM_BITS(-pi)) {
        printf("qm_atan2 signed seam differs\n");
        bad++;
    }
    if (QM_BITS(qm_wrap_pi(-pi)) != QM_BITS(pi)) {
        printf("qm_wrap_pi negative seam differs\n");
        bad++;
    }
    return bad;
}

int main(int argc, char **argv) {
    if (argc != 2) {
        fprintf(stderr, "usage: %s <dump from QM_SHARED_MATH_DUMP>\n", argv[0]);
        return 2;
    }
    int bad_constants = constants_match_published_bit_patterns();
    if (bad_constants != 0) {
        printf("FAIL: %d constants differ from the published bit patterns\n", bad_constants);
        return 1;
    }

    FILE *dump = fopen(argv[1], "rb");
    if (dump == NULL) {
        perror(argv[1]);
        return 2;
    }
    unsigned long long record[QM_RECORD_WORDS];
    unsigned long long records = 0, atan_mismatches = 0, tan_mismatches = 0;
    unsigned long long first_atan_x = 0, first_tan_x = 0;
    while (fread(record, sizeof(unsigned long long), QM_RECORD_WORDS, dump) == QM_RECORD_WORDS) {
        const unsigned long long got_atan = QM_BITS(qm_atan(QM_FROM_BITS(record[0])));
        const unsigned long long got_tan = QM_BITS(qm_tan(QM_FROM_BITS(record[2])));
        if (got_atan != record[1]) {
            if (atan_mismatches == 0) {
                first_atan_x = record[0];
            }
            atan_mismatches++;
        }
        if (got_tan != record[3]) {
            if (tan_mismatches == 0) {
                first_tan_x = record[2];
            }
            tan_mismatches++;
        }
        records++;
    }
    fclose(dump);

    if (records == 0) {
        printf("FAIL: no records read from %s\n", argv[1]);
        return 1;
    }
    printf("records %llu   qm_atan mismatches %llu   qm_tan mismatches %llu\n", records,
           atan_mismatches, tan_mismatches);
    if (atan_mismatches != 0) {
        printf("first qm_atan mismatch at x bits %016llx (%.17e)\n", first_atan_x,
               QM_FROM_BITS(first_atan_x));
    }
    if (tan_mismatches != 0) {
        printf("first qm_tan mismatch at x bits %016llx (%.17e)\n", first_tan_x,
               QM_FROM_BITS(first_tan_x));
    }
    if (atan_mismatches != 0 || tan_mismatches != 0) {
        printf("FAIL: the lanes are not bit-identical\n");
        return 1;
    }
    printf("PASS: %llu samples, every bit identical across lanes\n", records);
    return 0;
}

// Device mirror of noise-compute's `propagation::shared_math` — the model-v2
// node-placement transcendentals `qm_atan` / `qm_tan`, bit-identical to the CPU
// lane.
//
// WHY. Node COUNTS must agree between lanes exactly (the e2-full validator makes
// a count mismatch a hard failure, plan §6.3), and neither CUDA's nor glibc's
// f64 atan/tan is correctly rounded — CUDA documents <= 2 ulp — so a cell
// boundary can land on different sides of a ceil() on the two lanes. This header
// and shared_math.rs are ONE implementation written twice: same reduction, same
// constants, same expression tree under the same QM-ATAN-n / QM-TAN-n labels
// (plan §6.2 label discipline). A diff that touches a labelled expression on one
// lane must show the matching hunk on the other. The Rust file carries the full
// provenance note (FreeBSD/fdlibm s_atan.c + k_tan.c + e_rem_pio2.c in their
// musl form); read it first.
//
// NO FMA, AND WHY IT LOOKS LIKE THIS. nvcc contracts `a*b + c` into an fma by
// default (-fmad=true), and an fma is a DIFFERENT function of the inputs than a
// rounded multiply followed by a rounded add — one contracted expression is
// enough to fork the two lanes. scatter.cu needs -fmad=true for its f32 hot
// loop, so this file cannot ask for a translation-unit flag; instead every
// arithmetic operator goes through QM_ADD/QM_SUB/QM_MUL/QM_DIV, which expand to
// the __dadd_rn/__dsub_rn/__dmul_rn/__ddiv_rn intrinsics that the CUDA C
// Programming Guide defines as round-to-nearest-even and NOT contractible.
// Unary negation and the bit moves are exact and stay bare. The readable form of
// each expression is in the comment above it; the code is that form wrapped.
//
// HOST BUILD. Without __CUDACC__ the same source compiles as plain C99 with the
// operators bare — that is `qm_shared_math_bitcheck.c`, which replays a dump of
// CPU-lane results and compares u64 bit patterns. It MUST be compiled with
// -ffp-contract=off (gcc defaults to `fast`), or the host mirror contracts where
// the device one does not and the comparison tests the wrong thing.
//
// STATUS: SHARED CONTRACT. `qm_streaming_reduction.cuh` layers the reviewed
// decision helpers on this file and scatter.cu includes that one header.
// `qm_shared_math_selftest.cu` remains the independent compile+run check;
// build.rs picks it up automatically because it globs kernels/*.cu.

#ifndef QM_SHARED_MATH_CUH
#define QM_SHARED_MATH_CUH

#ifdef __CUDACC__
#define QM_FN __device__ __forceinline__
#define QM_TABLE __constant__ double
#define QM_ADD(a, b) __dadd_rn((a), (b))
#define QM_SUB(a, b) __dsub_rn((a), (b))
#define QM_MUL(a, b) __dmul_rn((a), (b))
#define QM_DIV(a, b) __ddiv_rn((a), (b))
#define QM_BITS(x) ((unsigned long long)__double_as_longlong(x))
#define QM_FROM_BITS(b) __longlong_as_double((long long)(b))
#else
#include <math.h>
#include <string.h>
#define QM_FN static
#define QM_TABLE static const double
#define QM_ADD(a, b) ((a) + (b))
#define QM_SUB(a, b) ((a) - (b))
#define QM_MUL(a, b) ((a) * (b))
#define QM_DIV(a, b) ((a) / (b))
static unsigned long long qm_host_bits(double x) {
    unsigned long long b;
    memcpy(&b, &x, sizeof(b));
    return b;
}
static double qm_host_from_bits(unsigned long long b) {
    double x;
    memcpy(&x, &b, sizeof(x));
    return x;
}
#define QM_BITS(x) qm_host_bits(x)
#define QM_FROM_BITS(b) qm_host_from_bits(b)
#endif

// ---------------------------------------------------------------------------
// Constants — the fdlibm/musl tables, character-identical to shared_math.rs.
// The hex in each comment is the intended bit pattern; the Rust test
// `constants_match_published_bit_patterns` pins it, and the host bitcheck
// re-pins it on this side.
// ---------------------------------------------------------------------------

QM_TABLE QM_ATANHI[4] = {
    4.63647609000806093515e-01, /* atan(0.5)hi 0x3FDDAC670561BB4F */
    7.85398163397448278999e-01, /* atan(1.0)hi 0x3FE921FB54442D18 */
    9.82793723247329054082e-01, /* atan(1.5)hi 0x3FEF730BD281F69B */
    1.57079632679489655800e+00, /* atan(inf)hi 0x3FF921FB54442D18 */
};

QM_TABLE QM_ATANLO[4] = {
    2.26987774529616870924e-17, /* atan(0.5)lo 0x3C7A2B7F222F65E2 */
    3.06161699786838301793e-17, /* atan(1.0)lo 0x3C81A62633145C07 */
    1.39033110312309984516e-17, /* atan(1.5)lo 0x3C7007887AF0CBBD */
    6.12323399573676603587e-17, /* atan(inf)lo 0x3C91A62633145C07 */
};

QM_TABLE QM_AT[11] = {
    3.33333333333329318027e-01,  /* 0x3FD555555555550D */
    -1.99999999998764832476e-01, /* 0xBFC999999998EBC4 */
    1.42857142725034663711e-01,  /* 0x3FC24924920083FF */
    -1.11111104054623557880e-01, /* 0xBFBC71C6FE231671 */
    9.09088713343650656196e-02,  /* 0x3FB745CDC54C206E */
    -7.69187620504482999495e-02, /* 0xBFB3B0F2AF749A6D */
    6.66107313738753120669e-02,  /* 0x3FB10D66A0D03D51 */
    -5.83357013379057348645e-02, /* 0xBFADDE2D52DEFD9A */
    4.97687799461593236017e-02,  /* 0x3FA97B4B24760DEB */
    -3.65315727442169155270e-02, /* 0xBFA2B4442C6A6C2F */
    1.62858201153657823623e-02,  /* 0x3F90AD3AE322DA11 */
};

QM_TABLE QM_T[13] = {
    3.33333333333334091986e-01,  /* 0x3FD5555555555563 */
    1.33333333333201242699e-01,  /* 0x3FC111111110FE7A */
    5.39682539762260521377e-02,  /* 0x3FABA1BA1BB341FE */
    2.18694882948595424599e-02,  /* 0x3F9664F48406D637 */
    8.86323982359930005737e-03,  /* 0x3F8226E3E96E8493 */
    3.59207910759131235356e-03,  /* 0x3F6D6D22C9560328 */
    1.45620945432529025516e-03,  /* 0x3F57DBC8FEE08315 */
    5.88041240820264096874e-04,  /* 0x3F4344D8F2F26501 */
    2.46463134818469906812e-04,  /* 0x3F3026F71A8D1068 */
    7.81794442939557092300e-05,  /* 0x3F147E88A03792A6 */
    7.14072491382608190305e-05,  /* 0x3F12B80F32F0A7E9 */
    -1.85586374855275456654e-05, /* 0xBEF375CBDB605373 */
    2.59073051863633712884e-05,  /* 0x3EFB2A7074BF7AD4 */
};

#define QM_PIO4 7.85398163397448278999e-01    /* 0x3FE921FB54442D18 */
#define QM_PIO4_LO 3.06161699786838301793e-17 /* 0x3C81A62633145C07 */
#define QM_PIO2_1 1.57079632673412561417e+00  /* 0x3FF921FB54400000 */
#define QM_PIO2_1T 6.07710050650619224932e-11 /* 0x3DD0B4611A626331 */
#define QM_PIO2_2 6.07710050630396597660e-11  /* 0x3DD0B4611A600000 */
#define QM_PIO2_2T 2.02226624879595063154e-21 /* 0x3BA3198A2E037073 */

// ---------------------------------------------------------------------------
// qm_atan — mirrors shared_math.rs `qm_atan`
// ---------------------------------------------------------------------------

QM_FN double qm_atan(double x) {
    const unsigned long long bits = QM_BITS(x);
    const int negative = (int)(bits >> 63);
    const unsigned int ix = ((unsigned int)(bits >> 32)) & 0x7fffffffu;

    // QM-ATAN-1  |x| >= 2^66: atan is pi/2 to well under half an ulp; NaN passes
    // through unchanged.
    if (ix >= 0x44100000u) {
        if (ix > 0x7ff00000u || (ix == 0x7ff00000u && (bits & 0xffffffffull) != 0ull)) {
            return x;
        }
        return negative ? -QM_ATANHI[3] : QM_ATANHI[3];
    }

    // QM-ATAN-2  Reduce |x| into |t| <= 7/16 by one of four identities, recorded
    // in `anchor`:  -1 -> t = x               (|x| < 7/16, no anchor)
    //                0 -> t = (2x-1)/(2+x)     (atan(1/2),  7/16 <= |x| < 11/16)
    //                1 -> t = (x-1)/(x+1)      (atan(1),   11/16 <= |x| < 19/16)
    //                2 -> t = (x-3/2)/(1+3x/2) (atan(3/2), 19/16 <= |x| < 39/16)
    //                3 -> t = -1/x             (atan(inf), 39/16 <= |x| < 2^66)
    double t = x;
    int anchor;
    if (ix < 0x3fdc0000u) {
        // |x| < 2^-27: atan(x) = x - x^3/3 + ... rounds to x, and this branch
        // carries +-0 through with its sign.
        if (ix < 0x3e400000u) {
            return x;
        }
        anchor = -1;
    } else {
        t = QM_FROM_BITS(bits & 0x7fffffffffffffffull); // |x|, exact bit move
        if (ix < 0x3ff30000u) {
            if (ix < 0x3fe60000u) {
                t = QM_DIV(QM_SUB(QM_MUL(2.0, t), 1.0), QM_ADD(2.0, t));
                anchor = 0;
            } else {
                t = QM_DIV(QM_SUB(t, 1.0), QM_ADD(t, 1.0));
                anchor = 1;
            }
        } else if (ix < 0x40038000u) {
            t = QM_DIV(QM_SUB(t, 1.5), QM_ADD(1.0, QM_MUL(1.5, t)));
            anchor = 2;
        } else {
            t = QM_DIV(-1.0, t);
            anchor = 3;
        }
    }

    // QM-ATAN-3  z = t*t ;  z2 = z*z
    const double z = QM_MUL(t, t);
    const double z2 = QM_MUL(z, z);

    // QM-ATAN-4  odd_sum = z*(AT0 + z2*(AT2 + z2*(AT4 + z2*(AT6 + z2*(AT8 + z2*AT10)))))
    const double odd_sum = QM_MUL(
        z,
        QM_ADD(QM_AT[0],
               QM_MUL(z2,
                      QM_ADD(QM_AT[2],
                             QM_MUL(z2,
                                    QM_ADD(QM_AT[4],
                                           QM_MUL(z2,
                                                  QM_ADD(QM_AT[6],
                                                         QM_MUL(z2,
                                                                QM_ADD(QM_AT[8],
                                                                       QM_MUL(z2, QM_AT[10]))))))))))); // clang-format on

    // QM-ATAN-5  even_sum = z2*(AT1 + z2*(AT3 + z2*(AT5 + z2*(AT7 + z2*AT9))))
    const double even_sum = QM_MUL(
        z2,
        QM_ADD(QM_AT[1],
               QM_MUL(z2,
                      QM_ADD(QM_AT[3],
                             QM_MUL(z2,
                                    QM_ADD(QM_AT[5],
                                           QM_MUL(z2, QM_ADD(QM_AT[7], QM_MUL(z2, QM_AT[9])))))))));

    // QM-ATAN-6  unanchored: atan(t) = t - t*(odd_sum + even_sum)
    if (anchor < 0) {
        return QM_SUB(t, QM_MUL(t, QM_ADD(odd_sum, even_sum)));
    }

    // QM-ATAN-7  anchored: ATANHI - ((t*(odd_sum+even_sum) - ATANLO) - t), then
    // an exact sign restore.
    const double result =
        QM_SUB(QM_ATANHI[anchor],
               QM_SUB(QM_SUB(QM_MUL(t, QM_ADD(odd_sum, even_sum)), QM_ATANLO[anchor]), t));
    return negative ? -result : result;
}

// ---------------------------------------------------------------------------
// qm_tan_kernel / qm_tan — mirror shared_math.rs `qm_tan_kernel` / `qm_tan`
// ---------------------------------------------------------------------------

/// Exact, sign-preserving: clear the low 32 significand bits.
QM_FN double qm_zero_low_word(double x) { return QM_FROM_BITS(QM_BITS(x) & 0xffffffff00000000ull); }

/// fdlibm __kernel_tan: tan(x + y) for |x| <~ pi/4, or -1/tan(x + y) when odd==1.
QM_FN double qm_tan_kernel(double x, double y, int odd) {
    const unsigned int hx = (unsigned int)(QM_BITS(x) >> 32);
    const int big = (hx & 0x7fffffffu) >= 0x3fe59428u; // |x| >= 0.6744
    const int negative = (int)(hx >> 31);

    // QM-TAN-3  |x| >= 0.6744: fold through tan(pi/4 - y) = (1-tan y)/(1+tan y)
    // so the polynomial is never evaluated past its 0.67434 fit interval.
    if (big) {
        if (negative) {
            x = -x;
            y = -y;
        }
        x = QM_ADD(QM_SUB(QM_PIO4, x), QM_SUB(QM_PIO4_LO, y));
        y = 0.0;
    }

    // QM-TAN-4  z = x*x ; z2 = z*z ; the degree-27 odd polynomial split into two
    // Horner chains in z2.
    //   poly_hi = T1 + z2*(T3 + z2*(T5 + z2*(T7 + z2*(T9 + z2*T11))))
    //   poly_lo = z *(T2 + z2*(T4 + z2*(T6 + z2*(T8 + z2*(T10 + z2*T12)))))
    const double z = QM_MUL(x, x);
    const double z2 = QM_MUL(z, z);
    const double poly_hi = QM_ADD(
        QM_T[1],
        QM_MUL(z2,
               QM_ADD(QM_T[3],
                      QM_MUL(z2,
                             QM_ADD(QM_T[5],
                                    QM_MUL(z2,
                                           QM_ADD(QM_T[7],
                                                  QM_MUL(z2, QM_ADD(QM_T[9], QM_MUL(z2, QM_T[11]))))))))));
    const double poly_lo = QM_MUL(
        z,
        QM_ADD(QM_T[2],
               QM_MUL(z2,
                      QM_ADD(QM_T[4],
                             QM_MUL(z2,
                                    QM_ADD(QM_T[6],
                                           QM_MUL(z2,
                                                  QM_ADD(QM_T[8],
                                                         QM_MUL(z2,
                                                                QM_ADD(QM_T[10],
                                                                       QM_MUL(z2, QM_T[12]))))))))))); // clang-format on

    // QM-TAN-5  s = z*x ; tail = y + z*(s*(poly_hi+poly_lo) + y) + s*T0 ;
    //           sum = x + tail
    const double s = QM_MUL(z, x);
    const double tail = QM_ADD(QM_ADD(y, QM_MUL(z, QM_ADD(QM_MUL(s, QM_ADD(poly_hi, poly_lo)), y))),
                               QM_MUL(s, QM_T[0]));
    const double sum = QM_ADD(x, tail);

    // QM-TAN-6  folded branch: undo the pi/4 fold, choosing tan or -1/tan by
    // `odd`, then restore the sign.
    //   sign_flip = 1 - 2*odd
    //   folded    = sign_flip - 2*(x + (tail - sum*sum/(sum + sign_flip)))
    if (big) {
        const double sign_flip = QM_SUB(1.0, QM_MUL(2.0, (double)odd));
        const double folded = QM_SUB(
            sign_flip,
            QM_MUL(2.0,
                   QM_ADD(x, QM_SUB(tail, QM_DIV(QM_MUL(sum, sum), QM_ADD(sum, sign_flip))))));
        return negative ? -folded : folded;
    }
    if (odd == 0) {
        return sum;
    }

    // QM-TAN-7  cotangent arm: -1/(x+tail) carries up to 2 ulp, so refine once.
    //   correction = tail - (sum_hi - x)
    //   result     = reciprocal_hi
    //              + reciprocal*(1 + reciprocal_hi*sum_hi + reciprocal_hi*correction)
    const double sum_hi = qm_zero_low_word(sum);
    const double correction = QM_SUB(tail, QM_SUB(sum_hi, x));
    const double reciprocal = QM_DIV(-1.0, sum);
    const double reciprocal_hi = qm_zero_low_word(reciprocal);
    return QM_ADD(reciprocal_hi,
                  QM_MUL(reciprocal,
                         QM_ADD(QM_ADD(1.0, QM_MUL(reciprocal_hi, sum_hi)),
                                QM_MUL(reciprocal_hi, correction))));
}

/// tan(x) for x in (-pi/2, pi/2) — the only range the node generator uses.
/// Total and deterministic outside it (correct out to |x| < 3pi/4, merely
/// inaccurate beyond), but only the open half-turn is contracted. The Rust lane
/// carries a debug_assert here; the device has no panic path, which is the one
/// deliberate structural difference between the lanes.
QM_FN double qm_tan(double x) {
    const unsigned long long bits = QM_BITS(x);
    const unsigned int ix = ((unsigned int)(bits >> 32)) & 0x7fffffffu;

    // QM-TAN-1  |x| < 2^-27: tan(x) rounds to x; also carries +-0 with its sign.
    if (ix < 0x3e400000u) {
        return x;
    }

    // Both kernels are exactly odd, so peeling the sign here and restoring it at
    // the end reproduces musl's signed paths bit for bit with half the branches.
    const double a = QM_FROM_BITS(bits & 0x7fffffffffffffffull);

    double magnitude;
    if (ix <= 0x3fe921fbu) {
        // |x| <~ pi/4: the kernel's own domain, no reduction.
        magnitude = qm_tan_kernel(a, 0.0, 0);
    } else {
        // QM-TAN-2  pi/4 < |x| < pi/2: one-quadrant reduction y = |x| - pi/2 < 0,
        // then -1/tan(y). musl's rem_pio2 medium path with the quadrant count
        // folded in as the literal 1 (multiplying by 1.0 is exact, so these are
        // musl's expressions unchanged); `r` is exact by Sterbenz. The second
        // round fires when the first cancels more than 16 binary digits. musl's
        // third round is dead on this domain and is not ported — see the Rust
        // file for the numeric argument.
        double r = QM_SUB(a, QM_PIO2_1);
        double w = QM_PIO2_1T;
        double y0 = QM_SUB(r, w);
        const int exponent_of_x = (int)(ix >> 20);
        const int exponent_of_y = ((int)(QM_BITS(y0) >> 52)) & 0x7ff;
        if (exponent_of_x - exponent_of_y > 16) {
            const double t = r;
            w = QM_PIO2_2;
            r = QM_SUB(t, w);
            w = QM_SUB(QM_PIO2_2T, QM_SUB(QM_SUB(t, r), w));
            y0 = QM_SUB(r, w);
        }
        const double y1 = QM_SUB(QM_SUB(r, y0), w);
        magnitude = qm_tan_kernel(y0, y1, 1);
    }

    return (bits >> 63) != 0ull ? -magnitude : magnitude;
}

// ---------------------------------------------------------------------------
// qm_atan2 / qm_wrap_pi — mirror shared_math.rs decision helpers
// ---------------------------------------------------------------------------

QM_FN double qm_atan2(double y, double x) {
    const double pi = QM_FROM_BITS(0x400921fb54442d18ull);
    const double half_pi = QM_FROM_BITS(0x3ff921fb54442d18ull);
    const double canonical_nan = QM_FROM_BITS(0x7ff8000000000000ull);

    // QM-ATAN2-1
    if (!isfinite(x) || !isfinite(y)) {
        return canonical_nan;
    }
    const unsigned long long x_bits = QM_BITS(x);
    const unsigned long long y_bits = QM_BITS(y);
    const int x_negative = (int)(x_bits >> 63);
    const int y_negative = (int)(y_bits >> 63);

    // QM-ATAN2-2
    if (y == 0.0) {
        if (x_negative) {
            return y_negative ? -pi : pi;
        }
        return y;
    }
    if (x == 0.0) {
        return y_negative ? -half_pi : half_pi;
    }

    const double ax = QM_FROM_BITS(x_bits & 0x7fffffffffffffffull);
    const double ay = QM_FROM_BITS(y_bits & 0x7fffffffffffffffull);

    // QM-ATAN2-3
    const double acute = ax >= ay ? qm_atan(QM_DIV(ay, ax))
                                 : QM_SUB(half_pi, qm_atan(QM_DIV(ax, ay)));

    // QM-ATAN2-4
    const double magnitude = x_negative ? QM_SUB(pi, acute) : acute;
    return y_negative ? -magnitude : magnitude;
}

QM_FN double qm_wrap_pi(double angle) {
    const double pi = QM_FROM_BITS(0x400921fb54442d18ull);
    const double tau = QM_FROM_BITS(0x401921fb54442d18ull);
    const double canonical_nan = QM_FROM_BITS(0x7ff8000000000000ull);

    // QM-WRAP-1
    if (!isfinite(angle) || angle < -tau || angle > tau) {
        return canonical_nan;
    }
    // QM-WRAP-2
    if (angle > pi) {
        return QM_SUB(angle, tau);
    }
    if (angle <= -pi) {
        return QM_ADD(angle, tau);
    }
    return angle;
}

#endif // QM_SHARED_MATH_CUH

// Standalone device self-test for qm_shared_math.cuh — evaluates the shared
// transcendentals over a host-supplied sample array so the GPU owner can
// bit-compare against the CPU lane's dump.
//
// Independent model-v2 §6 contract gate: it keeps the base transcendentals
// separately compiled and run even though scatter.cu now reaches this header
// through qm_streaming_reduction.cuh.
//
// build.rs globs kernels/*.cu, so `cargo build --features gpu` already emits
// $OUT_DIR/qm_shared_math_selftest.ptx — no wiring needed. A bare syntax check
// without the crate is `nvcc -ptx -arch=sm_89 kernels/qm_shared_math_selftest.cu`.
//
// Evidence procedure (the CPU lane produces the reference; see the
// `cross_lane_sample_dump` test in noise-compute's propagation/shared_math.rs):
//
//   QM_SHARED_MATH_DUMP=/tmp/qm.bin cargo test -p noise-compute shared_math
//
// gives 10^6 little-endian u64 quadruples [x, qm_atan(x), u, qm_tan(u)]. Upload
// columns 0 and 2, launch this kernel, and compare the returned bit patterns
// against columns 1 and 3 — the pass criterion is ZERO differing bits, not a
// tolerance.

#include "qm_shared_math.cuh"

extern "C" __global__ void qm_shared_math_selftest(const double *__restrict__ atan_in,
                                                   const double *__restrict__ tan_in,
                                                   double *__restrict__ atan_out,
                                                   double *__restrict__ tan_out, int n) {
    const int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= n) {
        return;
    }
    atan_out[i] = qm_atan(atan_in[i]);
    tan_out[i] = qm_tan(tan_in[i]);
}

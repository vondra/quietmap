// Same aircraft energy kernel at explicit cruise lattice or facade receivers.
#include <cuda_runtime.h>
#include <cmath>
#include "airborne_defines.cuh"
#include "airborne_energy.cuh"

struct CruiseSource {
    double geography[6];
    double physical[13]; // + power_w [11], heli_db [12]
    int identity[5]; // installation, class, departure, period, power_row
};
struct CruiseReceiver {
    double latitude, longitude, model_metres_per_longitude_degree, altitude;
    double gate_metres_per_longitude_degree;
};
static_assert(sizeof(CruiseSource) == 176);
static_assert(sizeof(CruiseReceiver) == 40);

__device__ double cruise_longitude_delta(double from, double to) {
    double delta = to - from;
    if (delta >= 180.0) delta -= 360.0;
    if (delta < -180.0) delta += 360.0;
    return delta;
}

__global__ void cruise_parts(const CruiseSource* sources, unsigned int source_count,
    const CruiseReceiver* receivers, const double* npd, unsigned int parts, double* output)
{
    __shared__ double sums[3][256];
    unsigned int receiver = blockIdx.x;
    const CruiseReceiver rx = receivers[receiver];
    unsigned int first = blockIdx.y * AIRBORNE_REDUCTION_ROWS;
    unsigned int end = min(first + AIRBORNE_REDUCTION_ROWS, source_count);
    double energy[3] = {};
    for (unsigned int row = first + threadIdx.x; row < end; row += blockDim.x) {
        const CruiseSource& source = sources[row];
        double north = (source.geography[2] - rx.latitude) * MLAT;
        double east = cruise_longitude_delta(rx.longitude, source.geography[3]) * rx.gate_metres_per_longitude_degree;
        double reach = AIRBORNE_REACH_M + source.geography[4];
        if (north * north + east * east > reach * reach) continue;
        double ax = cruise_longitude_delta(rx.longitude, source.geography[1]) * rx.model_metres_per_longitude_degree;
        double ay = (source.geography[0] - rx.latitude) * MLAT;
        double dx = source.physical[1] * rx.model_metres_per_longitude_degree;
        double sel;
        const AirborneScreen no_screen{};
        if (aircraft_sel<double, false>(ax, ay, dx, source.physical, source.identity[1], source.identity[2],
            source.identity[0], source.identity[4], rx.altitude, npd,
            npd + 2 * NPD_NC * NPD_NR * (NPD_NB + 1),
            0, no_screen, nullptr, &sel)) {
            energy[source.identity[3]] += aircraft_fast_exp(sel * LN10 * 0.1) * source.geography[5];
        }
    }
    for (int period = 0; period < 3; period++) sums[period][threadIdx.x] = energy[period];
    __syncthreads();
    for (unsigned int stride = 128; stride > 0; stride >>= 1) {
        if (threadIdx.x < stride) {
            for (int period = 0; period < 3; period++) sums[period][threadIdx.x] += sums[period][threadIdx.x + stride];
        }
        __syncthreads();
    }
    if (threadIdx.x == 0) {
        for (int period = 0; period < 3; period++) output[((size_t)receiver * parts + blockIdx.y) * 3 + period] = sums[period][0];
    }
}
__global__ void cruise_reduce(const double* partial, unsigned int receivers, unsigned int parts, double* output) {
    unsigned int index = blockIdx.x * blockDim.x + threadIdx.x;
    if (index >= receivers * 3) return;
    unsigned int receiver = index / 3, period = index % 3;
    double sum = 0.0;
    for (unsigned int part = 0; part < parts; part++) sum += partial[((size_t)receiver * parts + part) * 3 + period];
    output[index] = sum;
}
extern "C" int relevant_source_cuda_cruise(const CruiseSource* sources, unsigned int source_count,
    const CruiseReceiver* receivers, unsigned int receiver_count, const double* npd, double* partial, double* output)
{
    if (!source_count || !receiver_count) return cudaErrorInvalidValue;
    unsigned int parts = (source_count - 1) / AIRBORNE_REDUCTION_ROWS + 1;
    if (parts > 65535 || receiver_count > 65535) return cudaErrorInvalidValue;
    cruise_parts<<<dim3(receiver_count, parts), 256>>>(sources, source_count, receivers, npd, parts, partial);
    cudaError_t status = cudaGetLastError();
    if (status != cudaSuccess) return status;
    cruise_reduce<<<(receiver_count * 3 + 255) / 256, 256>>>(partial, receiver_count, parts, output);
    return cudaGetLastError();
}

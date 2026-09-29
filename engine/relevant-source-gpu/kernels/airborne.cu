// Screened independent events and split chords at explicit receivers.
#include <cuda_runtime.h>
#include <cmath>
#include "airborne_defines.cuh"
#include "airborne_energy.cuh"

struct AirborneSource {
    float endpoints[4]; // start lat/lon, end lat/lon at prepared f32 precision
    float physical[13]; // + power_w [11], heli_db [12]
    int identity[6]; // installation, class, departure, period, secondary-only provenance, power_row
};
struct AirborneReceiver {
    double latitude, longitude, metres_per_longitude_degree;
    float altitude;
    unsigned int padding;
};
static_assert(sizeof(AirborneSource) == 92);
static_assert(sizeof(AirborneReceiver) == 32);

template<typename Source>
__device__ bool airborne_row_in_envelope(const Source& source, const AirborneReceiver& rx) {
    float south = (float)(rx.latitude - AIRBORNE_REACH_M / MLAT);
    float north = (float)(rx.latitude + AIRBORNE_REACH_M / MLAT);
    if (fmaxf(source.endpoints[0], source.endpoints[2]) < south ||
        fminf(source.endpoints[0], source.endpoints[2]) > north) return false;
    double start = source.endpoints[1];
    double delta = (double)source.endpoints[3] - start;
    if (delta > 180.0) delta -= 360.0;
    if (delta < -180.0) delta += 360.0;
    double end = start + delta;
    for (int turn = -1; turn <= 1; turn++) {
        float west = (float)(rx.longitude + turn * 360.0 - AIRBORNE_REACH_M / rx.metres_per_longitude_degree);
        float east = (float)(rx.longitude + turn * 360.0 + AIRBORNE_REACH_M / rx.metres_per_longitude_degree);
        if (fmax(start, end) >= west && fmin(start, end) <= east) return true;
    }
    return false;
}

// Preserve the CPU CPA arithmetic at discrete horizon-sector boundaries.
__device__ void airborne_screen_geometry(const AirborneSource& source, const AirborneReceiver& rx, float* result) {
    double longitude_delta = (double)source.endpoints[3] - source.endpoints[1];
    if (longitude_delta > 180.0) longitude_delta -= 360.0;
    if (longitude_delta < -180.0) longitude_delta += 360.0;
    double start_delta = (double)source.endpoints[1] - rx.longitude;
    if (start_delta > 180.0) start_delta -= 360.0;
    if (start_delta < -180.0) start_delta += 360.0;
    double ax = start_delta * rx.metres_per_longitude_degree;
    double ay = ((double)source.endpoints[0] - rx.latitude) * MLAT;
    double dx = longitude_delta * rx.metres_per_longitude_degree;
    double dy = ((double)source.endpoints[2] - source.endpoints[0]) * MLAT;
    double length_squared = dx * dx + dy * dy;
    double inverse = length_squared > 1e-6 ? 1.0 / length_squared : 0.0;
    double t = -(ax * dx + ay * dy) * inverse;
    double cpx = ax + t * dx, cpy = ay + t * dy;
    double relative_alt = source.physical[0] + t * source.physical[3] - rx.altitude;
    double physical_t = fmin(fmax(t, 0.0), 1.0);
    result[0] = (float)cpx; result[1] = (float)cpy; result[2] = (float)relative_alt;
    result[3] = (float)(cpx * cpx + cpy * cpy + relative_alt * relative_alt);
    result[4] = (float)(ax + physical_t * dx); result[5] = (float)(ay + physical_t * dy);
    result[6] = (float)(source.physical[0] + physical_t * source.physical[3] - rx.altitude);
}

// A block stages consecutive rows in shared memory once and evaluates each against
// AIRBORNE_BLOCK_RECEIVERS neighbouring receivers: one warp takes one row for 32 receivers,
// so its branches agree and the row is read from the L2 once per block, not once per receiver.
#define AIRBORNE_BLOCK_RECEIVERS 32
#define AIRBORNE_ROW_LANES 8
#define AIRBORNE_STAGED_ROWS (AIRBORNE_BLOCK_RECEIVERS * AIRBORNE_ROW_LANES)
__global__ void airborne_independent_parts(
    const AirborneSource* sources, unsigned int source_count,
    const AirborneReceiver* receivers, unsigned int receiver_count, const float* npd, const float* weights,
    AirborneScreen screen, unsigned int parts, float* output)
{
    __shared__ AirborneSource staged[AIRBORNE_STAGED_ROWS];
    __shared__ float sums[3][AIRBORNE_ROW_LANES][AIRBORNE_BLOCK_RECEIVERS];
    unsigned int column = threadIdx.x % AIRBORNE_BLOCK_RECEIVERS;
    unsigned int lane = threadIdx.x / AIRBORNE_BLOCK_RECEIVERS;
    unsigned int receiver = blockIdx.x * AIRBORNE_BLOCK_RECEIVERS + column;
    bool active = receiver < receiver_count;
    const AirborneReceiver rx = receivers[active ? receiver : 0];
    unsigned int first = blockIdx.y * AIRBORNE_REDUCTION_ROWS;
    unsigned int end = min(first + AIRBORNE_REDUCTION_ROWS, source_count);
    float energy[3] = {};
    for (unsigned int chunk = first; chunk < end; chunk += AIRBORNE_STAGED_ROWS) {
        __syncthreads();
        if (chunk + threadIdx.x < end) staged[threadIdx.x] = sources[chunk + threadIdx.x];
        __syncthreads();
        unsigned int staged_rows = min(AIRBORNE_STAGED_ROWS, end - chunk);
        for (unsigned int row = lane; active && row < staged_rows; row += AIRBORNE_ROW_LANES) {
            const AirborneSource& source = staged[row];
            if (!airborne_row_in_envelope(source, rx)) continue;
            float ax = airborne_offset_east(source.endpoints[1], rx.longitude, rx.metres_per_longitude_degree);
            float ay = airborne_row_offset_north(source.endpoints[0], rx.latitude);
            float dx = airborne_row_segment_dx(source.physical[1], rx.metres_per_longitude_degree);
            float sel;
            float screen_geometry[7];
            airborne_screen_geometry(source, rx, screen_geometry);
            if (aircraft_sel<float, true>(ax, ay, dx, source.physical, source.identity[1], source.identity[2],
                             source.identity[0], source.identity[5], rx.altitude, npd,
                             npd + 2 * NPD_NC * NPD_NR * (NPD_NB + 1),
                             receiver, screen, screen_geometry, &sel)) {
                energy[source.identity[3]] += aircraft_fast_exp(sel * (float)LN10 * 0.1f) * weights[source.identity[4]];
            }
        }
    }
    for (int period = 0; period < 3; period++) sums[period][lane][column] = energy[period];
    __syncthreads();
    if (lane == 0 && active) {
        for (int period = 0; period < 3; period++) {
            float total = 0.0f;
            for (unsigned int l = 0; l < AIRBORNE_ROW_LANES; l++) total += sums[period][l][column];
            output[((size_t)receiver * parts + blockIdx.y) * 3 + period] = total;
        }
    }
}
__global__ void airborne_independent_reduce(const float* partial, unsigned int receivers,
    unsigned int parts, float days, float* output)
{
    unsigned int index = blockIdx.x * blockDim.x + threadIdx.x;
    if (index >= receivers * 3) return;
    unsigned int receiver = index / 3, period = index % 3;
    double sum = 0.0;
    for (unsigned int part = 0; part < parts; part++) sum += partial[((size_t)receiver * parts + part) * 3 + period];
    const double seconds[3] = AIRBORNE_PERIOD_SECONDS;
    output[index] = (float)(sum / (days * seconds[period]));
}
extern "C" int relevant_source_cuda_airborne(
    const AirborneSource* sources, unsigned int source_count,
    const AirborneReceiver* receivers, unsigned int receiver_count,
    const float* npd, const float* weights, const AirborneScreen* screen,
    float days, float* partial, float* output)
{
    if (!source_count || !receiver_count || receiver_count != screen->records || !(days > 0.0f)) return cudaErrorInvalidValue;
    unsigned int parts = (source_count - 1) / AIRBORNE_REDUCTION_ROWS + 1;
    if (parts > 65535 || receiver_count > 65535) return cudaErrorInvalidValue;
    airborne_independent_parts<<<dim3((receiver_count - 1) / AIRBORNE_BLOCK_RECEIVERS + 1, parts), AIRBORNE_STAGED_ROWS>>>(
        sources, source_count, receivers, receiver_count, npd, weights, *screen, parts, partial);
    cudaError_t status = cudaGetLastError();
    if (status != cudaSuccess) return status;
    airborne_independent_reduce<<<(receiver_count * 3 + 255) / 256, 256>>>(partial, receiver_count, parts, days, output);
    return cudaGetLastError();
}

#include "airborne_chords.cuh"

// Split chords retain receiver-dependent chain breaks and source-order floor sums.
#pragma once
#include <climits>
struct ChordSource {
    float endpoints[4];
    double physical[13]; // + power_w [11], heli_db [12]
    int identity[6]; // installation, class, departure, period, secondary-only provenance, power_row
};
static_assert(sizeof(ChordSource) == 144);

__global__ void airborne_chord_evaluate(const ChordSource* sources, unsigned int rows,
    const AirborneReceiver* receivers, const double* npd, AirborneScreen screen,
    double* levels, unsigned int* accepted)
{
    for (unsigned int row = blockIdx.y * blockDim.x + threadIdx.x; row < rows; row += blockDim.x * gridDim.y) {
        unsigned int receiver = blockIdx.x;
        size_t index = (size_t)receiver * rows + row;
        levels[index * 2] = -INFINITY;
        const ChordSource& source = sources[row];
        if (source.identity[1] < 0) continue;
        const AirborneReceiver rx = receivers[receiver];
        if (!airborne_row_in_envelope(source, rx)) continue;
        double delta = (double)source.endpoints[1] - rx.longitude;
        if (delta > 180.0) delta -= 360.0;
        if (delta < -180.0) delta += 360.0;
        double ax = delta * rx.metres_per_longitude_degree;
        double ay = ((double)source.endpoints[0] - rx.latitude) * MLAT;
        double by = ((double)source.endpoints[2] - rx.latitude) * MLAT;
        double dx = source.physical[1] * rx.metres_per_longitude_degree;
        double f[13];
        for (int i = 0; i < 13; i++) f[i] = source.physical[i];
        f[2] = by - ay; // Keep the canonical CPU subtraction order.
        double length_squared = dx * dx + f[2] * f[2];
        double cross = ax * f[2] - ay * dx;
        if (length_squared > 1.0 && cross * cross > f[8] * length_squared) continue;
        double inverse = length_squared > 1e-6 ? 1.0 / length_squared : 0.0;
        double t = -(ax * dx + ay * f[2]) * inverse;
        double cpx = ax + t * dx, cpy = ay + t * f[2];
        double relative = f[0] + t * f[3] - rx.altitude;
        double physical_t = fmin(fmax(t, 0.0), 1.0);
        float geometry[7] = {(float)cpx, (float)cpy, (float)relative,
            (float)(cpx*cpx + cpy*cpy + relative*relative),
            (float)(ax + physical_t * dx), (float)(ay + physical_t * f[2]),
            (float)(f[0] + physical_t * f[3] - rx.altitude)};
        double sel, free_sel;
        if (aircraft_sel<double, true, false>(ax, ay, dx, f, source.identity[1], source.identity[2],
            source.identity[0], source.identity[5], (double)rx.altitude, npd,
            npd + 2 * NPD_NC * NPD_NR * (NPD_NB + 1),
            receiver, screen, geometry, &sel, &free_sel)) {
            levels[index * 2] = sel;
            levels[index * 2 + 1] = free_sel;
            atomicAdd(accepted + receiver, 1u); // Integer only; acoustic sums stay deterministic.
        }
    }
}


__global__ void airborne_chord_heads(const unsigned int* ranges, const unsigned int* predecessors,
    unsigned int rows, const double* levels, const unsigned int* accepted, unsigned int* heads)
{
    for (unsigned int row = blockIdx.y * blockDim.x + threadIdx.x; row < rows; row += blockDim.x * gridDim.y) {
        size_t first = (size_t)blockIdx.x * rows;
        if (!isfinite(levels[(first + row) * 2])) { heads[first + row] = UINT_MAX; continue; }
        unsigned int head = row, steps = 0;
        while (true) {
            unsigned int previous = UINT_MAX;
            for (unsigned int j = 0; j < ranges[head * 4 + 1]; j++) {
                unsigned int candidate = predecessors[ranges[head * 4] + j];
                if (isfinite(levels[(first + candidate) * 2])) { previous = candidate; break; }
            }
            if (previous == UINT_MAX || previous == head || steps >= accepted[blockIdx.x]) break;
            head = previous;
            steps++;
        }
        heads[first + row] = head;
    }
}


__global__ void airborne_chord_parts(const ChordSource* sources, const unsigned int* ranges,
    const unsigned int* members, unsigned int rows, const double* levels,
    const unsigned int* heads, const double* weights, unsigned int parts, double* partial)
{
    __shared__ double sums[3][256];
    unsigned int receiver = blockIdx.x;
    size_t first = (size_t)receiver * rows;
    unsigned int begin = blockIdx.y * AIRBORNE_REDUCTION_ROWS;
    unsigned int end = min(begin + AIRBORNE_REDUCTION_ROWS, rows);
    double energy[3] = {};
    for (unsigned int head = begin + threadIdx.x; head < end; head += blockDim.x) {
        // A cycle's final head need not itself map to that head. Every row can
        // own a group; the member scan below, not heads[head], determines it.
        double raw = 0.0, free_raw = 0.0, periods[3] = {};
        for (unsigned int j = 0; j < ranges[head * 4 + 3]; j++) {
            unsigned int row = members[ranges[head * 4 + 2] + j];
            if (heads[first + row] != head) continue;
            double sel = levels[(first + row) * 2];
            raw += pow(10.0, sel / 10.0);
            free_raw += pow(10.0, levels[(first + row) * 2 + 1] / 10.0);
            periods[sources[row].identity[3]] += aircraft_fast_exp(sel * LN10 * 0.1)
                * weights[sources[row].identity[4]];
        }
        if (10.0 * log10(raw) < 20.0 || 10.0 * log10(free_raw) < 20.0) continue;
        for (int p = 0; p < 3; p++) energy[p] += periods[p];
    }
    for (int p = 0; p < 3; p++) sums[p][threadIdx.x] = energy[p];
    __syncthreads();
    for (unsigned int stride = 128; stride; stride >>= 1) {
        if (threadIdx.x < stride)
            for (int p = 0; p < 3; p++) sums[p][threadIdx.x] += sums[p][threadIdx.x + stride];
        __syncthreads();
    }
    if (!threadIdx.x)
        for (int p = 0; p < 3; p++) partial[((size_t)receiver * parts + blockIdx.y) * 3 + p] = sums[p][0];
}

__global__ void airborne_chord_reduce(const double* partial, unsigned int receivers,
    unsigned int parts, double days, float* output)
{
    unsigned int index = blockIdx.x * blockDim.x + threadIdx.x;
    if (index >= receivers * 3) return;
    unsigned int receiver = index / 3, period = index % 3;
    double sum = 0.0;
    for (unsigned int part = 0; part < parts; part++) sum += partial[((size_t)receiver * parts + part) * 3 + period];
    const double seconds[3] = AIRBORNE_PERIOD_SECONDS;
    output[index] = (float)(sum / (days * seconds[period]));
}

extern "C" int relevant_source_cuda_airborne_chords(const ChordSource* sources, unsigned int rows,
    const unsigned int* ranges, const unsigned int* predecessors, const unsigned int* members,
    const AirborneReceiver* receivers, unsigned int receiver_count, const double* npd,
    const double* weights, const AirborneScreen* screen, double days,
    double* levels, unsigned int* heads, unsigned int* accepted, double* partial, float* output)
{
    if (!rows || !receiver_count || receiver_count != screen->records || !(days > 0.0)) return cudaErrorInvalidValue;
    unsigned int row_blocks = min((rows - 1) / 256 + 1, 65535u);
    unsigned int parts = (rows - 1) / AIRBORNE_REDUCTION_ROWS + 1;
    if (parts > 65535 || receiver_count > 65535) return cudaErrorInvalidValue;
    cudaError_t status = cudaMemset(accepted, 0, receiver_count * sizeof(unsigned int));
    if (status != cudaSuccess) return status;
    airborne_chord_evaluate<<<dim3(receiver_count, row_blocks), 256>>>(sources, rows, receivers, npd, *screen, levels, accepted);
    status = cudaGetLastError(); if (status != cudaSuccess) return status;
    airborne_chord_heads<<<dim3(receiver_count, row_blocks), 256>>>(ranges, predecessors, rows, levels, accepted, heads);
    status = cudaGetLastError(); if (status != cudaSuccess) return status;
    airborne_chord_parts<<<dim3(receiver_count, parts), 256>>>(sources, ranges, members, rows, levels, heads, weights, parts, partial);
    status = cudaGetLastError(); if (status != cudaSuccess) return status;
    airborne_chord_reduce<<<(receiver_count * 3 + 255) / 256, 256>>>(partial, receiver_count, parts, days, output);
    return cudaGetLastError();
}

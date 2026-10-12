// A painted pixel's loudness on the card in f32, at the map's resolution, after distribution.cuh:
// each state's layers into the period's mixtures, the place's their sum from the steady layers'
// energy (popup/src/percentiles.rs layered_distributions); then each mixture on its curve
// (popup/src/loudness.rs loudness), ISO 532-1 (physics/src/loudness.rs) at the whole decibels the
// levels need. Two kernels: the mixtures a warp a task, then the loudness a block a pixel.

// The floats of a pixel's mixtures: per period and curve (the layers', then the place's) and bin
// its share of the time and that share times its mean intensity.
#define LOUD_MIXTURE_FLOATS (PERIODS * LOUD_CURVES * LOUD_BINS * 2)

// Adds `d` times `weight` into a mixture's bins, the lanes a bin each in turn.
__device__ void loud_accumulate(float* mixture, const LoudDistribution& d, float weight, int lane) {
    for (int b = d.low + lane; b <= d.high; b += LOUD_LANES) {
        if (d.bins[b][0] > 0.0f) {
            atomicAdd(&mixture[2 * b], weight * d.bins[b][0]);
            atomicAdd(&mixture[2 * b + 1], weight * d.bins[b][1]);
        }
    }
}

// The pixels' mixtures: a warp a task (a pixel's period, group of hours and weather state,
// LOUD_TASK_*), each layer's distribution and the place's added with the task's share of the period
// into the pixel's mixtures, which start at zero. A pixel none of whose lines follows the hours has
// one group a period: its tasks of the other groups do nothing.
extern "C" __global__ void pixel_mixtures(const float* input, const u64* offsets, u32 task_count, float* mixtures) {
    __shared__ LoudWarp warps[LOUD_TASK_WARPS];
    int lane = threadIdx.x % LOUD_LANES;
    LoudWarp& w = warps[threadIdx.x / LOUD_LANES];
    u32 task = blockIdx.x * LOUD_TASK_WARPS + threadIdx.x / LOUD_LANES;
    if (task >= task_count) return;
    u32 pixel = task / LOUD_TASKS;
    int k = task % LOUD_TASKS;
    int p = LOUD_TASK_PERIOD[k], group = LOUD_TASK_GROUP[k], state = LOUD_TASK_STATE[k];
    const float* in = input + offsets[pixel];
    int start[LOUD_LAYERS], count[LOUD_LAYERS];
    bool profiled = loud_layers(in, start, count);
    if (!profiled && group > 0) return;
    float weight = profiled ? (float)LOUD_GROUP_SLOTS[p * LOUD_GROUPS_MAX + group] / (LOUD_PERIOD_HOURS[p] * LOUD_STATES)
                            : 1.0f / LOUD_STATES;
    float* mixture = mixtures + (u64)pixel * LOUD_MIXTURE_FLOATS + (u64)p * LOUD_CURVES * LOUD_BINS * 2;
    // What holds steady in every layer at once, then the layers that move.
    float floors[LOUD_LAYERS], held[LOUD_LAYERS];
    bool moves[LOUD_LAYERS];
    float floor = 0.0f;
    for (int l = 0; l < LOUD_LAYERS; l++) {
        floors[l] = loud_floor(in + start[l], count[l], in[start[l] - PERIODS - 1 + p], p, group, state, &held[l], &moves[l]);
        if (!moves[l]) floor += floors[l];
    }
    loud_steady(floor, w.whole[0], lane);
    int whole = 0;
    for (int l = 0; l < LOUD_LAYERS; l++) {
        int own = loud_layer(in + start[l], count[l], floors[l], held[l], moves[l], p, group, state, w, lane);
        loud_accumulate(mixture + l * LOUD_BINS * 2, w.layer[own], weight, lane);
        if (moves[l]) {
            loud_with_distribution(w.whole[whole], w.layer[own], w.values, w.whole[1 - whole], lane);
            whole = 1 - whole;
        }
    }
    loud_accumulate(mixture + LOUD_LAYERS * LOUD_BINS * 2, w.whole[whole], weight, lane);
}

// physics/src/loudness.rs core_loudness and zwicker_loudness_sone: ISO 532-1 of third-octave
// levels (dB).
__device__ float loud_zwicker(const float* levels_in) {
    float levels[LOUD_THIRDS];
    for (int k = 0; k < LOUD_THIRDS; k++) levels[k] = fmaxf(levels_in[k], LOUD_SILENT_DB);
    float corrected[11];
    for (int band = 0; band < 11; band++) {
        float level = levels[band];
        int range = 7;
        for (int r = 0; r < 8; r++) {
            if (level <= LOUD_RAP[r] - LOUD_DLL[r * 11 + band]) {
                range = r;
                break;
            }
        }
        corrected[band] = energy_of_level(level + LOUD_DLL[range * 11 + band]);
    }
    float lowest[3];
    const int first[4] = {0, 6, 9, 11};
    for (int k = 0; k < 3; k++) {
        float sum = 0.0f;
        for (int b = first[k]; b < first[k + 1]; b++) sum += corrected[b];
        lowest[k] = sum > 0.0f ? 10.0f * log10f(sum) : -QM_INFINITY;
    }
    float core[21];
    for (int band = 0; band < 21; band++) core[band] = 0.0f;
    for (int band = 0; band < 20; band++) {
        float level = (band < 3 ? lowest[band] : levels[band + 8]) - LOUD_A0[band];
        if (level > LOUD_LTQ[band]) {
            level -= LOUD_DCB[band];
            const float s = 0.25f;
            float threshold = energy_of_level(LOUD_LTQ[band] / 4.0f);
            core[band] = fmaxf(0.0635f * threshold * (sqrtf(sqrtf(1.0f - s + s * energy_of_level(level - LOUD_LTQ[band]))) - 1.0f), 0.0f);
        }
    }
    float correction = 0.4f + 0.32f * powf(core[0], 0.2f);
    if (correction <= 1.0f) core[0] *= correction;
    float total = 0.0f, z1 = 0.0f, n1 = 0.0f;
    for (int band = 0; band < 21; band++) {
        float upper = LOUD_ZUP[band] + 0.0001f;
        int group = min(max(band - 1, 0), 7);
        float c = core[band];
        int guard = 0;
        while (z1 < upper && guard++ < 64) {
            if (n1 <= c) {
                total += c * (upper - z1);
                z1 = upper;
                n1 = c;
            } else {
                int range = 17;
                for (int r = 0; r < 18; r++) {
                    if (n1 > LOUD_RNS[r]) {
                        range = r;
                        break;
                    }
                }
                float steepness = LOUD_USL[range * 8 + group];
                float n2 = fmaxf(LOUD_RNS[range], c);
                float z2 = z1 + (n1 - n2) / steepness;
                if (z2 > upper) {
                    z2 = upper;
                    n2 = n1 - (z2 - z1) * steepness;
                }
                total += (z2 - z1) * (n1 + n2) / 2.0f;
                z1 = z2;
                n1 = n2;
            }
        }
    }
    return fmaxf(total, 0.0f);
}

// A mixture bin's place on its curve: the curve's step below its level and the fraction above it.
__device__ __forceinline__ int loud_curve_step(const float* bin, int p, float* fraction) {
    float level = 10.0f * log10f(bin[1] / bin[0]) + LOUD_PENALTY_DB[p];
    float position = clampf(level - LOUD_CURVE_LOW_DB, 0.0f, (float)LOUD_CURVE_STEPS);
    int below = min((int)floorf(position), LOUD_CURVE_STEPS - 1);
    *fraction = position - (float)below;
    return below;
}

// The loudness of each pixel per layer and in total (Nden, sone; 0 for a layer without a spectrum
// or energy) from its mixtures: a block of 32 threads a pixel, a period at a time.
extern "C" __global__ void pixel_ndens(const float* input, const u64* offsets, const float* mixtures, u32 pixel_count,
                                       float* ndens) {
    u32 pixel = blockIdx.x;
    if (pixel >= pixel_count) return;
    const float* in = input + offsets[pixel];
    __shared__ float curve[LOUD_CURVES][LOUD_CURVE_STEPS + 1];
    __shared__ unsigned char needed[LOUD_CURVES][LOUD_CURVE_STEPS + 1];
    __shared__ float nden[LOUD_CURVES];
    __shared__ int curves_at;
    int t = threadIdx.x;
    if (t == 0) {
        int start[LOUD_LAYERS], count[LOUD_LAYERS];
        loud_layers(in, start, count);
        curves_at = start[LOUD_LAYERS - 1] + count[LOUD_LAYERS - 1] * LOUD_LINE_FLOATS;
    }
    if (t < LOUD_CURVES) nden[t] = 0.0f;
    __syncthreads();
    for (int p = 0; p < PERIODS; p++) {
        const float* mixture = mixtures + (u64)pixel * LOUD_MIXTURE_FLOATS + (u64)p * LOUD_CURVES * LOUD_BINS * 2;
        for (int k = t; k < LOUD_CURVES * (LOUD_CURVE_STEPS + 1); k += blockDim.x) {
            needed[k / (LOUD_CURVE_STEPS + 1)][k % (LOUD_CURVE_STEPS + 1)] = 0;
        }
        __syncthreads();
        // The whole decibels each curve's levels need (loudness.rs Curve::at), then their loudness.
        for (int k = t; k < LOUD_CURVES * LOUD_BINS; k += blockDim.x) {
            int c = k / LOUD_BINS;
            const float* bin = mixture + 2 * k;
            if (bin[0] <= 0.0f || isnan(in[curves_at + (c * PERIODS + p) * LOUD_THIRDS])) continue;
            float fraction;
            int below = loud_curve_step(bin, p, &fraction);
            needed[c][below] = 1;
            needed[c][below + 1] = 1;
        }
        __syncthreads();
        for (int k = t; k < LOUD_CURVES * (LOUD_CURVE_STEPS + 1); k += blockDim.x) {
            int c = k / (LOUD_CURVE_STEPS + 1), step = k % (LOUD_CURVE_STEPS + 1);
            if (!needed[c][step]) continue;
            const float* shape = in + curves_at + (c * PERIODS + p) * LOUD_THIRDS;
            float levels[LOUD_THIRDS];
            for (int n = 0; n < LOUD_THIRDS; n++) levels[n] = shape[n] + LOUD_CURVE_LOW_DB + (float)step;
            curve[c][step] = loud_zwicker(levels);
        }
        __syncthreads();
        if (t < LOUD_CURVES && !isnan(in[curves_at + (t * PERIODS + p) * LOUD_THIRDS])) {
            float mean = 0.0f;
            for (int b = 0; b < LOUD_BINS; b++) {
                const float* bin = mixture + 2 * (t * LOUD_BINS + b);
                if (bin[0] <= 0.0f) continue;
                float fraction;
                int below = loud_curve_step(bin, p, &fraction);
                mean += bin[0] * (curve[t][below] + (curve[t][below + 1] - curve[t][below]) * fraction);
            }
            nden[t] += LOUD_PERIOD_HOURS[p] * mean / 24.0f;
        }
        __syncthreads();
    }
    if (t < LOUD_CURVES) ndens[(u64)pixel * LOUD_CURVES + t] = nden[t];
}

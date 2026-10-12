// A painted pixel's loudness on the card in f32, at the map's resolution: popup/src/distribution.rs
// (the level's distribution on a grid of whole decibels, a line's values added in runs that share a
// bin), popup/src/percentiles.rs (each layer's lines in each group of hours and weather state over
// its steady energy, the place's the layers' added), popup/src/loudness.rs (Nden on a layer's and
// the place's curves) and physics/src/loudness.rs (ISO 532-1). The CPU (f64) is the reference;
// `qm-paint-gpu check-loudness` measures this port against it. The host writes the LOUD_* tables
// (paint-gpu/src/loudness.rs) from the Rust ones.

// A pixel's input (floats, the host's `pack`): per layer its steady energy per period and its line
// count, then each line (LOUD_LINE_FLOATS: energy per period, its mean in each merged weather state
// per period, lambda per period, hourly profile (-1 none), duty per period (NaN none)); then per
// curve (the layers' and the place's) and period the spectrum's third-octave levels at 0 dB(A)
// (the first NaN: no spectrum).
#define LOUD_LINE_FLOATS (PERIODS + PERIODS * LOUD_STATES + PERIODS + 1 + PERIODS)
#define LOUD_CURVES (LOUD_LAYERS + 1)
#define LOUD_LAST (LOUD_BINS - 1)
#define LOUD_THREADS 32
// A line's values: a Kurze line's nodes, or a distribution's bins and its silence.
#define LOUD_VALUES_MAX (LOUD_NODES > LOUD_BINS + 1 ? LOUD_NODES : LOUD_BINS + 1)

// distribution.rs Distribution: per bin its share of the time and that share times its moments'
// mean intensity, and the share in silence.
struct LoudDistribution {
    float silent;
    float bins[LOUD_BINS][2];
};

__device__ void loud_clear(LoudDistribution& d) {
    d.silent = 0.0f;
    for (int b = 0; b < LOUD_BINS; b++) d.bins[b][0] = d.bins[b][1] = 0.0f;
}

// Distribution::bin: the nearest grid level, the lowest bin holding what lies under the grid.
__device__ __forceinline__ int loud_bin(float level_db) {
    float position = rintf((level_db - LOUD_LEVEL_MIN_DB) / LOUD_BIN_DB);
    return position <= 0.0f ? 0 : (position >= (float)LOUD_LAST ? LOUD_LAST : (int)position);
}

// A line's values: intensities relative to a scale, ascending, each with its share of the time; and
// from each value up the sums of the shares and of the shares times the values (from the top, so a
// rare loud pass keeps its share in f32, where distribution.rs LineValues sums from the bottom in
// f64).
struct LoudValues {
    int count;
    float value[LOUD_VALUES_MAX];
    float shares_from[LOUD_VALUES_MAX + 1];
    float weighted_from[LOUD_VALUES_MAX + 1];
};

__device__ void loud_sums_from_top(LoudValues& v, const float* shares) {
    v.shares_from[v.count] = 0.0f;
    v.weighted_from[v.count] = 0.0f;
    for (int k = v.count - 1; k >= 0; k--) {
        v.shares_from[k] = v.shares_from[k + 1] + shares[k];
        v.weighted_from[k] = v.weighted_from[k + 1] + shares[k] * v.value[k];
    }
}

// distribution.rs first_at_or_above: the first value from `from` reaching `limit`, galloping.
__device__ __forceinline__ int loud_first_at_or_above(const LoudValues& v, int from, float limit) {
    int low = from, step = 1;
    while (low + step - 1 < v.count && v.value[low + step - 1] < limit) {
        low += step;
        step *= 2;
    }
    int high = min(low + step - 1, v.count);
    while (low < high) {
        int middle = (low + high) / 2;
        if (v.value[middle] < limit) low = middle + 1;
        else high = middle;
    }
    return low;
}

// Distribution::runs into `out`: a moment at `intensity` (0 silent) with share `share` and each of
// `v`'s values times `scale` (above 0) added, each bin's run of values summed at once.
__device__ void loud_add_runs(LoudDistribution& out, float intensity, float share, const LoudValues& v, float scale) {
    int from = 0;
    if (intensity <= 0.0f) {
        while (from < v.count && v.value[from] <= 0.0f) from++;
        if (from > 0) out.silent += share * (v.shares_from[0] - v.shares_from[from]);
    }
    if (from == v.count) return;
    const float step = energy_of_level(LOUD_BIN_DB);
    int bin = loud_bin(10.0f * log10f(intensity + scale * v.value[from]));
    float ceiling = energy_of_level(LOUD_LEVEL_MIN_DB + ((float)bin + 0.5f) * LOUD_BIN_DB);
    while (true) {
        int to = bin == LOUD_LAST ? v.count : loud_first_at_or_above(v, from + 1, (ceiling - intensity) / scale);
        float shares = v.shares_from[from] - v.shares_from[to];
        float weighted = scale * (v.weighted_from[from] - v.weighted_from[to]);
        out.bins[bin][0] += share * shares;
        out.bins[bin][1] += share * (intensity * shares + weighted);
        from = to;
        if (from >= v.count) break;
        float value = v.value[from];
        do {
            bin++;
            ceiling *= step;
        } while (!(bin == LOUD_LAST || intensity + scale * value < ceiling));
    }
}

// Distribution::with_line: `d` with an independent line (`v` times `scale`) added at every moment.
__device__ void loud_with_line(const LoudDistribution& d, const LoudValues& v, float scale, LoudDistribution& out) {
    loud_clear(out);
    if (d.silent > 0.0f) loud_add_runs(out, 0.0f, d.silent, v, scale);
    for (int b = 0; b < LOUD_BINS; b++) {
        float share = d.bins[b][0];
        if (share > 0.0f) loud_add_runs(out, d.bins[b][1] / share, share, v, scale);
    }
}

// Distribution::over on silence: every moment at `intensity` (silence at none).
__device__ void loud_steady(float intensity, LoudDistribution& out) {
    loud_clear(out);
    if (intensity > 0.0f) {
        int bin = loud_bin(10.0f * log10f(intensity));
        out.bins[bin][0] = 1.0f;
        out.bins[bin][1] = intensity;
    } else {
        out.silent = 1.0f;
    }
}

// Distribution::with_distribution: `d` with an independent `other` added, its levels (and its
// silence, a value of nothing) as a line's values.
__device__ void loud_with_distribution(const LoudDistribution& d, const LoudDistribution& other, LoudValues& scratch,
                                       LoudDistribution& out) {
    float shares[LOUD_BINS + 1];
    scratch.count = 0;
    if (other.silent > 0.0f) {
        scratch.value[0] = 0.0f;
        shares[0] = other.silent;
        scratch.count = 1;
    }
    for (int b = 0; b < LOUD_BINS; b++) {
        if (other.bins[b][0] > 0.0f) {
            scratch.value[scratch.count] = other.bins[b][1] / other.bins[b][0];
            shares[scratch.count] = other.bins[b][0];
            scratch.count++;
        }
    }
    loud_sums_from_top(scratch, shares);
    loud_with_line(d, scratch, 1.0f, out);
}

// physics/src/percentile.rs AtProbabilities::relative_intensities for node `k` (the probabilities
// of distribution.rs nodes, then the table's last quantile's and the held test's), the parts that
// hang on lambda alone (`at_top`, `passing_top`) given.
__device__ float loud_relative_intensity(float lambda, int k, float at_top, float passing_top) {
    if (isinf(lambda)) return 1.0f;
    if (lambda <= 0.0f) return 0.0f;
    if (lambda >= LOUD_SIMULATED_LAMBDA_MAX) {
        float deviation = 1.0f / sqrtf(2.0f * QM_PI * lambda);
        return fmaxf(1.0f + deviation * LOUD_NORMAL[k], 0.0f);
    }
    if (LOUD_TAIL[k]) {
        float s = -LOUD_LN_P[k] / (2.0f * lambda);
        return at_top + 1.0f / (lambda * QM_PI * (1.0f + s * s)) - passing_top;
    }
    const float* rows = &LOUD_ROWS[k * LOUD_ROWS_COUNT];
    if (lambda < LOUD_LAMBDA_MIN) return lambda / LOUD_LAMBDA_MIN * expf(rows[0]);
    float position = clampf(log10f(lambda / LOUD_LAMBDA_MIN) * LOUD_STEPS_PER_DECADE, 0.0f, (float)(LOUD_ROWS_COUNT - 1));
    int row = (int)floorf(position);
    float fraction = position - (float)row;
    float low = rows[row], high = rows[min(row + 1, LOUD_ROWS_COUNT - 1)];
    return expf(low + fraction * (high - low));
}

// The parts of a sparse line's intensity above the table that hang on lambda alone: its intensity
// at the table's last quantile and that quantile's passing term.
__device__ void loud_top(float lambda, float* at_top, float* passing_top) {
    *at_top = 0.0f;
    *passing_top = 0.0f;
    if (isinf(lambda) || lambda <= 0.0f || lambda >= LOUD_SIMULATED_LAMBDA_MAX) return;
    *at_top = loud_relative_intensity(lambda, LOUD_TOP_NODE, 0.0f, 0.0f);
    float s = -LOUD_LN_P[LOUD_TOP_NODE] / (2.0f * lambda);
    *passing_top = 1.0f / (lambda * QM_PI * (1.0f + s * s));
}

// percentiles.rs Line::shape's values: Kurze's quantiles at `lambda` relative to the line's mean.
__device__ void loud_kurze(float lambda, LoudValues& v) {
    float at_top, passing_top;
    loud_top(lambda, &at_top, &passing_top);
    v.count = LOUD_NODES;
    for (int k = 0; k < LOUD_NODES; k++) v.value[k] = loud_relative_intensity(lambda, k, at_top, passing_top);
    loud_sums_from_top(v, LOUD_SHARES);
}

// percentiles.rs HELD_EXCEEDED: the intensity a line of `lambda` exceeds a thousandth of the time,
// relative to its mean.
__device__ float loud_exceeded(float lambda) {
    float at_top, passing_top;
    loud_top(lambda, &at_top, &passing_top);
    return loud_relative_intensity(lambda, LOUD_HELD_NODE, at_top, passing_top);
}

// An event's values relative to its mean: silent off its duty, its mean over the duty on it.
__device__ void loud_event(float duty, LoudValues& v) {
    float shares[2] = {1.0f - duty, duty};
    v.count = 2;
    v.value[0] = 0.0f;
    v.value[1] = 1.0f / duty;
    loud_sums_from_top(v, shares);
}

// One line of a packed layer.
struct LoudLine {
    const float* f;
    __device__ float energy(int p) const { return f[p]; }
    __device__ float mean(int p, int state) const { return f[PERIODS + p * LOUD_STATES + state]; }
    __device__ float lambda(int p) const { return f[PERIODS + PERIODS * LOUD_STATES + p]; }
    __device__ int profile() const { return (int)f[2 * PERIODS + PERIODS * LOUD_STATES]; }
    __device__ float duty(int p) const { return f[2 * PERIODS + PERIODS * LOUD_STATES + 1 + p]; }
    // The line's flow in group `group` of period `p` against the period's mean (percentiles.rs
    // hour_factor over the group's hours).
    __device__ float factor(int p, int group) const {
        int profile = this->profile();
        return profile >= 0 ? LOUD_FACTOR[(profile * PERIODS + p) * LOUD_GROUPS_MAX + group] : 1.0f;
    }
};

// percentiles.rs moment: one layer's energy that holds steady in group `group` of period `p` and
// weather state `state` (its `steady` energy, the lines that hold their mean, those whose loud
// moments stay under a hundredth of it), and whether a line moves; `held` is the energy the held
// test compares against.
__device__ float loud_floor(const float* lines, int count, float steady, int p, int group, int state, float* held,
                            bool* moves) {
    float floor = steady, candidates = 0.0f;
    *moves = false;
    for (int i = 0; i < count; i++) {
        LoudLine line{lines + i * LOUD_LINE_FLOATS};
        float factor = line.factor(p, group);
        float mean = line.mean(p, state) * factor;
        if (line.energy(p) <= 0.0f || mean <= 0.0f) continue;
        float duty = line.duty(p);
        if (!isnan(duty)) {
            *moves = *moves || duty > 0.0f;
            continue;
        }
        if (line.lambda(p) * factor >= LOUD_HOLDS_MEAN_LAMBDA) floor += mean;
        else candidates += mean;
    }
    *held = floor + candidates;
    for (int i = 0; i < count; i++) {
        LoudLine line{lines + i * LOUD_LINE_FLOATS};
        float factor = line.factor(p, group);
        float mean = line.mean(p, state) * factor;
        float lambda = line.lambda(p) * factor;
        if (line.energy(p) <= 0.0f || mean <= 0.0f || !isnan(line.duty(p)) || lambda >= LOUD_HOLDS_MEAN_LAMBDA) continue;
        if (mean * loud_exceeded(lambda) < LOUD_HELD_SHARE * *held) floor += mean;
        else *moves = true;
    }
    return floor;
}

// percentiles.rs layer_states for one state: the layer's steady energy, then its moving lines
// added in moment()'s order (the events, then the others); the result in `*a` or `*b`, returned.
__device__ LoudDistribution* loud_layer(const float* lines, int count, float steady, int p, int group, int state,
                                        LoudValues& values, LoudDistribution* a, LoudDistribution* b) {
    float held;
    bool moves;
    float floor = loud_floor(lines, count, steady, p, group, state, &held, &moves);
    loud_steady(floor, *a);
    LoudDistribution *now = a, *next = b;
    for (int pass = 0; pass < 2 && moves; pass++) {
        for (int i = 0; i < count; i++) {
            LoudLine line{lines + i * LOUD_LINE_FLOATS};
            float factor = line.factor(p, group);
            float mean = line.mean(p, state) * factor;
            if (line.energy(p) <= 0.0f || mean <= 0.0f) continue;
            float duty = line.duty(p), lambda = line.lambda(p) * factor;
            if (pass == 0) {
                if (isnan(duty) || duty <= 0.0f) continue;
                loud_event(duty, values);
            } else {
                if (!isnan(duty) || lambda >= LOUD_HOLDS_MEAN_LAMBDA) continue;
                if (mean * loud_exceeded(lambda) < LOUD_HELD_SHARE * held) continue;
                loud_kurze(lambda, values);
            }
            loud_with_line(*now, values, mean, *next);
            LoudDistribution* swap = now;
            now = next;
            next = swap;
        }
    }
    return now;
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

// The loudness of each pixel per layer and in total (Nden, sone; 0 for a layer without a spectrum
// or energy): a block a pixel, its threads the groups of hours and weather states of one period at
// a time; each state's layers into the period's mixtures, the place's their sum from the steady
// layers' energy (popup/src/percentiles.rs layered_distributions); then each mixture on its curve
// (popup/src/loudness.rs loudness), ISO 532-1 at the whole decibels the levels need.
extern "C" __global__ void pixel_loudness(const float* input, const u64* offsets, u32 pixel_count, float* ndens) {
    u32 pixel = blockIdx.x;
    if (pixel >= pixel_count) return;
    const float* in = input + offsets[pixel];
    __shared__ float mixture[LOUD_CURVES][LOUD_BINS][2];
    __shared__ float silent[LOUD_CURVES];
    __shared__ float curve[LOUD_CURVES][LOUD_CURVE_STEPS + 1];
    __shared__ unsigned char needed[LOUD_CURVES][LOUD_CURVE_STEPS + 1];
    __shared__ float nden[LOUD_CURVES];
    __shared__ int layer_start[LOUD_LAYERS], layer_count[LOUD_LAYERS];
    __shared__ bool profiled;
    int t = threadIdx.x;
    if (t == 0) {
        int at = 0;
        profiled = false;
        for (int l = 0; l < LOUD_LAYERS; l++) {
            layer_count[l] = (int)in[at + PERIODS];
            layer_start[l] = at + PERIODS + 1;
            for (int i = 0; i < layer_count[l]; i++) {
                LoudLine line{in + layer_start[l] + i * LOUD_LINE_FLOATS};
                profiled = profiled || line.profile() >= 0;
            }
            at = layer_start[l] + layer_count[l] * LOUD_LINE_FLOATS;
        }
        for (int c = 0; c < LOUD_CURVES; c++) nden[c] = 0.0f;
    }
    __syncthreads();
    // The curves' spectra after the layers.
    int curves_at = layer_start[LOUD_LAYERS - 1] + layer_count[LOUD_LAYERS - 1] * LOUD_LINE_FLOATS;
    LoudValues values;
    LoudDistribution layer_a, layer_b, whole_a, whole_b;
    for (int p = 0; p < PERIODS; p++) {
        int groups = profiled ? LOUD_GROUPS[p] : 1;
        float hours = profiled ? LOUD_PERIOD_HOURS[p] : 1.0f;
        for (int k = t; k < LOUD_CURVES * LOUD_BINS; k += LOUD_THREADS) {
            mixture[k / LOUD_BINS][k % LOUD_BINS][0] = 0.0f;
            mixture[k / LOUD_BINS][k % LOUD_BINS][1] = 0.0f;
        }
        for (int k = t; k < LOUD_CURVES * (LOUD_CURVE_STEPS + 1); k += LOUD_THREADS) {
            needed[k / (LOUD_CURVE_STEPS + 1)][k % (LOUD_CURVE_STEPS + 1)] = 0;
        }
        if (t < LOUD_CURVES) silent[t] = 0.0f;
        __syncthreads();
        if (t < groups * LOUD_STATES) {
            int group = t / LOUD_STATES, state = t % LOUD_STATES;
            int slots = profiled ? LOUD_GROUP_SLOTS[p * LOUD_GROUPS_MAX + group] : 1;
            float weight = (float)slots / (hours * (float)LOUD_STATES);
            // What holds steady in every layer at once, then the layers that move.
            float floor = 0.0f;
            for (int l = 0; l < LOUD_LAYERS; l++) {
                float held;
                bool moves;
                float own = loud_floor(in + layer_start[l], layer_count[l], in[layer_start[l] - PERIODS - 1 + p], p,
                                       profiled ? group : 0, state, &held, &moves);
                if (!moves) floor += own;
            }
            loud_steady(floor, whole_a);
            LoudDistribution *whole = &whole_a, *whole_next = &whole_b;
            for (int l = 0; l < LOUD_LAYERS; l++) {
                float held;
                bool moves;
                float steady = in[layer_start[l] - PERIODS - 1 + p];
                loud_floor(in + layer_start[l], layer_count[l], steady, p, profiled ? group : 0, state, &held, &moves);
                LoudDistribution* own = loud_layer(in + layer_start[l], layer_count[l], steady, p, profiled ? group : 0,
                                                   state, values, &layer_a, &layer_b);
                atomicAdd(&silent[l], weight * own->silent);
                for (int b = 0; b < LOUD_BINS; b++) {
                    if (own->bins[b][0] > 0.0f) {
                        atomicAdd(&mixture[l][b][0], weight * own->bins[b][0]);
                        atomicAdd(&mixture[l][b][1], weight * own->bins[b][1]);
                    }
                }
                if (moves) {
                    loud_with_distribution(*whole, *own, values, *whole_next);
                    LoudDistribution* swap = whole;
                    whole = whole_next;
                    whole_next = swap;
                }
            }
            atomicAdd(&silent[LOUD_LAYERS], weight * whole->silent);
            for (int b = 0; b < LOUD_BINS; b++) {
                if (whole->bins[b][0] > 0.0f) {
                    atomicAdd(&mixture[LOUD_LAYERS][b][0], weight * whole->bins[b][0]);
                    atomicAdd(&mixture[LOUD_LAYERS][b][1], weight * whole->bins[b][1]);
                }
            }
        }
        __syncthreads();
        // The whole decibels each curve's levels need (loudness.rs Curve::at), then their loudness.
        for (int k = t; k < LOUD_CURVES * LOUD_BINS; k += LOUD_THREADS) {
            int c = k / LOUD_BINS, b = k % LOUD_BINS;
            float share = mixture[c][b][0];
            if (share <= 0.0f || isnan(in[curves_at + (c * PERIODS + p) * LOUD_THIRDS])) continue;
            float level = 10.0f * log10f(mixture[c][b][1] / share) + LOUD_PENALTY_DB[p];
            float position = clampf(level - LOUD_CURVE_LOW_DB, 0.0f, (float)LOUD_CURVE_STEPS);
            int below = min((int)floorf(position), LOUD_CURVE_STEPS - 1);
            needed[c][below] = 1;
            needed[c][below + 1] = 1;
        }
        __syncthreads();
        for (int k = t; k < LOUD_CURVES * (LOUD_CURVE_STEPS + 1); k += LOUD_THREADS) {
            int c = k / (LOUD_CURVE_STEPS + 1), step = k % (LOUD_CURVE_STEPS + 1);
            if (!needed[c][step]) continue;
            const float* shape = in + curves_at + (c * PERIODS + p) * LOUD_THIRDS;
            float levels[LOUD_THIRDS];
            for (int n = 0; n < LOUD_THIRDS; n++) levels[n] = shape[n] + LOUD_CURVE_LOW_DB + (float)step;
            curve[c][step] = loud_zwicker(levels);
        }
        __syncthreads();
        if (t < LOUD_CURVES) {
            int c = t;
            float mean = 0.0f;
            if (!isnan(in[curves_at + (c * PERIODS + p) * LOUD_THIRDS])) {
                for (int b = 0; b < LOUD_BINS; b++) {
                    float share = mixture[c][b][0];
                    if (share <= 0.0f) continue;
                    float level = 10.0f * log10f(mixture[c][b][1] / share) + LOUD_PENALTY_DB[p];
                    float position = clampf(level - LOUD_CURVE_LOW_DB, 0.0f, (float)LOUD_CURVE_STEPS);
                    int below = min((int)floorf(position), LOUD_CURVE_STEPS - 1);
                    float low = curve[c][below], high = curve[c][below + 1];
                    mean += share * (low + (high - low) * (position - (float)below));
                }
            }
            nden[c] += LOUD_PERIOD_HOURS[p] * mean / 24.0f;
        }
        __syncthreads();
    }
    if (t < LOUD_CURVES) ndens[(u64)pixel * LOUD_CURVES + t] = nden[t];
}

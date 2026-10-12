// A pixel's level distributions on the card in f32, at the map's resolution, a warp a task:
// popup/src/distribution.rs (the level's distribution on a grid of whole decibels, a line's values
// added in runs that share a bin) and popup/src/percentiles.rs (a layer's lines in one group of hours
// and weather state over its steady energy). The warp's 32 lanes share each step (a line's values,
// the source bins of a sum) and its distributions live in shared memory. The CPU (f64) is the
// reference; `qm-paint-gpu check-loudness` measures the port. The host writes the LOUD_* tables
// (paint-gpu/src/loudness.rs) from the Rust ones.

// A pixel's input (floats, the host's `pack`): per layer its steady energy per period and its line
// count, then each line (LOUD_LINE_FLOATS: energy per period, its mean in each merged weather state
// per period, lambda per period, hourly profile (-1 none), duty per period (NaN none)); then per
// curve (the layers' and the place's) and period the spectrum's third-octave levels at 0 dB(A)
// (the first NaN: no spectrum).
#define LOUD_LINE_FLOATS (PERIODS + PERIODS * LOUD_STATES + PERIODS + 1 + PERIODS)
#define LOUD_CURVES (LOUD_LAYERS + 1)
#define LOUD_LAST (LOUD_BINS - 1)
// A line's values: a Kurze line's nodes, or a distribution's bins and its silence.
#define LOUD_VALUES_MAX (LOUD_NODES > LOUD_BINS + 1 ? LOUD_NODES : LOUD_BINS + 1)
#define LOUD_LANES 32
#define LOUD_FULL_MASK 0xffffffffu
// The most values one lane sums.
#define LOUD_CHUNK ((LOUD_VALUES_MAX + LOUD_LANES - 1) / LOUD_LANES)

// distribution.rs Distribution: per bin its share of the time and that share times its moments'
// mean intensity, and the share in silence. Only the bins from `low` to `high` are read (none when
// low > high).
struct LoudDistribution {
    float silent;
    int low, high;
    float bins[LOUD_BINS][2];
};

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

// One warp's working memory: a layer's distribution and the place's, each in two buffers, and a
// line's values.
struct LoudWarp {
    LoudDistribution layer[2], whole[2];
    LoudValues values;
};

// Distribution::bin: the nearest grid level, the lowest bin holding what lies under the grid.
__device__ __forceinline__ int loud_bin(float level_db) {
    float position = rintf((level_db - LOUD_LEVEL_MIN_DB) / LOUD_BIN_DB);
    return position <= 0.0f ? 0 : (position >= (float)LOUD_LAST ? LOUD_LAST : (int)position);
}

// The intensity at a bin's upper edge.
__device__ __forceinline__ float loud_ceiling(int bin) {
    return energy_of_level(LOUD_LEVEL_MIN_DB + ((float)bin + 0.5f) * LOUD_BIN_DB);
}

// `v`'s sums from the top of its values' `shares` (`v.shares_from` itself may hold them): each lane
// sums a chunk from its top, then adds the chunks above it.
__device__ void loud_sums_from_top(LoudValues& v, const float* shares, int lane) {
    int count = v.count, chunk = (count + LOUD_LANES - 1) / LOUD_LANES;
    int first = min(lane * chunk, count), end = min(first + chunk, count);
    float own_shares[LOUD_CHUNK], own_weighted[LOUD_CHUNK];
    float total_shares = 0.0f, total_weighted = 0.0f;
    for (int k = end - 1; k >= first; k--) {
        total_shares += shares[k];
        total_weighted += shares[k] * v.value[k];
        own_shares[k - first] = total_shares;
        own_weighted[k - first] = total_weighted;
    }
    // Each lane's total with every lane's above it, then the lanes above alone.
    float with_above = total_shares, weighted_with_above = total_weighted;
    for (int offset = 1; offset < LOUD_LANES; offset <<= 1) {
        float s = __shfl_down_sync(LOUD_FULL_MASK, with_above, offset);
        float w = __shfl_down_sync(LOUD_FULL_MASK, weighted_with_above, offset);
        if (lane + offset < LOUD_LANES) {
            with_above += s;
            weighted_with_above += w;
        }
    }
    float above = __shfl_down_sync(LOUD_FULL_MASK, with_above, 1);
    float weighted_above = __shfl_down_sync(LOUD_FULL_MASK, weighted_with_above, 1);
    if (lane == LOUD_LANES - 1) above = weighted_above = 0.0f;
    __syncwarp();
    for (int k = first; k < end; k++) {
        v.shares_from[k] = own_shares[k - first] + above;
        v.weighted_from[k] = own_weighted[k - first] + weighted_above;
    }
    if (lane == 0) v.shares_from[count] = v.weighted_from[count] = 0.0f;
    __syncwarp();
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

// Distribution::runs into `out` (one lane, adding atomically): a moment at `intensity` (0 silent)
// with share `share` and each of `v`'s values times `scale` (above 0) added, each bin's run of values
// summed at once.
__device__ void loud_add_runs(LoudDistribution& out, float intensity, float share, const LoudValues& v, float scale) {
    int from = 0;
    if (intensity <= 0.0f) {
        while (from < v.count && v.value[from] <= 0.0f) from++;
        if (from > 0) atomicAdd(&out.silent, share * (v.shares_from[0] - v.shares_from[from]));
    }
    if (from == v.count) return;
    const float step = energy_of_level(LOUD_BIN_DB);
    int bin = loud_bin(10.0f * log10f(intensity + scale * v.value[from]));
    float ceiling = loud_ceiling(bin);
    while (true) {
        int to = bin == LOUD_LAST ? v.count : loud_first_at_or_above(v, from + 1, (ceiling - intensity) / scale);
        float shares = v.shares_from[from] - v.shares_from[to];
        float weighted = scale * (v.weighted_from[from] - v.weighted_from[to]);
        atomicAdd(&out.bins[bin][0], share * shares);
        atomicAdd(&out.bins[bin][1], share * (intensity * shares + weighted));
        from = to;
        if (from >= v.count) break;
        float value = v.value[from];
        do {
            bin++;
            ceiling *= step;
        } while (!(bin == LOUD_LAST || intensity + scale * value < ceiling));
    }
}

// Distribution::with_line: `d` with an independent line (`v` times `scale`) added at every moment,
// into `out`: the bins the sums can reach cleared (one more each side for the runs' rounding), then
// each lane adds its share of `d`'s bins, the first lane its silence too.
__device__ void loud_with_line(const LoudDistribution& d, const LoudValues& v, float scale, LoudDistribution& out,
                               int lane) {
    int low = LOUD_BINS, high = -1, count = v.count;
    if (count > 0) {
        float top = scale * v.value[count - 1];
        if (d.silent > 0.0f) {
            int first = 0;
            while (first < count && v.value[first] <= 0.0f) first++;
            if (first < count) {
                low = loud_bin(10.0f * log10f(scale * v.value[first]));
                high = loud_bin(10.0f * log10f(top));
            }
        }
        if (d.low <= d.high) {
            float floor = d.low == 0 ? 0.0f : loud_ceiling(d.low - 1);
            float ceiling = d.high == LOUD_LAST ? QM_INFINITY : loud_ceiling(d.high);
            low = min(low, loud_bin(10.0f * log10f(floor + scale * v.value[0])));
            high = max(high, loud_bin(10.0f * log10f(ceiling + top)));
        }
    }
    if (low <= high) {
        low = max(low - 1, 0);
        high = min(high + 1, LOUD_LAST);
    }
    for (int b = low + lane; b <= high; b += LOUD_LANES) out.bins[b][0] = out.bins[b][1] = 0.0f;
    if (lane == 0) {
        out.silent = 0.0f;
        out.low = low;
        out.high = high;
    }
    __syncwarp();
    if (lane == 0 && d.silent > 0.0f) loud_add_runs(out, 0.0f, d.silent, v, scale);
    for (int b = d.low + lane; b <= d.high; b += LOUD_LANES) {
        float share = d.bins[b][0];
        if (share > 0.0f) loud_add_runs(out, d.bins[b][1] / share, share, v, scale);
    }
    __syncwarp();
}

// Distribution::over on silence: every moment at `intensity` (silence at none).
__device__ void loud_steady(float intensity, LoudDistribution& out, int lane) {
    if (lane == 0) {
        out.silent = 0.0f;
        out.low = LOUD_BINS;
        out.high = -1;
        if (intensity > 0.0f) {
            int bin = loud_bin(10.0f * log10f(intensity));
            out.bins[bin][0] = 1.0f;
            out.bins[bin][1] = intensity;
            out.low = out.high = bin;
        } else {
            out.silent = 1.0f;
        }
    }
    __syncwarp();
}

// Distribution::with_distribution: `d` with an independent `other` added, its levels (and its
// silence, a value of nothing) as a line's values in `v`, gathered in order by the lanes.
__device__ void loud_with_distribution(const LoudDistribution& d, const LoudDistribution& other, LoudValues& v,
                                       LoudDistribution& out, int lane) {
    int count = 0;
    if (other.silent > 0.0f) {
        if (lane == 0) {
            v.value[0] = 0.0f;
            v.shares_from[0] = other.silent;
        }
        count = 1;
    }
    for (int start = other.low; start <= other.high; start += LOUD_LANES) {
        int b = start + lane;
        bool held = b <= other.high && other.bins[b][0] > 0.0f;
        unsigned ballot = __ballot_sync(LOUD_FULL_MASK, held);
        if (held) {
            int at = count + __popc(ballot & ((1u << lane) - 1u));
            v.value[at] = other.bins[b][1] / other.bins[b][0];
            v.shares_from[at] = other.bins[b][0];
        }
        count += __popc(ballot);
    }
    if (lane == 0) v.count = count;
    __syncwarp();
    loud_sums_from_top(v, v.shares_from, lane);
    loud_with_line(d, v, 1.0f, out, lane);
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

// percentiles.rs Line::shape's values: Kurze's quantiles at `lambda` relative to the line's mean,
// the lanes a node each in turn.
__device__ void loud_kurze(float lambda, LoudValues& v, int lane) {
    float at_top, passing_top;
    loud_top(lambda, &at_top, &passing_top);
    for (int k = lane; k < LOUD_NODES; k += LOUD_LANES) {
        v.value[k] = loud_relative_intensity(lambda, k, at_top, passing_top);
        v.shares_from[k] = LOUD_SHARES[k];
    }
    if (lane == 0) v.count = LOUD_NODES;
    __syncwarp();
    loud_sums_from_top(v, v.shares_from, lane);
}

// percentiles.rs HELD_EXCEEDED: the intensity a line of `lambda` exceeds a thousandth of the time,
// relative to its mean.
__device__ float loud_exceeded(float lambda) {
    float at_top, passing_top;
    loud_top(lambda, &at_top, &passing_top);
    return loud_relative_intensity(lambda, LOUD_HELD_NODE, at_top, passing_top);
}

// An event's values relative to its mean: silent off its duty, its mean over the duty on it.
__device__ void loud_event(float duty, LoudValues& v, int lane) {
    if (lane == 0) {
        v.count = 2;
        v.value[0] = 0.0f;
        v.value[1] = 1.0f / duty;
        v.shares_from[0] = 1.0f - duty;
        v.shares_from[1] = duty;
    }
    __syncwarp();
    loud_sums_from_top(v, v.shares_from, lane);
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

// Where each layer's lines begin in a pixel's input and how many; whether any line follows the
// hours (the pixel's periods then split into groups of hours).
__device__ bool loud_layers(const float* in, int* start, int* count) {
    bool profiled = false;
    int at = 0;
    for (int l = 0; l < LOUD_LAYERS; l++) {
        count[l] = (int)in[at + PERIODS];
        start[l] = at + PERIODS + 1;
        for (int i = 0; i < count[l]; i++) profiled = profiled || LoudLine{in + start[l] + i * LOUD_LINE_FLOATS}.profile() >= 0;
        at = start[l] + count[l] * LOUD_LINE_FLOATS;
    }
    return profiled;
}

// percentiles.rs moment: one layer's energy that holds steady in group `group` of period `p` and
// weather state `state` (its `steady` energy, the lines that hold their mean, those whose loud
// moments stay under a hundredth of it), and whether a line moves; `held` is the energy the held
// test compares against. Every lane alike.
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

// percentiles.rs layer_states for one state: the layer's floor (loud_floor's, with its `held` and
// `moves`), then its moving lines added in moment()'s order (the events, then the others); returns
// which of `w.layer` holds the result.
__device__ int loud_layer(const float* lines, int count, float floor, float held, bool moves, int p, int group, int state,
                          LoudWarp& w, int lane) {
    loud_steady(floor, w.layer[0], lane);
    int now = 0;
    for (int pass = 0; pass < 2 && moves; pass++) {
        for (int i = 0; i < count; i++) {
            LoudLine line{lines + i * LOUD_LINE_FLOATS};
            float factor = line.factor(p, group);
            float mean = line.mean(p, state) * factor;
            if (line.energy(p) <= 0.0f || mean <= 0.0f) continue;
            float duty = line.duty(p), lambda = line.lambda(p) * factor;
            if (pass == 0) {
                if (isnan(duty) || duty <= 0.0f) continue;
                loud_event(duty, w.values, lane);
            } else {
                if (!isnan(duty) || lambda >= LOUD_HOLDS_MEAN_LAMBDA) continue;
                if (mean * loud_exceeded(lambda) < LOUD_HELD_SHARE * held) continue;
                loud_kurze(lambda, w.values, lane);
            }
            loud_with_line(w.layer[now], w.values, mean, w.layer[1 - now], lane);
            now = 1 - now;
        }
    }
    return now;
}

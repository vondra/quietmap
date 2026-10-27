// Aircraft box energies at the painter's lattice points: popup/src/aircraft/boxes.rs tile_flight_sums
// and physics/src/doc29/boxes.rs box_sel_at_receiver in single precision, every box of a segment
// (one point's boxes of one tile, file and lattice) a thread, summed per point in double.

// The constants (NPD_DISTANCES, LOG10_NPD_DISTANCES_FT, the installation slants, the screening's
// ceiling and slope, PERIOD_SECONDS, ...) come from aircraft.rs aircraft_constants, from the Rust.

// tiles::aircraft::AircraftBox at the square's frame, with what boxes.rs derives once a box:
// the centroid in the square's frame (m), the average length (boxes.rs average_length_m), whether
// screening applies (centroid altitude above ground under the ceiling), per period its flights' Kurze
// rate per slant (boxes.rs box_lambda without the slant) and whether it is silent.
struct FlightBox {
    float east_m, north_m;
    float centroid_altitude_m;
    float sin_axis, cos_axis;
    float gradient, gradient_spread;
    float average_length_m;
    float piece_length_m[PERIODS];
    float levels_db[PERIODS][NPD_DISTANCES];
    float tail_levels_db[PERIODS];
    float lg_scaled_distance[NPD_DISTANCES];
    float installation_shares[PERIODS][2][3];
    float lambda_per_slant[PERIODS];
    u32 screened;
    u32 helicopter;
    u32 silent[PERIODS];
};

// One lattice point: where it stands in the square's frame, its altitude, the NPD levels' impedance
// gain there (fixed wing), and its horizons (popup/src/aircraft/horizons.rs Horizons::write_f32).
struct FlightPoint {
    float x, y;
    float altitude_m;
    float impedance_gain;
    u64 horizons;
};

// One point's boxes of one tile, file and band: `count` boxes from `first`, heard within
// `within_m`, each weighted by `weight` (FLIGHT_WEIGHT_*) at its distance, into sums `sum`.
struct FlightSegment {
    u32 point;
    u32 sum;
    u32 first;
    u32 count;
    u32 weight;
    float within_m;
    float coarse_m;
};

#define FLIGHT_WEIGHT_ONE 0u
#define FLIGHT_WEIGHT_FAR 1u
#define FLIGHT_WEIGHT_NEAR 2u

// paint/src/flight_boxes.rs FlightWeight::at: the smoothstep from 1.5 to 3 times the hum's
// spacing.
__device__ float flight_weight(u32 kind, float distance_m, float coarse_m) {
    if (kind == FLIGHT_WEIGHT_ONE) return 1.0f;
    float fraction = clampf((distance_m - 1.5f * coarse_m) / (1.5f * coarse_m), 0.0f, 1.0f);
    float far = fraction * fraction * (3.0f - 2.0f * fraction);
    return kind == FLIGHT_WEIGHT_FAR ? far : 1.0f - far;
}

// physics::doc29::screening::path_difference_loss_db
__device__ float path_difference_loss_db(float path_difference_m) {
    float raw = 10.0f * log10f(3.0f + PATH_DIFFERENCE_SLOPE_PER_M * fmaxf(path_difference_m, 0.0f));
    return clampf(raw - GRAZING_LOSS_DB, 0.0f, SCREENING_CAP_DB);
}

// physics::doc29::screening::edge_loss_db; the path difference without cancellation
// (common.cuh triangle_excess: the edge lies ahead of the receiver toward the source).
__device__ float edge_loss_db(float edge_range_m, float edge_height_m, float source_range_m, float source_height_m) {
    bool blocks = edge_range_m < source_range_m && edge_height_m * source_range_m > source_height_m * edge_range_m;
    if (!blocks) return 0.0f;
    PlanePoint receiver{0.0f, 0.0f}, edge{edge_range_m, edge_height_m}, source{source_range_m, source_height_m};
    return path_difference_loss_db(triangle_excess(receiver, edge, source));
}

// popup::aircraft::horizons::SectorEdges::loss_db over `sectors` sectors stored from `at`: the
// steepest tangent of all, then per sector its steepest and six bands' (range, tangent), NaN range
// for an empty band.
__device__ float horizon_loss_db(const float* at, int sectors, float x, float y, float z) {
    float range_m = hypotf(x, y);
    if (range_m <= 1.0f || z >= at[0] * range_m) return 0.0f;
    float angle = rem_euclidf(atan2f(y, x), QM_TAU);
    int sector = min((int)(angle / QM_TAU * (float)sectors), sectors - 1);
    const float* edges = at + 1 + sector * (1 + 2 * HORIZON_BANDS);
    if (z >= edges[0] * range_m) return 0.0f;
    float loss = 0.0f;
    for (int band = 0; band < HORIZON_BANDS; band++) {
        float edge_range = edges[1 + 2 * band], tangent = edges[2 + 2 * band];
        if (isnan(edge_range)) continue;
        loss = fmaxf(loss, edge_loss_db(edge_range, tangent * edge_range, range_m, z));
    }
    return loss;
}

// physics::doc29::npd::NpdPosition::at and linear
struct NpdPosition {
    int interval;
    float fraction;
    float slant_m;
};

__device__ NpdPosition npd_position(float slant_m) {
    NpdPosition position;
    position.slant_m = fmaxf(slant_m, NPD_NEAREST_SLANT_M);
    float log_d = log10f(position.slant_m / METRES_PER_FOOT);
    int interval = 0;
    for (int k = NPD_DISTANCES - 2; k >= 1; k--) {
        if (log_d > LOG10_NPD_DISTANCES_FT[k]) {
            interval = k;
            break;
        }
    }
    position.interval = interval;
    position.fraction = (log_d - LOG10_NPD_DISTANCES_FT[interval]) / (LOG10_NPD_DISTANCES_FT[interval + 1] - LOG10_NPD_DISTANCES_FT[interval]);
    return position;
}

__device__ __forceinline__ float npd_linear(const NpdPosition& position, const float* curve) {
    float lower = curve[position.interval];
    return lower + position.fraction * (curve[position.interval + 1] - lower);
}

// physics::doc29::boxes::box_level_db
__device__ float box_level_db(const float* levels, float tail_db, const NpdPosition& position) {
    float last = levels[NPD_DISTANCES - 1], slant_m = position.slant_m;
    if (slant_m < NPD_LAST_DISTANCE_M) return npd_linear(position, levels);
    if (isinf(last) && last < 0.0f) return -QM_INFINITY;
    float tail_spherical = 20.0f * log10f(TAIL_ANCHOR_M / NPD_LAST_DISTANCE_M);
    float absorption_db_per_m = fmaxf((last - tail_spherical - tail_db) / (TAIL_ANCHOR_M - NPD_LAST_DISTANCE_M), 0.0f);
    return last - 20.0f * log10f(slant_m / NPD_LAST_DISTANCE_M) - absorption_db_per_m * (slant_m - NPD_LAST_DISTANCE_M);
}

// physics::doc29::corrections::finite_segment_energy_fraction. The arctangents' difference is
// taken as one arctangent while both ends lie on one side: two single-precision values near pi/2
// (a far segment pointing at the receiver) would cancel to nothing.
__device__ float finite_segment_energy_fraction(float along_m, float length_m, float scaled_distance_m) {
    if (length_m < 1.0f || scaled_distance_m < 1.0f) return 1.0f;
    float alpha_start = -along_m / scaled_distance_m;
    float alpha_end = -(along_m - length_m) / scaled_distance_m;
    float product = alpha_start * alpha_end;
    float angle = product > -1.0f ? atanf((alpha_end - alpha_start) / (1.0f + product)) : atanf(alpha_end) - atanf(alpha_start);
    float rational = alpha_end / (1.0f + alpha_end * alpha_end) - alpha_start / (1.0f + alpha_start * alpha_start);
    return fmaxf((rational + angle) / QM_PI, 1e-15f);
}

// physics::doc29::corrections::lateral_attenuation_db
__device__ float lateral_attenuation_db(float height_m, float lateral_m) {
    float beta_deg = atanf(height_m / fmaxf(lateral_m, 0.01f)) * (180.0f / QM_PI);
    if (beta_deg > 50.0f) return 0.0f;
    float gamma = lateral_m <= 914.0f ? 1.089f * (1.0f - expf(-0.00274f * lateral_m)) : 1.0f;
    float lambda_beta = beta_deg < 0.0f ? 10.857f : 1.137f - 0.0229f * beta_deg + 9.72f * expf(-0.142f * beta_deg);
    return gamma * lambda_beta;
}

// physics::doc29::corrections::installation_energy_gain, wing (0) and fuselage (1); propellers 1
__device__ float installation_energy_gain(int installation, float height_m, float slant_m) {
    float a = installation == 0 ? 0.0039f : 0.1225f;
    float b = installation == 0 ? 0.062f : 0.329f;
    float c = installation == 0 ? 0.8786f : 1.0f;
    height_m = fmaxf(height_m, 0.0f);
    float sin_squared = height_m * height_m / fmaxf(slant_m * slant_m, 1e-12f);
    float cos_squared = 1.0f - sin_squared;
    float numerator = a * cos_squared + sin_squared;
    float difference = cos_squared - sin_squared;
    float denominator = c * 4.0f * sin_squared * cos_squared + difference * difference;
    return denominator > 0.0f && numerator > 0.0f ? powf(numerator, b) / denominator : 1.0f;
}

// One average piece's SEL energy per period at the receiver (physics::doc29::boxes::
// average_piece_sel), the piece `centroid_m` (relative to the receiver) along the box's axis at
// `gradient`; its closest slant to `closest_slant_m`.
__device__ void average_piece_sel(const FlightBox& b, const float* centroid_m, float gradient, const float* terrain,
                                  const float* buildings, float* sel_energy, float* closest_slant_m) {
    float half = 0.5f * fmaxf(b.average_length_m, 1.0f);
    float step[3] = {b.cos_axis * half, b.sin_axis * half, gradient * half};
    float start[3], delta[3];
    for (int axis = 0; axis < 3; axis++) {
        start[axis] = centroid_m[axis] - step[axis];
        delta[axis] = 2.0f * step[axis];
    }
    // physics::doc29::segment::closest_points
    float horizontal_squared = delta[0] * delta[0] + delta[1] * delta[1];
    float length_squared = horizontal_squared + delta[2] * delta[2];
    float along, on_line[3], on_segment[3], lateral_m, height_m, lambda_height_m;
    if (horizontal_squared <= 1e-6f) {
        along = 0.0f;
        for (int axis = 0; axis < 3; axis++) on_line[axis] = on_segment[axis] = start[axis];
        lateral_m = hypotf(start[0], start[1]);
        height_m = lambda_height_m = start[2];
    } else {
        along = -(start[0] * delta[0] + start[1] * delta[1] + start[2] * delta[2]) / length_squared;
        float clamped = clampf(along, 0.0f, 1.0f);
        for (int axis = 0; axis < 3; axis++) {
            on_line[axis] = start[axis] + along * delta[axis];
            on_segment[axis] = start[axis] + clamped * delta[axis];
        }
        lateral_m = fabsf(start[0] * delta[1] - start[1] * delta[0]) / sqrtf(horizontal_squared);
        float slant_squared = on_line[0] * on_line[0] + on_line[1] * on_line[1] + on_line[2] * on_line[2];
        height_m = copysignf(sqrtf(fmaxf(slant_squared - lateral_m * lateral_m, 0.0f)), on_line[2]);
        float cos_climb = sqrtf(horizontal_squared / length_squared);
        lambda_height_m = along >= 0.0f && along <= 1.0f ? height_m : on_segment[2] / cos_climb;
    }
    float segment_length_m = fmaxf(sqrtf(length_squared), 1.0f);
    float slant_m = sqrtf(on_line[0] * on_line[0] + on_line[1] * on_line[1] + on_line[2] * on_line[2]);
    *closest_slant_m = slant_m;
    NpdPosition position = npd_position(slant_m);
    float scaled_distance_m = expf(npd_linear(position, b.lg_scaled_distance) * QM_LN_10);
    float along_m = along * segment_length_m;
    float stretch = hypotf(gradient, 1.0f);
    float lateral_attenuation = lateral_attenuation_db(lambda_height_m, lateral_m);
    float corrections[3] = {installation_energy_gain(0, height_m, slant_m), installation_energy_gain(1, height_m, slant_m), 1.0f};
    float fraction = clampf(log10f(slant_m / INSTALLATION_SHARE_NEAR_M) / log10f(INSTALLATION_SHARE_FAR_M / INSTALLATION_SHARE_NEAR_M), 0.0f, 1.0f);
    float terrain_loss = 0.0f, building_loss = 0.0f;
    if (b.screened) {
        terrain_loss = horizon_loss_db(terrain, TERRAIN_SECTORS, on_line[0], on_line[1], on_line[2]);
        building_loss = horizon_loss_db(buildings, BUILDING_SECTORS, on_segment[0], on_segment[1], on_segment[2]);
    }
    // physics::doc29::screening::screened_sel_db(0, ...) - lateral attenuation
    float attenuation_gain = energy_of_level(-fmaxf(fmaxf(terrain_loss, building_loss) - lateral_attenuation, 0.0f) - lateral_attenuation);
    for (int period = 0; period < PERIODS; period++) {
        if (b.silent[period]) {
            sel_energy[period] = 0.0f;
            continue;
        }
        float length_m = fmaxf(b.piece_length_m[period] * stretch, 1.0f);
        float finite = finite_segment_energy_fraction(along_m + 0.5f * (length_m - segment_length_m), length_m, scaled_distance_m);
        float installation = 0.0f;
        for (int k = 0; k < 3; k++) {
            float share = b.installation_shares[period][0][k] + fraction * (b.installation_shares[period][1][k] - b.installation_shares[period][0][k]);
            installation += share * corrections[k];
        }
        if (!(installation > 0.0f)) installation = 1.0f;
        sel_energy[period] = energy_of_level(box_level_db(b.levels_db[period], b.tail_levels_db[period], position)) * finite * installation * attenuation_gain;
    }
}

// Per segment, a block: its boxes' energies and energies times their flights' Kurze lambda per
// period (popup::aircraft::boxes::tile_flight_sums) added to its point's `sums` (2 x PERIODS doubles).
extern "C" __global__ void flight_sums(const FlightBox* boxes, const FlightPoint* points, const float* horizons,
                                       const FlightSegment* segments, double* sums) {
    const FlightSegment segment = segments[blockIdx.x];
    const FlightPoint point = points[segment.point];
    const float* terrain = horizons + point.horizons;
    const float* buildings = terrain + TERRAIN_HORIZON_FLOATS;
    float reach_m = fminf(segment.within_m, AIRCRAFT_REACH_M);
    float energy[PERIODS] = {0.0f, 0.0f, 0.0f}, lambda[PERIODS] = {0.0f, 0.0f, 0.0f};
    for (u32 k = threadIdx.x; k < segment.count; k += blockDim.x) {
        const FlightBox& b = boxes[segment.first + k];
        float east_m = b.east_m - point.x, north_m = b.north_m - point.y;
        float distance_m = hypotf(east_m, north_m);
        if (distance_m > reach_m) continue;
        float share = flight_weight(segment.weight, distance_m, segment.coarse_m);
        if (share <= 0.0f) continue;
        float centroid_m[3] = {east_m, north_m, b.centroid_altitude_m - point.altitude_m};
        float sel[PERIODS], closest_slant_m;
        if (b.gradient_spread <= 0.0f) {
            average_piece_sel(b, centroid_m, b.gradient, terrain, buildings, sel, &closest_slant_m);
        } else {
            float upper[PERIODS], upper_slant_m;
            average_piece_sel(b, centroid_m, b.gradient - b.gradient_spread, terrain, buildings, sel, &closest_slant_m);
            average_piece_sel(b, centroid_m, b.gradient + b.gradient_spread, terrain, buildings, upper, &upper_slant_m);
            for (int period = 0; period < PERIODS; period++) sel[period] = 0.5f * (sel[period] + upper[period]);
        }
        float impedance = b.helicopter ? 1.0f : point.impedance_gain;
        for (int period = 0; period < PERIODS; period++) {
            float e = share * sel[period] * impedance / PERIOD_SECONDS[period];
            energy[period] += e;
            // The slant at least the NPD curves' nearest, where the exposure is read, as on the cores.
            lambda[period] += e * b.lambda_per_slant[period] * fmaxf(closest_slant_m, NPD_NEAREST_SLANT_M);
        }
    }
    __shared__ float partial[2 * PERIODS][FLIGHT_THREADS];
    for (int period = 0; period < PERIODS; period++) {
        partial[period][threadIdx.x] = energy[period];
        partial[PERIODS + period][threadIdx.x] = lambda[period];
    }
    __syncthreads();
    if (threadIdx.x < 2 * PERIODS) {
        double total = 0.0;
        for (int t = 0; t < FLIGHT_THREADS; t++) total += (double)partial[threadIdx.x][t];
        atomicAdd(&sums[(u64)segment.sum * 2 * PERIODS + threadIdx.x], total);
    }
}

// The sizes of the records the host writes, in aircraft.rs FLIGHT_STRUCT_SIZES's order.
extern "C" __global__ void flight_struct_sizes(u64* out) {
    out[0] = sizeof(FlightBox);
    out[1] = sizeof(FlightPoint);
    out[2] = sizeof(FlightSegment);
}

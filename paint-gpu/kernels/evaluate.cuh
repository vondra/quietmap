// The full physics of one source at a receiver: popup/src/evaluate.rs (a point on one ray, a line
// piece through its quadrature nodes), the weather of the receiver (physics/src/weather.rs) and
// the receiver itself (paint/src/levels.rs Point::outdoors).

// physics/src/weather.rs
#define SECTORS 16
#define WEATHER_ROWS 361
#define WEATHER_COLUMNS 720
#define WEATHER_NODE_BYTES (PERIODS * SECTORS + 2 * BANDS)
#define WEATHER_NODES_PER_DEGREE 2

// paint/src/levels.rs, popup/src/answer.rs RECEIVER_HEIGHT_M
#define RECEIVER_HEIGHT_M 4.0f

// physics::weather::PlaceWeather
struct PlaceWeather {
    float favourable[PERIODS][SECTORS];
    float alpha_db_per_km[BANDS];
};

// weather::corners and weather::place_of over the whole table's nodes (without its magic).
__device__ void place_weather(const u8* nodes, float lat, float lon, PlaceWeather* out) {
    float per_degree = (float)WEATHER_NODES_PER_DEGREE;
    float y = clampf((90.0f - lat) * per_degree, 0.0f, (float)(WEATHER_ROWS - 1));
    float x = rem_euclidf(lon, 360.0f) * per_degree;
    int row = (int)floorf(y);
    if (row > WEATHER_ROWS - 2) row = WEATHER_ROWS - 2;
    int column = (int)floorf(x);
    float fy = y - (float)row, fx = x - (float)column;
    int index[4] = {
        row * WEATHER_COLUMNS + column % WEATHER_COLUMNS,
        row * WEATHER_COLUMNS + (column + 1) % WEATHER_COLUMNS,
        (row + 1) * WEATHER_COLUMNS + column % WEATHER_COLUMNS,
        (row + 1) * WEATHER_COLUMNS + (column + 1) % WEATHER_COLUMNS,
    };
    float weight[4] = {(1.0f - fy) * (1.0f - fx), (1.0f - fy) * fx, fy * (1.0f - fx), fy * fx};
    for (int period = 0; period < PERIODS; period++) {
        for (int sector = 0; sector < SECTORS; sector++) {
            float sum = 0.0f;
            for (int k = 0; k < 4; k++) sum += weight[k] * (float)nodes[(u64)index[k] * WEATHER_NODE_BYTES + period * SECTORS + sector];
            out->favourable[period][sector] = sum / 100.0f;
        }
    }
    for (int band = 0; band < BANDS; band++) {
        float sum = 0.0f;
        for (int k = 0; k < 4; k++) {
            const u8* at = nodes + (u64)index[k] * WEATHER_NODE_BYTES + PERIODS * SECTORS + 2 * band;
            sum += weight[k] * (float)((u16)(at[0] | (at[1] << 8)));
        }
        out->alpha_db_per_km[band] = sum / 100.0f;
    }
}

// weather::FavourableProbability::at
__device__ float favourable_at(const PlaceWeather& weather, int period, float azimuth_rad) {
    float bearing_deg = rem_euclidf(90.0f - azimuth_rad * (180.0f / QM_PI), 360.0f);
    float position = bearing_deg / (360.0f / (float)SECTORS);
    int lower = ((int)floorf(position)) % SECTORS;
    int upper = (lower + 1) % SECTORS;
    float fraction = position - floorf(position);
    const float* row = weather.favourable[period];
    return row[lower] + fraction * (row[upper] - row[lower]);
}

// One receiver (popup::evaluate::Receiver without its reflection, which the painter applies to a
// pixel's sum).
struct Receiver {
    float x;
    float y;
    float altitude_m;
    u64 own_footprint;
    PlaceWeather weather;
};

// One candidate: its geometry in the square's frame (popup::candidates::Candidate) and its
// attribute (SourceAttribute) by index.
struct Candidate {
    float ax, ay, bx, by;
    float ground0, ground1;
    u32 line;
    u32 attribute;
};

struct Attribute {
    float height_m;
    float ground_factor;          // negative: the ground under the source
    float platform_half_width_m;
    float exclusion_radius_m;
    u64 footprint_id;
    float energy[PERIODS][BANDS]; // A-weighted linear band energies, per metre for lines
};

// evaluate.rs ray: one ray from (px, py) to the receiver, its transfer weighted by `weight` into
// the period sums (add_ray and period_sums, summed per period at once: the same sum).
__device__ u32 add_ray(const TerrainScene& ground, const ObstacleScene& obstacles, const Receiver& receiver,
                       const Attribute& source, const RayEnds& ends, float px, float py, bool obstacles_on_ray, float weight,
                       RayStream& s, float* periods) {
    Transfer transfer;
    u32 failed = ray_transfer(ground, obstacles, px, py, receiver.x, receiver.y, receiver.own_footprint, obstacles_on_ray, ends, s, &transfer);
    if (failed) return failed;
    float azimuth = atan2f(receiver.y - py, receiver.x - px);
    float p[PERIODS], sum[PERIODS] = {0.0f, 0.0f, 0.0f};
    for (int period = 0; period < PERIODS; period++) p[period] = favourable_at(receiver.weather, period, azimuth);
    for (int band = 0; band < BANDS; band++) {
        float air = attenuation_energy(receiver.weather.alpha_db_per_km[band] * transfer.slant_m / 1000.0f);
        float homogeneous = attenuation_energy(state_attenuation_db(transfer.states[HOMOGENEOUS], band));
        float favourable = attenuation_energy(state_attenuation_db(transfer.states[FAVOURABLE], band));
        for (int period = 0; period < PERIODS; period++) {
            sum[period] += source.energy[period][band] * air * (p[period] * favourable + (1.0f - p[period]) * homogeneous);
        }
    }
    for (int period = 0; period < PERIODS; period++) periods[period] += weight * sum[period];
    return 0;
}

// The rays of one line piece's quadrature nodes (evaluate.rs received_bands for a line).
struct LineNodes {
    const TerrainScene* ground;
    const ObstacleScene* obstacles;
    const Receiver* receiver;
    const Candidate* candidate;
    const Attribute* source;
    const RayEnds* ends;
    const LinePieceGeometry* g;
    float divergence;
    RayStream* s;
    float* periods;
    __device__ u32 operator()(float along_m, float weight_rad, bool obstacles_on_ray) {
        float fraction = along_m / g->length_m;
        float px = candidate->ax + fraction * (candidate->bx - candidate->ax);
        float py = candidate->ay + fraction * (candidate->by - candidate->ay);
        return add_ray(*ground, *obstacles, *receiver, *source, *ends, px, py, obstacles_on_ray, weight_rad * divergence, *s, periods);
    }
};

// evaluate::received_bands summed per period (period_sums): the energy one candidate delivers at
// the receiver, without the receiver's reflection; 0 or a failure.
__device__ u32 received_periods(const TerrainScene& ground, const ObstacleScene& obstacles, const Receiver& receiver,
                                const Candidate& candidate, const Attribute& source, RayStream& s, float* periods) {
    for (int period = 0; period < PERIODS; period++) periods[period] = 0.0f;
    RayEnds ends;
    ends.source_height_m = source.height_m;
    ends.receiver_altitude_m = receiver.altitude_m;
    ends.source_ground_factor = source.ground_factor;
    ends.platform_half_width_m = source.platform_half_width_m;
    ends.own_footprint = source.footprint_id;
    if (!candidate.line) {
        float distance_m = hypotf(candidate.ax - receiver.x, candidate.ay - receiver.y);
        float distance = fmaxf(hypotf(fmaxf(distance_m, source.exclusion_radius_m), receiver.altitude_m - (candidate.ground0 + source.height_m)), 1.0f);
        float weight = 1.0f / (distance * distance * energy_of_level(11.0f));
        return add_ray(ground, obstacles, receiver, source, ends, candidate.ax, candidate.ay, true, weight, s, periods);
    }
    float start[3] = {candidate.ax - receiver.x, candidate.ay - receiver.y, candidate.ground0 + source.height_m - receiver.altitude_m};
    float end[3] = {candidate.bx - receiver.x, candidate.by - receiver.y, candidate.ground1 + source.height_m - receiver.altitude_m};
    LinePieceGeometry g;
    if (!line_piece_geometry(start, end, &g)) return 0;
    LineNodes nodes = {&ground, &obstacles, &receiver, &candidate, &source, &ends, &g,
                       1.0f / (POINT_DIVERGENCE_LINEAR * g.perpendicular_m), &s, periods};
    return line_quadrature_nodes(g, obstacles, receiver.x, receiver.y, fmaxf(source.height_m, 0.0f), nodes);
}

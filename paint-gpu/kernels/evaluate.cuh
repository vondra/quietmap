// The full physics of one source at a receiver: popup/src/evaluate.rs (a point on one ray, a line
// piece through its quadrature nodes), the weather of the receiver (physics/src/weather.rs) and
// the receiver itself (paint/src/exact.rs Point::at).

// physics/src/weather.rs
#define SECTORS 16
#define WEATHER_ROWS 361
#define WEATHER_COLUMNS 720
#define WEATHER_NODE_BYTES (PERIODS * SECTORS + 2 * BANDS)
#define WEATHER_NODES_PER_DEGREE 2

// paint/src/exact.rs, popup/src/answer.rs RECEIVER_HEIGHT_M
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
    float ground0, ground1;       // the ground under the ends: the terrain, or a bridge's deck
    u32 line;
    u32 attribute;
    u32 on_deck;                  // the source stands on the deck in ground0, ground1
    u32 layer;
};

struct Attribute {
    float height_m;
    float ground_factor;          // negative: the ground under the source
    float platform_half_width_m;
    float exclusion_radius_m;
    u64 footprint_id;
    float energy[PERIODS][BANDS]; // A-weighted linear band energies, per metre for lines
};

// What a pair delivers per period (popup::evaluate::Received): its energy mixed by the direction's
// favourable share (the received bands summed), then in calm air and downwind alone (its path's
// states); PAIR_SUMS floats a pair.
#define PAIR_SUMS (3 * PERIODS)

// evaluate.rs ray: one ray from (px, py) to the receiver, its transfer weighted by `weight` into the
// pair's sums and its mixed band energies per period into the pair's `bands` (PERIODS x BANDS).
__device__ u32 add_ray(const TerrainScene& ground, const ObstacleScene& obstacles, const Receiver& receiver,
                       const Attribute& source, const RayEnds& ends, float px, float py, bool obstacles_on_ray, float weight,
                       RayStream& s, float* sums, float* bands) {
    Transfer transfer;
    u32 failed = ray_transfer(ground, obstacles, px, py, receiver.x, receiver.y, receiver.own_footprint, obstacles_on_ray, ends, s, &transfer);
    if (failed) return failed;
    float azimuth = atan2f(receiver.y - py, receiver.x - px);
    float p[PERIODS];
    for (int period = 0; period < PERIODS; period++) p[period] = favourable_at(receiver.weather, period, azimuth);
    for (int band = 0; band < BANDS; band++) {
        float air = weight * attenuation_energy(receiver.weather.alpha_db_per_km[band] * transfer.slant_m / 1000.0f);
        float homogeneous = attenuation_energy(state_attenuation_db(transfer.states[HOMOGENEOUS], band)
                                               + foliage_db(transfer.foliage_m[HOMOGENEOUS], band));
        float favourable = attenuation_energy(state_attenuation_db(transfer.states[FAVOURABLE], band)
                                              + foliage_db(transfer.foliage_m[FAVOURABLE], band));
        for (int period = 0; period < PERIODS; period++) {
            float calm = source.energy[period][band] * air * homogeneous;
            float downwind = source.energy[period][band] * air * favourable;
            float mixed = p[period] * downwind + (1.0f - p[period]) * calm;
            sums[period] += mixed;
            sums[PERIODS + period] += calm;
            sums[2 * PERIODS + period] += downwind;
            bands[period * BANDS + band] += mixed;
        }
    }
    return 0;
}

// evaluate::ray_ends: a source's ray ends at the receiver, standing on the terrain under it.
__device__ __forceinline__ RayEnds source_ends(const Attribute& source, const Receiver& receiver) {
    RayEnds ends;
    ends.source_height_m = source.height_m;
    ends.source_ground_m = QM_NAN;
    ends.receiver_altitude_m = receiver.altitude_m;
    ends.source_ground_factor = source.ground_factor;
    ends.platform_half_width_m = source.platform_half_width_m;
    ends.own_footprint = source.footprint_id;
    return ends;
}

// One ray of a pair (evaluate::received_bands: a point source's ray, or a line piece's quadrature
// node): where it leaves the source, its weight, a deck's ground under it (NaN: the terrain) and
// whether obstacles may stand on it. A pair's rays are listed together; each runs on its own thread.
struct PairRay {
    u32 pair;
    u32 obstacles_on_ray;
    float px, py;
    float weight;
    float source_ground_m;
};

// The rays candidate `candidate` sends to `receiver`, each handed to `take` (0 or a failure, which
// ends the listing): a point's one ray, weighted by its divergence; a line piece's quadrature nodes,
// each by its dphi over the piece's perpendicular (LinePieceGeometry::divergence_factor).
template <typename Take>
__device__ u32 pair_rays_of(const ObstacleScene& obstacles, const Receiver& receiver, const Candidate& candidate,
                            const Attribute& source, u32 pair, Take& take) {
    PairRay ray;
    ray.pair = pair;
    if (!candidate.line) {
        float distance_m = hypotf(candidate.ax - receiver.x, candidate.ay - receiver.y);
        float distance = fmaxf(hypotf(fmaxf(distance_m, source.exclusion_radius_m), receiver.altitude_m - (candidate.ground0 + source.height_m)), 1.0f);
        ray.obstacles_on_ray = 1u;
        ray.px = candidate.ax;
        ray.py = candidate.ay;
        ray.weight = 1.0f / (distance * distance * energy_of_level(11.0f));
        ray.source_ground_m = candidate.on_deck ? candidate.ground0 : QM_NAN;
        return take(ray);
    }
    float start[3] = {candidate.ax - receiver.x, candidate.ay - receiver.y, candidate.ground0 + source.height_m - receiver.altitude_m};
    float end[3] = {candidate.bx - receiver.x, candidate.by - receiver.y, candidate.ground1 + source.height_m - receiver.altitude_m};
    LineBuckets buckets;
    u32 failed = line_buckets_of(start, end, buckets);
    if (failed || buckets.count == 0) return failed;
    mask_wide_buckets(obstacles, receiver.x, receiver.y, fmaxf(source.height_m, 0.0f), buckets);
    const LinePieceGeometry& g = buckets.g;
    LineNodes nodes;
    nodes.count = 0;
    failed = line_quadrature_nodes(buckets, nodes);
    if (failed) return failed;
    float divergence = hypotf(g.unit[0], g.unit[1]) / (POINT_DIVERGENCE_LINEAR * g.perpendicular_m);
    for (int node = 0; node < nodes.count; node++) {
        float fraction = nodes.along_m[node] / g.length_m;
        ray.obstacles_on_ray = nodes.obstacles_on_ray[node] ? 1u : 0u;
        ray.px = candidate.ax + fraction * (candidate.bx - candidate.ax);
        ray.py = candidate.ay + fraction * (candidate.by - candidate.ay);
        ray.weight = nodes.weight_rad[node] * divergence;
        ray.source_ground_m = candidate.on_deck ? candidate.ground0 + fraction * (candidate.ground1 - candidate.ground0) : QM_NAN;
        failed = take(ray);
        if (failed) return failed;
    }
    return 0;
}

// One listed ray at the receiver: its sums (PAIR_SUMS) and mixed bands (PERIODS x BANDS) added to
// `sums` and `bands`; 0 or a failure.
__device__ u32 ray_received(const TerrainScene& ground, const ObstacleScene& obstacles, const Receiver& receiver,
                            const Attribute& source, const PairRay& ray, RayStream& s, float* sums, float* bands) {
    RayEnds ends = source_ends(source, receiver);
    if (!isnan(ray.source_ground_m)) ends.source_ground_m = ray.source_ground_m;
    return add_ray(ground, obstacles, receiver, source, ends, ray.px, ray.py, ray.obstacles_on_ray != 0u, ray.weight, s, sums,
                   bands);
}

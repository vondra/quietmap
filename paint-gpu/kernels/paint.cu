// The kernels: the energy one candidate delivers at one receiver, for pairs the host lists. The
// square's tables are in one descriptor; a thread's ray lives in its own local memory.

// Everything one square's kernels read (paint-gpu/src/device.rs writes it).
struct SquareScene {
    TerrainScene ground;
    ObstacleScene obstacles;
    const u8* weather_nodes;
    const Candidate* candidates;
    const Attribute* attributes;
    float frame_origin_x;
    float frame_origin_y;
    float frame_east_m_per_unit;
    float frame_north_m_per_unit;
};

// The sizes of the structs the host writes, in device.rs HOST_STRUCT_SIZES's order.
extern "C" __global__ void struct_sizes(u64* out) {
    out[0] = sizeof(TerrainTile);
    out[1] = sizeof(TerrainScene);
    out[2] = sizeof(ObstacleTile);
    out[3] = sizeof(ObstacleScene);
    out[4] = sizeof(Candidate);
    out[5] = sizeof(Attribute);
    out[6] = sizeof(SquareScene);
}

// geo::Mercator::to_degrees of a point in the square's frame (LocalFrame::to_mercator).
__device__ void frame_degrees(const SquareScene& square, float x, float y, float* lat, float* lon) {
    float mx = square.frame_origin_x + x / square.frame_east_m_per_unit;
    float my = square.frame_origin_y - y / square.frame_north_m_per_unit;
    float n = (float)TILES_PER_AXIS;
    *lat = atanf(sinhf(QM_PI * (1.0f - 2.0f * my / n))) * (180.0f / QM_PI);
    *lon = mx / n * 360.0f - 180.0f;
}

// The receiver at a point in the square's frame standing outdoors or in building `own_footprint`
// (levels::Point::outdoors and Point::at): its ground plus the receiver height, its weather.
__device__ u32 receiver_at(const SquareScene& square, float x, float y, u64 own_footprint, Receiver* receiver) {
    GroundSample ground;
    u32 failed = ground_at(square.ground, x, y, &ground);
    if (failed) return failed;
    receiver->x = x;
    receiver->y = y;
    receiver->altitude_m = ground.height_m + RECEIVER_HEIGHT_M;
    receiver->own_footprint = own_footprint;
    float lat, lon;
    frame_degrees(square, x, y, &lat, &lon);
    place_weather(square.weather_nodes, lat, lon, &receiver->weather);
    return 0;
}

// Per pair: the sums (PAIR_SUMS) candidate `candidate_index[i]` delivers at the receiver at
// (receiver_xy[2i], receiver_xy[2i + 1]) standing in building `own_footprint[i]` (0 outdoors), or
// its failure; an answered pair's mixed bands added to point `point_index[i]`'s in `bands`
// (LAYERS x PERIODS x BANDS a point).
extern "C" __global__ void evaluate_pairs(const SquareScene* square_ptr, const float* receiver_xy, const u64* own_footprint,
                                          const u32* candidate_index, const u32* point_index, u32 pair_count, float* sums,
                                          float* bands, u32* failed) {
    const SquareScene& square = *square_ptr;
    u64 pair = (u64)blockIdx.x * blockDim.x + threadIdx.x;
    if (pair >= pair_count) return;
    RayStream s;
    Receiver receiver;
    u32 status = receiver_at(square, receiver_xy[2 * pair], receiver_xy[2 * pair + 1], own_footprint[pair], &receiver);
    float out[PAIR_SUMS], pair_bands[PERIODS * BANDS];
    for (int k = 0; k < PAIR_SUMS; k++) out[k] = 0.0f;
    const Candidate& candidate = square.candidates[candidate_index[pair]];
    if (status == 0) {
        status = received_periods(square.ground, square.obstacles, receiver, candidate, square.attributes[candidate.attribute], s, out,
                                  pair_bands);
    }
    if (status == 0) {
        float* point_bands = bands + ((u64)point_index[pair] * LAYERS + candidate.layer) * (PERIODS * BANDS);
        for (int k = 0; k < PERIODS * BANDS; k++) atomicAdd(&point_bands[k], pair_bands[k]);
    }
    for (int k = 0; k < PAIR_SUMS; k++) sums[PAIR_SUMS * pair + k] = status == 0 ? out[k] : 0.0f;
    failed[pair] = status;
}

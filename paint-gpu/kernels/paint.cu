// The kernels: the energy one candidate delivers at one receiver, for pairs the host lists: each
// pair's rays listed, then a thread a ray (a pair's rays differ in number, a warp's threads in step),
// then the answered pairs' bands into their points. The square's tables are in one descriptor; a
// thread's ray lives in its own local memory.

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

// What a pair's listing hands each ray to: counted, or written at the pair's offset.
struct RayLister {
    PairRay* rays;
    u32 count;
    __device__ u32 operator()(const PairRay& ray) {
        if (rays) rays[count] = ray;
        count++;
        return 0;
    }
};

// Per pair (candidate `candidate_index[i]` at the receiver at (receiver_xy[2i], receiver_xy[2i + 1])
// standing in building `own_footprint[i]`, 0 outdoors): with `rays` null its rays counted into
// `ray_counts` and its failure into `failed`; else its rays written from `ray_offsets[i]`.
extern "C" __global__ void pair_rays(const SquareScene* square_ptr, const float* receiver_xy, const u64* own_footprint,
                                     const u32* candidate_index, u32 pair_count, const u32* ray_offsets, PairRay* rays,
                                     u32* ray_counts, u32* failed) {
    const SquareScene& square = *square_ptr;
    u32 pair = blockIdx.x * blockDim.x + threadIdx.x;
    if (pair >= pair_count || (rays && failed[pair])) return;
    Receiver receiver;
    u32 status = receiver_at(square, receiver_xy[2 * pair], receiver_xy[2 * pair + 1], own_footprint[pair], &receiver);
    const Candidate& candidate = square.candidates[candidate_index[pair]];
    RayLister lister{rays ? rays + ray_offsets[pair] : 0, 0u};
    if (status == 0) status = pair_rays_of(square.obstacles, receiver, candidate, square.attributes[candidate.attribute], pair, lister);
    if (!rays) {
        ray_counts[pair] = status == 0 ? lister.count : 0u;
        failed[pair] = status;
    }
}

// Per listed ray, a thread each: its pair's sums (PAIR_SUMS a pair) and mixed bands (PERIODS x BANDS
// a pair) added to, or its failure to the pair's.
extern "C" __global__ void ray_transfers(const SquareScene* square_ptr, const float* receiver_xy, const u64* own_footprint,
                                         const u32* candidate_index, const PairRay* rays, u32 ray_count, float* sums,
                                         float* pair_bands, u32* failed) {
    const SquareScene& square = *square_ptr;
    u32 k = blockIdx.x * blockDim.x + threadIdx.x;
    if (k >= ray_count) return;
    const PairRay ray = rays[k];
    u32 pair = ray.pair;
    Receiver receiver;
    u32 status = receiver_at(square, receiver_xy[2 * pair], receiver_xy[2 * pair + 1], own_footprint[pair], &receiver);
    const Candidate& candidate = square.candidates[candidate_index[pair]];
    RayStream s;
    float out[PAIR_SUMS], ray_bands[PERIODS * BANDS];
    for (int j = 0; j < PAIR_SUMS; j++) out[j] = 0.0f;
    for (int j = 0; j < PERIODS * BANDS; j++) ray_bands[j] = 0.0f;
    if (status == 0) status = ray_received(square.ground, square.obstacles, receiver, square.attributes[candidate.attribute], ray, s, out, ray_bands);
    if (status) {
        atomicOr(&failed[pair], status);
        return;
    }
    for (int j = 0; j < PAIR_SUMS; j++) atomicAdd(&sums[(u64)PAIR_SUMS * pair + j], out[j]);
    for (int j = 0; j < PERIODS * BANDS; j++) atomicAdd(&pair_bands[(u64)pair * (PERIODS * BANDS) + j], ray_bands[j]);
}

// Per answered pair: its mixed bands added to point `point_index[i]`'s in `bands` (LAYERS x PERIODS
// x BANDS a point).
extern "C" __global__ void pair_bands_to_points(const SquareScene* square_ptr, const u32* candidate_index,
                                                const u32* point_index, u32 pair_count, const float* pair_bands,
                                                const u32* failed, float* bands) {
    const SquareScene& square = *square_ptr;
    u32 pair = blockIdx.x * blockDim.x + threadIdx.x;
    if (pair >= pair_count || failed[pair]) return;
    const Candidate& candidate = square.candidates[candidate_index[pair]];
    float* point_bands = bands + ((u64)point_index[pair] * LAYERS + candidate.layer) * (PERIODS * BANDS);
    for (int j = 0; j < PERIODS * BANDS; j++) atomicAdd(&point_bands[j], pair_bands[(u64)pair * (PERIODS * BANDS) + j]);
}

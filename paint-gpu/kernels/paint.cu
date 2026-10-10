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

// Per pair: the period energies candidate `candidate_index[i]` delivers at the receiver at
// (receiver_xy[2i], receiver_xy[2i + 1]) standing in building `own_footprint[i]` (0 outdoors), or
// its failure.
extern "C" __global__ void evaluate_pairs(const SquareScene* square_ptr, const float* receiver_xy, const u64* own_footprint,
                                          const u32* candidate_index, u32 pair_count, float* periods, u32* failed) {
    const SquareScene& square = *square_ptr;
    u64 pair = (u64)blockIdx.x * blockDim.x + threadIdx.x;
    if (pair >= pair_count) return;
    RayStream s;
    Receiver receiver;
    u32 status = receiver_at(square, receiver_xy[2 * pair], receiver_xy[2 * pair + 1], own_footprint[pair], &receiver);
    float out[PERIODS] = {0.0f, 0.0f, 0.0f};
    if (status == 0) {
        const Candidate& candidate = square.candidates[candidate_index[pair]];
        status = received_periods(square.ground, square.obstacles, receiver, candidate, square.attributes[candidate.attribute], s, out);
    }
    for (int period = 0; period < PERIODS; period++) periods[PERIODS * pair + period] = status == 0 ? out[period] : 0.0f;
    failed[pair] = status;
}

// Every crossing the walk hands on along one ray, for the crossing-walk parity check.
struct CrossingRecorder {
    float* t;
    u64* footprint;
    float* height;
    u32* building;
    u32 capacity;
    u32 count;
    __device__ u32 operator()(const Crossing& c) {
        if (count < capacity) {
            t[count] = c.t;
            footprint[count] = c.footprint_id;
            height[count] = c.height_m;
            building[count] = c.building;
        }
        count++;
        return 0;
    }
};

// The crossings of the segment from -> to (frame metres) in walk order; out[0] their count, out[1]
// the walk's failure.
extern "C" __global__ void ray_crossings(const SquareScene* square, float from_x, float from_y, float to_x, float to_y,
                                         float* t, u64* footprint, float* height, u32* building, u32 capacity, u32* out) {
    CrossingRecorder recorder = {t, footprint, height, building, capacity, 0};
    out[1] = walk_crossings(square->obstacles, from_x, from_y, to_x, to_y, recorder);
    out[0] = recorder.count;
}

// The quadrature nodes of one line pair as the device places them, each with its ray's period
// energies (weighted), for the line parity check.
struct NodeRecorder {
    LineNodes nodes;
    float* along;
    float* weight;
    u32* blocked;
    float* periods;
    u32 capacity;
    u32 count;
    __device__ u32 operator()(float along_m, float weight_rad, bool obstacles_on_ray) {
        float before[PERIODS] = {nodes.periods[0], nodes.periods[1], nodes.periods[2]};
        u32 failed = nodes(along_m, weight_rad, obstacles_on_ray);
        if (count < capacity) {
            along[count] = along_m;
            weight[count] = weight_rad;
            blocked[count] = obstacles_on_ray;
            for (int p = 0; p < PERIODS; p++) periods[PERIODS * count + p] = nodes.periods[p] - before[p];
        }
        count++;
        return failed;
    }
};

// One line pair's nodes; out[0] their count, out[1] the failure.
extern "C" __global__ void line_pair_nodes(const SquareScene* square_ptr, float rx, float ry, u32 candidate_index,
                                           float* along, float* weight, u32* blocked, float* periods, u32 capacity, u32* out) {
    const SquareScene& square = *square_ptr;
    RayStream s;
    Receiver receiver;
    u32 failed = receiver_at(square, rx, ry, 0, &receiver);
    out[0] = 0;
    if (failed) { out[1] = failed; return; }
    const Candidate& candidate = square.candidates[candidate_index];
    const Attribute& source = square.attributes[candidate.attribute];
    float sums[PERIODS] = {0.0f, 0.0f, 0.0f};
    RayEnds ends;
    ends.source_height_m = source.height_m;
    ends.receiver_altitude_m = receiver.altitude_m;
    ends.source_ground_factor = source.ground_factor;
    ends.platform_half_width_m = source.platform_half_width_m;
    ends.own_footprint = source.footprint_id;
    float start[3] = {candidate.ax - receiver.x, candidate.ay - receiver.y, candidate.ground0 + source.height_m - receiver.altitude_m};
    float end[3] = {candidate.bx - receiver.x, candidate.by - receiver.y, candidate.ground1 + source.height_m - receiver.altitude_m};
    LinePieceGeometry g;
    if (!line_piece_geometry(start, end, &g)) { out[1] = 0; return; }
    LineNodes nodes = {&square.ground, &square.obstacles, &receiver, &candidate, &source, &ends, &g,
                       1.0f / (POINT_DIVERGENCE_LINEAR * g.perpendicular_m), &s, sums};
    NodeRecorder recorder = {nodes, along, weight, blocked, periods, capacity, 0};
    out[1] = line_quadrature_nodes(g, square.obstacles, receiver.x, receiver.y, fmaxf(source.height_m, 0.0f), recorder);
    out[0] = recorder.count;
}

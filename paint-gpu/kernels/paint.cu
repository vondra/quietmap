// Painter ray assembly, exact energy transfers and importance-weighted receiver spectra.
//
// The kernels: the energy one candidate delivers at one receiver, for pairs the host lists: each
// pair's rays listed, then a thread a ray (a pair's rays differ in number, a warp's threads in step),
// then the answered pairs' bands into their points. The square's tables are in one descriptor; a
// thread's ray lives in its own local memory.
// Sampled quiet pairs weight their bands by importance; their energies stay unweighted.

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
    out[7] = sizeof(Receiver);
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
// (exact.rs Point::with_receiver_properties): its ground plus the receiver height, its weather.
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

// Per launched point (its frame position at (receiver_xy[2i], receiver_xy[2i + 1]), standing in building
// `own_footprint[i]`, 0 outdoors): its receiver, built once for all its pairs' listings and rays, and
// its failure (0 for none), which each of its pairs takes.
extern "C" __global__ void point_receivers(const SquareScene* square_ptr, const float* receiver_xy, const u64* own_footprint,
                                           u32 point_count, Receiver* receivers, u32* receiver_failed) {
    u32 point = blockIdx.x * blockDim.x + threadIdx.x;
    if (point >= point_count) return;
    receiver_failed[point] = receiver_at(*square_ptr, receiver_xy[2 * point], receiver_xy[2 * point + 1], own_footprint[point],
                                         &receivers[point]);
}

// A pair's first rays kept by its counting for its writing, in the pair's bands (which the rays fill
// only later): PAIR_RAY_SLOTS of them, each as its fraction along the source segment and its signed
// weight (never negative: its sign carries the flag of obstacles on the ray, set for none). Storing
// two floats per ray doubles the cached capacity to 12 rays in the same buffer and halves slot traffic.
#define PAIR_RAY_SLOTS (PERIODS * BANDS / 2)

// What a pair's listing hands each ray to: counted and kept, or written at the pair's offset.
struct RayLister {
    PairRay* rays;
    float* slots;
    u32 count;
    __device__ u32 operator()(const PairRay& ray, float fraction) {
        if (rays) {
            rays[count] = ray;
        } else if (count < PAIR_RAY_SLOTS) {
            float* slot = slots + 2 * count;
            slot[0] = fraction;
            slot[1] = (ray.obstacles_on_ray & PAIR_RAY_OBSTACLES) ? ray.weight : -ray.weight;
        }
        count++;
        return 0;
    }
};

// Exclusive prefix sum over `counts[0..n]` in tiles of 1024 (`blockDim.x == 256`), writing
// tile-local exclusive offsets to `offsets` and each tile's total to `block_sums`.
extern "C" __global__ void scan_blocks(const u32* counts, u32 n, u32* offsets, u32* block_sums) {
    __shared__ u32 warp_sums[8];
    int t = threadIdx.x, lane = t & 31, warp = t >> 5;
    u32 base = blockIdx.x * 1024u + 4u * (u32)t;
    u32 v0 = base < n ? counts[base] : 0u;
    u32 v1 = base + 1u < n ? counts[base + 1u] : 0u;
    u32 v2 = base + 2u < n ? counts[base + 2u] : 0u;
    u32 v3 = base + 3u < n ? counts[base + 3u] : 0u;
    u32 local_sum = v0 + v1 + v2 + v3;
    u32 s = local_sum;
#pragma unroll
    for (int off = 1; off < 32; off <<= 1) {
        u32 up = __shfl_up_sync(0xffffffffu, s, off);
        if (lane >= off) s += up;
    }
    if (lane == 31) warp_sums[warp] = s;
    __syncthreads();
    if (warp == 0) {
        u32 ws = lane < 8 ? warp_sums[lane] : 0u;
#pragma unroll
        for (int off = 1; off < 8; off <<= 1) {
            u32 up = __shfl_up_sync(0xffffffffu, ws, off);
            if (lane >= off) ws += up;
        }
        if (lane < 8) warp_sums[lane] = ws;
    }
    __syncthreads();
    u32 excl = (warp > 0 ? warp_sums[warp - 1] : 0u) + (s - local_sum);
    if (base < n) offsets[base] = excl;
    excl += v0;
    if (base + 1u < n) offsets[base + 1u] = excl;
    excl += v1;
    if (base + 2u < n) offsets[base + 2u] = excl;
    excl += v2;
    if (base + 3u < n) offsets[base + 3u] = excl;
    if (t == 255) block_sums[blockIdx.x] = warp_sums[7];
}

// Single-block exclusive prefix sum over `block_sums[0..num_blocks]` (`blockDim.x == 256`),
// writing the grand total into `*total_out`.
extern "C" __global__ void scan_block_sums(u32* block_sums, u32 num_blocks, u32* total_out) {
    __shared__ u32 warp_sums[8];
    int t = threadIdx.x, lane = t & 31, warp = t >> 5;
    u32 carry = 0u;
    for (u32 base = 0u; base < num_blocks; base += 256u) {
        u32 idx = base + (u32)t;
        u32 v = idx < num_blocks ? block_sums[idx] : 0u;
        u32 s = v;
#pragma unroll
        for (int off = 1; off < 32; off <<= 1) {
            u32 up = __shfl_up_sync(0xffffffffu, s, off);
            if (lane >= off) s += up;
        }
        if (lane == 31) warp_sums[warp] = s;
        __syncthreads();
        if (warp == 0) {
            u32 ws = lane < 8 ? warp_sums[lane] : 0u;
#pragma unroll
            for (int off = 1; off < 8; off <<= 1) {
                u32 up = __shfl_up_sync(0xffffffffu, ws, off);
                if (lane >= off) ws += up;
            }
            if (lane < 8) warp_sums[lane] = ws;
        }
        __syncthreads();
        u32 excl = carry + (warp > 0 ? warp_sums[warp - 1] : 0u) + (s - v);
        if (idx < num_blocks) block_sums[idx] = excl;
        carry += warp_sums[7];
        __syncthreads();
    }
    if (t == 0) *total_out = carry;
}

// Per pair (candidate `candidate_index[i]` at receiver `receiver_of_pair[i]` of `point_receivers`): with
// `rays` null its rays counted into `ray_counts`, its first ones kept in `slots` (PERIODS x BANDS floats
// a pair) and its failure into `failed`; else its rays written in strict candidate-sorted order at
// `ray_offsets[i] + block_sums[i >> 10]` when `0 < *total_rays <= ray_capacity`.
extern "C" __global__ void pair_rays(const SquareScene* square_ptr, const Receiver* receivers, const u32* receiver_failed,
                                     const u32* receiver_of_pair, const u32* candidate_index, u32 pair_count,
                                     const u32* ray_offsets, const u32* block_sums, const u32* total_rays, u32 ray_capacity,
                                     u32 direct_single, PairRay* rays, u32* ray_counts, u32* failed, float* slots) {
    if (rays) {
        u32 total = *total_rays;
        if (total == 0u || total > ray_capacity) return;
    }
    const SquareScene& square = *square_ptr;
    u32 pair = blockIdx.x * blockDim.x + threadIdx.x;
    if (pair >= pair_count || (rays && failed[pair])) return;
    float* pair_slots = slots + (u64)pair * (PERIODS * BANDS);
    if (rays && ray_counts[pair] <= PAIR_RAY_SLOTS) {
        u32 count = ray_counts[pair];
        u32 base = ray_offsets[pair] + block_sums[pair >> 10];
        u32 single = count == 1u ? PAIR_RAY_SINGLE : 0u;
        const Candidate& candidate = square.candidates[candidate_index[pair]];
        if (!candidate.line) {
            if (count == 1u) {
                float signed_w = pair_slots[1];
                u32 flags = (signbit(signed_w) ? 0u : PAIR_RAY_OBSTACLES) | PAIR_RAY_SINGLE;
                float ground_m = candidate.on_deck ? candidate.ground0 : QM_NAN;
                rays[base] = PairRay{pair, flags, candidate.ax, candidate.ay, fabsf(signed_w), ground_m};
            }
        } else {
            float dx = candidate.bx - candidate.ax, dy = candidate.by - candidate.ay;
            float dg = candidate.ground1 - candidate.ground0;
            bool on_deck = candidate.on_deck != 0u;
            for (u32 k = 0; k < count; k++) {
                const float* slot = pair_slots + 2 * k;
                float frac = slot[0], signed_w = slot[1];
                u32 flags = (signbit(signed_w) ? 0u : PAIR_RAY_OBSTACLES) | single;
                float ground_m = on_deck ? candidate.ground0 + frac * dg : QM_NAN;
                rays[base + k] = PairRay{pair, flags, candidate.ax + frac * dx, candidate.ay + frac * dy, fabsf(signed_w), ground_m};
            }
        }
        if (direct_single && count > 1u) {
            for (int j = 0; j < PERIODS * BANDS; j++) pair_slots[j] = 0.0f;
        }
        return;
    }
    u32 at = receiver_of_pair[pair];
    const Receiver& receiver = receivers[at];
    u32 status = receiver_failed[at];
    const Candidate& candidate = square.candidates[candidate_index[pair]];
    u32 base = rays ? ray_offsets[pair] + block_sums[pair >> 10] : 0u;
    RayLister lister{rays ? rays + base : 0, pair_slots, 0u};
    if (status == 0) status = pair_rays_of(square.obstacles, receiver, candidate, square.attributes[candidate.attribute], pair, lister);
    if (!rays) {
        ray_counts[pair] = status == 0 ? lister.count : 0u;
        failed[pair] = status;
    } else if (direct_single) {
        for (int j = 0; j < PERIODS * BANDS; j++) pair_slots[j] = 0.0f;
    }
}

// One-warp ray blocks resident on a multiprocessor, which bound a thread's registers: on compute
// capability 12.0 sixteen (128 registers) beat twelve (168) by 15 %; on 7.0 and 7.5 twelve beat
// sixteen by 11 and 8 %, the walk's live state kept in registers rather than in local memory
// (2026-10-20); requiring more than sixteen makes the allowance too small for the walk.
#if __CUDA_ARCH__ >= 800
#define RAY_BLOCKS_PER_MULTIPROCESSOR 16
#else
#define RAY_BLOCKS_PER_MULTIPROCESSOR 12
#endif

// Per listed ray, a thread each: its pair's sums (PAIR_SUMS a pair) and mixed bands (PERIODS x BANDS
// a pair) added to, or its failure to the pair's. For single-ray pairs when `bands` is provided,
// mixed bands go straight into the receiver's point bands, skipping the `pair_bands` round-trip.
extern "C" __global__ void __launch_bounds__(THREADS_PER_BLOCK, RAY_BLOCKS_PER_MULTIPROCESSOR) ray_transfers(
        const SquareScene* square_ptr, const Receiver* receivers, const u32* receiver_of_pair,
        const u32* candidate_index, const u32* point_of_receiver, const float* band_weight, float* bands,
        const PairRay* rays, const u32* total_rays, u32 ray_capacity, float* sums, float* pair_bands, u32* failed) {
    u32 ray_count = *total_rays;
    if (ray_count == 0u || ray_count > ray_capacity) return;
    u32 k = blockIdx.x * blockDim.x + threadIdx.x;
    if (k >= ray_count) return;
    const SquareScene& square = *square_ptr;
    const PairRay ray = rays[k];
    u32 pair = ray.pair;
    u32 rec = receiver_of_pair[pair];
    // A listed ray's receiver was built: a pair whose receiver failed lists none.
    const Receiver& receiver = receivers[rec];
    const Candidate& candidate = square.candidates[candidate_index[pair]];
    RayStream s;
    float out[PAIR_SUMS], ray_bands[PERIODS * BANDS];
    for (int j = 0; j < PAIR_SUMS; j++) out[j] = 0.0f;
    for (int j = 0; j < PERIODS * BANDS; j++) ray_bands[j] = 0.0f;
    bool single = (ray.obstacles_on_ray & PAIR_RAY_SINGLE) != 0u;
    u32 status = ray_received(square.ground, square.obstacles, receiver, square.attributes[candidate.attribute], ray, s, out, ray_bands);
    if (status) {
        atomicOr(&failed[pair], status);
        return;
    }
    if (single) {
        for (int j = 0; j < PAIR_SUMS; j++) sums[(u64)PAIR_SUMS * pair + j] = out[j];
        if (bands) {
            float w = band_weight ? band_weight[pair] : 1.0f;
            if (w != 0.0f) {
                u32 point = point_of_receiver[rec];
                float* point_bands = bands + ((u64)point * LAYERS + candidate.layer) * (PERIODS * BANDS);
                for (int j = 0; j < PERIODS * BANDS; j++) {
                    float v = w * ray_bands[j];
                    if (v != 0.0f) atomicAdd(&point_bands[j], v);
                }
            }
        } else {
            for (int j = 0; j < PERIODS * BANDS; j++) pair_bands[(u64)pair * (PERIODS * BANDS) + j] = ray_bands[j];
        }
    } else {
        for (int j = 0; j < PAIR_SUMS; j++) atomicAdd(&sums[(u64)PAIR_SUMS * pair + j], out[j]);
        for (int j = 0; j < PERIODS * BANDS; j++) atomicAdd(&pair_bands[(u64)pair * (PERIODS * BANDS) + j], ray_bands[j]);
    }
}

// Per answered multi-ray pair: its mixed bands added to its receiver's point in `bands` (single-ray
// pairs already accumulated directly inside `ray_transfers`).
extern "C" __global__ void pair_bands_to_points(const SquareScene* square_ptr, const u32* receiver_of_pair,
                                                const u32* candidate_index, const u32* point_of_receiver,
                                                const float* band_weight, u32 pair_count, const u32* total_rays,
                                                u32 ray_capacity, const u32* ray_counts, const float* pair_bands,
                                                const u32* failed, float* bands) {
    u32 ray_count = *total_rays;
    if (ray_count == 0u || ray_count > ray_capacity) return;
    u32 pair = blockIdx.x * blockDim.x + threadIdx.x;
    if (pair >= pair_count || failed[pair] || ray_counts[pair] <= 1u) return;
    float w = band_weight ? band_weight[pair] : 1.0f;
    if (w == 0.0f) return;
    const SquareScene& square = *square_ptr;
    const Candidate& candidate = square.candidates[candidate_index[pair]];
    u32 point = point_of_receiver[receiver_of_pair[pair]];
    float* point_bands = bands + ((u64)point * LAYERS + candidate.layer) * (PERIODS * BANDS);
    const float* pb = pair_bands + (u64)pair * (PERIODS * BANDS);
    for (int j = 0; j < PERIODS * BANDS; j++) {
        float v = w * pb[j];
        if (v != 0.0f) atomicAdd(&point_bands[j], v);
    }
}

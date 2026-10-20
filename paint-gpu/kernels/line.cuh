// The point sum of a straight line piece, in f32: physics/src/line.rs (quadrature nodes uniform in the
// in-plane angle, wide buckets split by an obstacle mask) and popup/src/obstacles/skyline.rs (the
// edges near the receiver that can break a sight line).

#define LINE_BUCKET_COUNT 5
#define ONE_BUCKET_SPAN_RAD (3.0f * QM_PI / 180.0f)
#define WIDE_BUCKET_MIN_AZIMUTH_SPAN_RAD (3.0f * QM_PI / 180.0f)
#define WIDE_BUCKET_MASK_BINS 128
#define WIDE_BUCKET_PART_MAX_SPAN_RAD 0.26f
#define WIDE_BUCKET_MAX_PARTS_PER_RUN 9
#define LINE_PERPENDICULAR_FLOOR_M 0.5f
#define POINT_DIVERGENCE_LINEAR 12.589254117941673f

// line::wrap_to_pi
__device__ float wrap_to_pi(float angle) {
    float a = fmodf(angle, QM_TAU);
    if (a > QM_PI) return a - QM_TAU;
    if (a <= -QM_PI) return a + QM_TAU;
    return a;
}

// line::LinePieceGeometry
struct LinePieceGeometry {
    float start[3];
    float unit[3];
    float length_m;
    float foot_along_m;
    float perpendicular_m;
    float start_angle_rad;
};

// LinePieceGeometry::new; false for a piece shorter than a millimetre.
__device__ bool line_piece_geometry(const float* start, const float* end, LinePieceGeometry* g) {
    float along[3] = {end[0] - start[0], end[1] - start[1], end[2] - start[2]};
    float length_m = sqrtf(along[0] * along[0] + along[1] * along[1] + along[2] * along[2]);
    if (length_m < 1e-3f) return false;
    for (int k = 0; k < 3; k++) {
        g->start[k] = start[k];
        g->unit[k] = along[k] / length_m;
    }
    g->length_m = length_m;
    float foot_along_m = -(start[0] * g->unit[0] + start[1] * g->unit[1] + start[2] * g->unit[2]);
    float foot[3] = {start[0] + foot_along_m * g->unit[0], start[1] + foot_along_m * g->unit[1], start[2] + foot_along_m * g->unit[2]};
    float perpendicular_m = fmaxf(sqrtf(foot[0] * foot[0] + foot[1] * foot[1] + foot[2] * foot[2]), LINE_PERPENDICULAR_FLOOR_M);
    g->foot_along_m = foot_along_m;
    g->perpendicular_m = perpendicular_m;
    g->start_angle_rad = atanf((0.0f - foot_along_m) / perpendicular_m);
    return true;
}

__device__ __forceinline__ float in_plane_angle_at(const LinePieceGeometry& g, float along_m) {
    return atanf((along_m - g.foot_along_m) / g.perpendicular_m);
}
__device__ __forceinline__ float along_at_in_plane_angle(const LinePieceGeometry& g, float angle_rad) {
    return clampf(g.foot_along_m + g.perpendicular_m * tanf(angle_rad), 0.0f, g.length_m);
}
__device__ __forceinline__ void point_at(const LinePieceGeometry& g, float along_m, float* p) {
    for (int k = 0; k < 3; k++) p[k] = g.start[k] + along_m * g.unit[k];
}
// LinePieceGeometry::along_at_azimuth; false for a ray parallel to the piece.
__device__ bool along_at_azimuth(const LinePieceGeometry& g, float azimuth_rad, float* along) {
    float dx = cosf(azimuth_rad), dy = sinf(azimuth_rad);
    float ux = g.unit[0] * g.length_m, uy = g.unit[1] * g.length_m;
    float denominator = dx * uy - dy * ux;
    if (fabsf(denominator) < 1e-12f) return false;
    float fraction = (dy * g.start[0] - dx * g.start[1]) / denominator;
    *along = clampf(fraction, 0.0f, 1.0f) * g.length_m;
    return true;
}
__device__ __forceinline__ float azimuth_at(const LinePieceGeometry& g, float along_m) {
    float p[3];
    point_at(g, along_m, p);
    return atan2f(p[1], p[0]);
}
__device__ __forceinline__ float horizontal_range_at(const LinePieceGeometry& g, float along_m) {
    float p[3];
    point_at(g, along_m, p);
    return hypotf(p[0], p[1]);
}

// A wide bucket's mask, a bit a bin.
#define WIDE_BUCKET_MASK_WORDS (WIDE_BUCKET_MASK_BINS / 32)

// Bins `first` to `last` of a mask blocked.
__device__ __forceinline__ void block_bins(u32* mask, int first, int last) {
#pragma unroll
    for (int w = 0; w < WIDE_BUCKET_MASK_WORDS; w++) {
        int lo = max(first, 32 * w), hi = min(last, 32 * w + 31);
        if (lo <= hi) {
            u32 bits = hi - lo == 31 ? 0xffffffffu : ((1u << (hi - lo + 1)) - 1u);
            mask[w] |= bits << (lo - 32 * w);
        }
    }
}

__device__ __forceinline__ bool bin_blocked(const u32* mask, int bin) {
    return (mask[bin >> 5] >> (bin & 31)) & 1u;
}

// line::mark_blocked_bins for one arc.
__device__ void mark_blocked_bins(const LinePieceGeometry& g, float arc_lo, float arc_hi, float arc_nearest,
                                  float need_radius, float span_lo, float span_hi, float bin_width, u32* mask) {
    if (arc_nearest > need_radius) return;
    const float shifts[3] = {0.0f, 2.0f * QM_PI, -2.0f * QM_PI};
    for (int s = 0; s < 3; s++) {
        float piece_lo = fmaxf(arc_lo + shifts[s], span_lo);
        float piece_hi = fminf(arc_hi + shifts[s], span_hi);
        if (piece_hi <= piece_lo) continue;
        float along;
        if (!along_at_azimuth(g, 0.5f * (piece_lo + piece_hi), &along)) continue;
        if (horizontal_range_at(g, along) - arc_nearest <= 1.0f) continue;
        float first_real = fmaxf(floorf((piece_lo - span_lo) / bin_width), 0.0f);
        int first = (int)first_real;
        long long last_raw = (long long)ceilf((piece_hi - span_lo) / bin_width);
        long long last = last_raw > 0 ? last_raw - 1 : 0;
        if (last < first) last = first;
        if (last > WIDE_BUCKET_MASK_BINS - 1) last = WIDE_BUCKET_MASK_BINS - 1;
        block_bins(mask, first, (int)last);
    }
}

// skyline::origin_to_segment_distance
__device__ float origin_to_segment_distance(float ax, float ay, float bx, float by) {
    float ex = bx - ax, ey = by - ay;
    float length2 = ex * ex + ey * ey;
    float t = length2 > 0.0f ? clampf(-(ax * ex + ay * ey) / length2, 0.0f, 1.0f) : 0.0f;
    float x = ax + t * ex, y = ay + t * ey;
    return sqrtf(x * x + y * y);
}

// A wide bucket of a line piece (line::push_wide_bucket_nodes): its ends along the piece, its azimuth
// span and turn, the radius its skyline needs and its bins' width.
struct WideSpan {
    float along_lo, along_hi;
    float span_lo, span_hi, turn;
    float need_radius;
    float bin_width;
};

// The bucket from in-plane angle `angle_lo` to `angle_hi` as a wide one; false when its azimuth span
// is too narrow for a mask (the bucket keeps its centre node).
__device__ bool wide_span(const LinePieceGeometry& g, float angle_lo, float angle_hi, WideSpan* w) {
    float along_lo = along_at_in_plane_angle(g, angle_lo), along_hi = along_at_in_plane_angle(g, angle_hi);
    float azimuth_a = azimuth_at(g, along_lo);
    float turn = wrap_to_pi(azimuth_at(g, along_hi) - azimuth_a);
    float span = fabsf(turn);
    if (span < WIDE_BUCKET_MIN_AZIMUTH_SPAN_RAD) return false;
    float range_lo = horizontal_range_at(g, along_lo), range_hi = horizontal_range_at(g, along_hi);
    float a[3], b[3];
    point_at(g, along_lo, a);
    point_at(g, along_hi, b);
    float chord = hypotf(a[0] - b[0], a[1] - b[1]);
    float centre_range = horizontal_range_at(g, along_at_in_plane_angle(g, 0.5f * (angle_lo + angle_hi)));
    w->along_lo = along_lo;
    w->along_hi = along_hi;
    w->span_lo = turn < 0.0f ? azimuth_a + turn : azimuth_a;
    w->span_hi = turn < 0.0f ? azimuth_a : azimuth_a + turn;
    w->turn = turn;
    w->need_radius = fminf(fminf(range_lo, range_hi), centre_range) + chord;
    w->bin_width = span / (float)WIDE_BUCKET_MASK_BINS;
    return true;
}

// A line piece's quadrature nodes, collected as the CPU collects them.
struct LineNodes {
    int count;
    float along_m[LINE_NODES];
    float weight_rad[LINE_NODES];
    bool obstacles_on_ray[LINE_NODES];
    __device__ u32 operator()(float along_m, float weight_rad, bool obstacles_on_ray) {
        if (count == LINE_NODES) return FAILED_CAPACITY;
        this->along_m[count] = along_m;
        this->weight_rad[count] = weight_rad;
        this->obstacles_on_ray[count++] = obstacles_on_ray;
        return 0;
    }
};

// line::push_wide_bucket_nodes from the bucket's mask, each node added to `node` (0 or a failure) as
// it is placed; false when the bucket keeps its centre node.
__device__ bool push_wide_bucket_nodes(const LinePieceGeometry& g, float angle_lo, float angle_hi, const WideSpan& w,
                                       const u32* mask, LineNodes& node, u32* failed) {
    bool any = false;
    for (int k = 0; k < WIDE_BUCKET_MASK_WORDS; k++) any = any || mask[k] != 0u;
    if (!any) return false;
    float span_lo = w.span_lo, span_hi = w.span_hi, bin_width = w.bin_width;
    float angle_at_span_lo = w.turn < 0.0f ? angle_hi : angle_lo;
    float angle_at_span_hi = w.turn < 0.0f ? angle_lo : angle_hi;
    float lo_bound = fminf(w.along_lo, w.along_hi), hi_bound = fmaxf(w.along_lo, w.along_hi);
    int bin = 0;
    while (bin < WIDE_BUCKET_MASK_BINS) {
        bool run_blocked = bin_blocked(mask, bin);
        int run_end = bin;
        while (run_end < WIDE_BUCKET_MASK_BINS && bin_blocked(mask, run_end) == run_blocked) run_end++;
        float run_lo = span_lo + (float)bin * bin_width;
        float run_hi = run_end == WIDE_BUCKET_MASK_BINS ? span_hi : span_lo + (float)run_end * bin_width;
        int parts = (int)clampf(ceilf((run_hi - run_lo) / WIDE_BUCKET_PART_MAX_SPAN_RAD), 1.0f, (float)WIDE_BUCKET_MAX_PARTS_PER_RUN);
        float step = (run_hi - run_lo) / (float)parts;
        for (int part = 0; part < parts; part++) {
            float part_lo = run_lo + (float)part * step;
            float part_hi = (part + 1 == parts && run_end == WIDE_BUCKET_MASK_BINS) ? span_hi : part_lo + step;
            float edge_lo, edge_hi, along;
            if (bin == 0 && part == 0) {
                edge_lo = angle_at_span_lo;
            } else if (along_at_azimuth(g, part_lo, &along)) {
                edge_lo = in_plane_angle_at(g, clampf(along, lo_bound, hi_bound));
            } else {
                edge_lo = angle_at_span_lo;
            }
            if (run_end == WIDE_BUCKET_MASK_BINS && part + 1 == parts) {
                edge_hi = angle_at_span_hi;
            } else if (along_at_azimuth(g, part_hi, &along)) {
                edge_hi = in_plane_angle_at(g, clampf(along, lo_bound, hi_bound));
            } else {
                edge_hi = angle_at_span_hi;
            }
            float node_along;
            if (!along_at_azimuth(g, 0.5f * (part_lo + part_hi), &node_along)) {
                node_along = along_at_in_plane_angle(g, 0.5f * (edge_lo + edge_hi));
            }
            *failed = node(node_along, fabsf(edge_hi - edge_lo), run_blocked);
            if (*failed) return true;
        }
        bin = run_end;
    }
    return true;
}

// A line piece at a receiver between its buckets' spans and their nodes (line::line_quadrature_nodes):
// its geometry, its buckets (none for a piece shorter than a millimetre) and their angle, and each
// wide bucket's span and, once its skyline is walked, its mask.
struct LineBuckets {
    LinePieceGeometry g;
    int count;
    float bucket_angle;
    bool wide[LINE_BUCKET_COUNT];
    WideSpan span[LINE_BUCKET_COUNT];
    u32 mask[LINE_BUCKET_COUNT][WIDE_BUCKET_MASK_WORDS];
};

// The piece from `start` to `end` (relative to the receiver) cut into its buckets, the wide ones'
// spans taken; 0 or a failure.
__device__ u32 line_buckets_of(const float* start, const float* end, LineBuckets& b) {
    b.count = 0;
    if (!line_piece_geometry(start, end, &b.g)) return 0;
    const LinePieceGeometry& g = b.g;
    // The span and the nodes from the start's angle, not as differences of two angles near
    // +-pi/2: a short piece pointing at a far receiver keeps its energy in f32 (in f64 the CPU's
    // differences are exact enough).
    float a = -g.foot_along_m, p = g.perpendicular_m;
    float span = atan2f(g.length_m * p, p * p + a * (g.length_m - g.foot_along_m));
    // Cutoff probes differ by several millionths of a radian after endpoint rounding. This narrow
    // neighbourhood goes through the CPU rather than let one rounded angle choose one ray or five.
    if (fabsf(span - ONE_BUCKET_SPAN_RAD) <= 1e-5f) return FAILED_LINE_CUTOFF;
    b.count = span < ONE_BUCKET_SPAN_RAD ? 1 : LINE_BUCKET_COUNT;
    b.bucket_angle = span / (float)b.count;
    for (int bucket = 0; bucket < b.count; bucket++) {
        float angle_lo = g.start_angle_rad + (float)bucket * b.bucket_angle;
        b.wide[bucket] = wide_span(g, angle_lo, angle_lo + b.bucket_angle, &b.span[bucket]);
    }
    return 0;
}

// obstacles::Scene::skyline_arcs of each wide bucket of the piece at the receiver (origin_x,
// origin_y), each arc marked into the bucket's mask (line.rs's visit), in one walk of the cells for
// all of them: the buckets share the receiver, so each cell, run and edge is read and its arc taken
// once and marked into every bucket whose box, wedge and radius take the cell and the edge (the five
// buckets of a near piece walked the same cells five times, most of the listing's time in a dense
// city square, 2026-10-20).
__device__ void mask_wide_buckets(const ObstacleScene& scene, float origin_x, float origin_y, float los_floor_m, LineBuckets& b) {
    struct Wedge {
        float low_c, low_s, high_c, high_s;
        i64 row_lo, row_hi, col_lo, col_hi;
    };
    Wedge wedges[LINE_BUCKET_COUNT];
    float ox, oy;
    lattice_steps(scene, origin_x, origin_y, &ox, &oy);
    float east_m = scene.metres_per_step_x, north_m = scene.metres_per_step_y;
    const float cell_steps = (float)CELL_STEPS;
    u32 wide = 0;
    i64 row_lo = 0, row_hi = -1, col_lo = 0, col_hi = -1;
    for (int k = 0; k < b.count; k++) {
        if (!b.wide[k]) continue;
        for (int w = 0; w < WIDE_BUCKET_MASK_WORDS; w++) b.mask[k][w] = 0u;
        const WideSpan& w = b.span[k];
        float radius_m = w.need_radius;
        Wedge& e = wedges[k];
        e.low_c = cosf(w.span_lo);
        e.low_s = sinf(w.span_lo);
        e.high_c = cosf(w.span_hi);
        e.high_s = sinf(w.span_hi);
        e.row_lo = (i64)floorf((oy - radius_m / north_m) / cell_steps);
        e.row_hi = (i64)floorf((oy + radius_m / north_m) / cell_steps);
        e.col_lo = (i64)floorf((ox - radius_m / east_m) / cell_steps);
        e.col_hi = (i64)floorf((ox + radius_m / east_m) / cell_steps);
        if (!wide) {
            row_lo = e.row_lo, row_hi = e.row_hi, col_lo = e.col_lo, col_hi = e.col_hi;
        } else {
            row_lo = min(row_lo, e.row_lo), row_hi = max(row_hi, e.row_hi);
            col_lo = min(col_lo, e.col_lo), col_hi = max(col_hi, e.col_hi);
        }
        wide |= 1u << k;
    }
    for (i64 row = row_lo; row <= row_hi; row++) {
        float south = (oy - (float)(row + 1) * cell_steps) * north_m;
        float north = (oy - (float)row * cell_steps) * north_m;
        float dy = fmaxf(fmaxf(south, -north), 0.0f);
        for (i64 column = col_lo; column <= col_hi; column++) {
            float west = ((float)column * cell_steps - ox) * east_m;
            float east = ((float)(column + 1) * cell_steps - ox) * east_m;
            float dx = fmaxf(fmaxf(west, -east), 0.0f);
            float cx[4] = {west, east, west, east}, cy[4] = {south, south, north, north};
            // The buckets that walk this cell.
            u32 walking = 0;
            for (int k = 0; k < b.count; k++) {
                const Wedge& e = wedges[k];
                if (!(wide >> k & 1u) || row < e.row_lo || row > e.row_hi || column < e.col_lo || column > e.col_hi) continue;
                float radius_m = b.span[k].need_radius;
                if (dx * dx + dy * dy > radius_m * radius_m) continue;
                bool below = true, above = true;
                for (int c = 0; c < 4; c++) {
                    if (!(e.low_c * cy[c] - e.low_s * cx[c] < 0.0f)) below = false;
                    if (!(cx[c] * e.high_s - cy[c] * e.high_c < 0.0f)) above = false;
                }
                if (!(below || above)) walking |= 1u << k;
            }
            if (!walking) continue;
            const ObstacleTile* tile = 0;
            int index = 0;
            if (scene_locate(scene, column, row, &tile, &index) != TILE_READ) continue;
            if (scene.cell_max_height[tile->cell_max_offset + index] <= los_floor_m) continue;
            u32 run_begin, run_end;
            tile_cell_runs(scene, *tile, index, &run_begin, &run_end);
            const u8* runs = tile_runs(scene, *tile);
            for (u32 run_index = run_begin; run_index < run_end; run_index++) {
                const u8* run = runs + (u64)RUN_BYTES * run_index;
                Outline outline = tile_outline(scene, *tile, read_u32(run));
                if (outline.kind == OUTLINE_KIND_WALL && outline.height_m <= los_floor_m) continue;
                u32 first = outline.first_vertex + read_u16(run + 4);
                u32 edge_count = read_u16(run + 6);
                int lx, ly;
                tile_vertex_local(scene, *tile, first, &lx, &ly);
                float ax = (tile->offset_x + (float)lx - ox) * east_m, ay = (oy - (tile->offset_y + (float)ly)) * north_m;
                float azimuth_a = atan2f(ay, ax);
                for (u32 next = first + 1; next <= first + edge_count; next++) {
                    tile_vertex_local(scene, *tile, next, &lx, &ly);
                    float bx = (tile->offset_x + (float)lx - ox) * east_m, by = (oy - (tile->offset_y + (float)ly)) * north_m;
                    float azimuth_b = atan2f(by, bx);
                    float nearest_m = origin_to_segment_distance(ax, ay, bx, by);
                    if (nearest_m >= 1e-6f) {
                        float unwrapped_b = azimuth_a + wrap_to_pi(azimuth_b - azimuth_a);
                        float arc_lo = fminf(azimuth_a, unwrapped_b), arc_hi = fmaxf(azimuth_a, unwrapped_b);
                        for (int k = 0; k < b.count; k++) {
                            const WideSpan& w = b.span[k];
                            if ((walking >> k & 1u) && nearest_m <= w.need_radius) {
                                mark_blocked_bins(b.g, arc_lo, arc_hi, nearest_m, w.need_radius, w.span_lo, w.span_hi, w.bin_width, b.mask[k]);
                            }
                        }
                    }
                    ax = bx;
                    ay = by;
                    azimuth_a = azimuth_b;
                }
            }
        }
    }
}

// line::line_quadrature_nodes into `node` from the buckets and their masks; 0 or a failure.
__device__ u32 line_quadrature_nodes(const LineBuckets& b, LineNodes& node) {
    const LinePieceGeometry& g = b.g;
    float a = -g.foot_along_m, p = g.perpendicular_m;
    for (int bucket = 0; bucket < b.count; bucket++) {
        float angle_lo = g.start_angle_rad + (float)bucket * b.bucket_angle;
        float angle_hi = angle_lo + b.bucket_angle;
        u32 failed = 0;
        if (b.wide[bucket] && push_wide_bucket_nodes(g, angle_lo, angle_hi, b.span[bucket], b.mask[bucket], node, &failed)) {
            if (failed) return failed;
            continue;
        }
        // tan(start + phi) = (A + T) / (1 - A T) with A = a / p, so along = T (p^2 + a^2) / (p - a T).
        float t = tanf(((float)bucket + 0.5f) * b.bucket_angle);
        float along = clampf(t * (p * p + a * a) / (p - a * t), 0.0f, g.length_m);
        failed = node(along, b.bucket_angle, true);
        if (failed) return failed;
    }
    return 0;
}

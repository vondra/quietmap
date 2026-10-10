// The point sum of a straight line piece, in f32: physics/src/line.rs (quadrature nodes uniform in the
// in-plane angle, wide buckets split by an obstacle mask) and popup/src/obstacles/skyline.rs (the
// edges near the receiver that can break a sight line).

#define LINE_BUCKET_COUNT 5
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
    float end_angle_rad;
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
    g->end_angle_rad = atanf((length_m - foot_along_m) / perpendicular_m);
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

// line::mark_blocked_bins for one arc.
__device__ void mark_blocked_bins(const LinePieceGeometry& g, float arc_lo, float arc_hi, float arc_nearest,
                                  float need_radius, float span_lo, float span_hi, float bin_width, bool* blocked) {
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
        for (long long bin = first; bin <= last; bin++) blocked[bin] = true;
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

// obstacles::Scene::skyline_arcs, each arc marked straight into the mask (line.rs's visit).
__device__ void skyline_mark(const ObstacleScene& scene, float origin_x, float origin_y, float lo_rad, float hi_rad,
                             float radius_m, float los_floor_m,
                             const LinePieceGeometry& g, float need_radius, float span_lo, float span_hi,
                             float bin_width, bool* blocked) {
    float ox, oy;
    lattice_steps(scene, origin_x, origin_y, &ox, &oy);
    float east_m = scene.metres_per_step_x, north_m = scene.metres_per_step_y;
    const float cell_steps = (float)CELL_STEPS;
    float low_c = cosf(lo_rad), low_s = sinf(lo_rad), high_c = cosf(hi_rad), high_s = sinf(hi_rad);
    i64 row_lo = (i64)floorf((oy - radius_m / north_m) / cell_steps), row_hi = (i64)floorf((oy + radius_m / north_m) / cell_steps);
    i64 col_lo = (i64)floorf((ox - radius_m / east_m) / cell_steps), col_hi = (i64)floorf((ox + radius_m / east_m) / cell_steps);
    for (i64 row = row_lo; row <= row_hi; row++) {
        float south = (oy - (float)(row + 1) * cell_steps) * north_m;
        float north = (oy - (float)row * cell_steps) * north_m;
        float dy = fmaxf(fmaxf(south, -north), 0.0f);
        for (i64 column = col_lo; column <= col_hi; column++) {
            float west = ((float)column * cell_steps - ox) * east_m;
            float east = ((float)(column + 1) * cell_steps - ox) * east_m;
            float dx = fmaxf(fmaxf(west, -east), 0.0f);
            if (dx * dx + dy * dy > radius_m * radius_m) continue;
            float cx[4] = {west, east, west, east}, cy[4] = {south, south, north, north};
            bool below = true, above = true;
            for (int k = 0; k < 4; k++) {
                if (!(low_c * cy[k] - low_s * cx[k] < 0.0f)) below = false;
                if (!(cx[k] * high_s - cy[k] * high_c < 0.0f)) above = false;
            }
            if (below || above) continue;
            const ObstacleTile* tile = 0;
            int index = 0;
            if (scene_locate(scene, column, row, &tile, &index) != TILE_READ) continue;
            if (scene.cell_max_height[tile->cell_max_offset + index] <= los_floor_m) continue;
            const u8* cells = tile_cells(scene, *tile);
            u32 run_begin = read_u32(cells + 4 * index), run_end = read_u32(cells + 4 * (index + 1));
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
                    if (nearest_m >= 1e-6f && nearest_m <= radius_m) {
                        float unwrapped_b = azimuth_a + wrap_to_pi(azimuth_b - azimuth_a);
                        mark_blocked_bins(g, fminf(azimuth_a, unwrapped_b), fmaxf(azimuth_a, unwrapped_b), nearest_m,
                                          need_radius, span_lo, span_hi, bin_width, blocked);
                    }
                    ax = bx;
                    ay = by;
                    azimuth_a = azimuth_b;
                }
            }
        }
    }
}

// line::push_wide_bucket_nodes, each node handed to `node(along_m, weight_rad, obstacles_on_ray)`
// (0 or a failure) as it is placed; false when the bucket keeps its centre node.
template <typename Node>
__device__ bool push_wide_bucket_nodes(const LinePieceGeometry& g, float angle_lo, float angle_hi,
                                       const ObstacleScene& scene, float receiver_x, float receiver_y, float los_floor_m,
                                       Node& node, u32* failed) {
    float along_lo = along_at_in_plane_angle(g, angle_lo), along_hi = along_at_in_plane_angle(g, angle_hi);
    float azimuth_a = azimuth_at(g, along_lo);
    float turn = wrap_to_pi(azimuth_at(g, along_hi) - azimuth_a);
    float span = fabsf(turn);
    if (span < WIDE_BUCKET_MIN_AZIMUTH_SPAN_RAD) return false;
    float span_lo = turn < 0.0f ? azimuth_a + turn : azimuth_a;
    float span_hi = turn < 0.0f ? azimuth_a : azimuth_a + turn;
    float range_lo = horizontal_range_at(g, along_lo), range_hi = horizontal_range_at(g, along_hi);
    float a[3], b[3];
    point_at(g, along_lo, a);
    point_at(g, along_hi, b);
    float chord = hypotf(a[0] - b[0], a[1] - b[1]);
    float centre_range = horizontal_range_at(g, along_at_in_plane_angle(g, 0.5f * (angle_lo + angle_hi)));
    float need_radius = fminf(fminf(range_lo, range_hi), centre_range) + chord;
    float bin_width = span / (float)WIDE_BUCKET_MASK_BINS;
    bool blocked[WIDE_BUCKET_MASK_BINS];
    for (int bin = 0; bin < WIDE_BUCKET_MASK_BINS; bin++) blocked[bin] = false;
    skyline_mark(scene, receiver_x, receiver_y, span_lo, span_hi, need_radius, los_floor_m, g, need_radius, span_lo, span_hi, bin_width, blocked);
    bool any = false;
    for (int bin = 0; bin < WIDE_BUCKET_MASK_BINS; bin++) any = any || blocked[bin];
    if (!any) return false;
    float angle_at_span_lo = turn < 0.0f ? angle_hi : angle_lo;
    float angle_at_span_hi = turn < 0.0f ? angle_lo : angle_hi;
    float lo_bound = fminf(along_lo, along_hi), hi_bound = fmaxf(along_lo, along_hi);
    int bin = 0;
    while (bin < WIDE_BUCKET_MASK_BINS) {
        bool run_blocked = blocked[bin];
        int run_end = bin;
        while (run_end < WIDE_BUCKET_MASK_BINS && blocked[run_end] == run_blocked) run_end++;
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

// line::line_quadrature_nodes, each node handed to `node(along_m, weight_rad, obstacles_on_ray)`
// as it is placed (the CPU collects them first; the sum is the same); 0 or a failure.
template <typename Node>
__device__ u32 line_quadrature_nodes(const LinePieceGeometry& g, const ObstacleScene& scene, float receiver_x, float receiver_y,
                                     float los_floor_m, Node& node) {
    // The span and the nodes from the start's angle, not as differences of two angles near
    // +-pi/2: a short piece pointing at a far receiver keeps its energy in f32 (in f64 the CPU's
    // differences are exact enough).
    float a = -g.foot_along_m, p = g.perpendicular_m;
    float span = atan2f(g.length_m * p, p * p + a * (g.length_m - g.foot_along_m));
    float bucket_angle = span / (float)LINE_BUCKET_COUNT;
    for (int bucket = 0; bucket < LINE_BUCKET_COUNT; bucket++) {
        float angle_lo = g.start_angle_rad + (float)bucket * bucket_angle;
        float angle_hi = angle_lo + bucket_angle;
        u32 failed = 0;
        if (push_wide_bucket_nodes(g, angle_lo, angle_hi, scene, receiver_x, receiver_y, los_floor_m, node, &failed)) {
            if (failed) return failed;
            continue;
        }
        // tan(start + phi) = (A + T) / (1 - A T) with A = a / p, so along = T (p^2 + a^2) / (p - a T).
        float t = tanf(((float)bucket + 0.5f) * bucket_angle);
        float along = clampf(t * (p * p + a * a) / (p - a * t), 0.0f, g.length_m);
        failed = node(along, bucket_angle, true);
        if (failed) return failed;
    }
    return 0;
}

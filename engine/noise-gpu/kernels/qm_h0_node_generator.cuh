// Bounded CUDA mirror of LineNodeIterator::new_h0.
//
// H0 has at most the two source arms split by the mandatory foot boundary, no
// skyline hints and no retained candidate state. Every arithmetic expression
// below mirrors engine/noise-compute/src/compute/element.rs through the H0-*
// labels and the RN helpers from qm_shared_math.cuh.

#ifndef QM_H0_NODE_GENERATOR_CUH
#define QM_H0_NODE_GENERATOR_CUH

#include "qm_streaming_reduction.cuh"

#ifndef V2_H0_NODE_CAP
#error "V2_H0_NODE_CAP must be generated from compute/element.rs"
#endif
#ifndef V2_THETA_MAX_RAD
#error "V2_THETA_MAX_RAD must be generated from compute/element.rs"
#endif
#ifndef V2_THETA_MAX_RAD_BITS
#error "V2_THETA_MAX_RAD_BITS must come from the verified H0 selection"
#endif
#ifndef V2_H0_SELECTION_SCHEMA
#error "V2_H0_SELECTION_SCHEMA must come from the verified H0 selection"
#endif
#ifndef V2_H0_SELECTION_EPOCH
#error "V2_H0_SELECTION_EPOCH must come from the verified H0 selection"
#endif
#ifndef V2_H0_H_MAX
#error "V2_H0_H_MAX must come from the verified H0 selection"
#endif
#ifndef V2_ROAD_D_FLOOR_M
#error "V2_ROAD_D_FLOOR_M must be generated from compute/element.rs"
#endif
#ifndef V2_RAIL_D_FLOOR_M
#error "V2_RAIL_D_FLOOR_M must be generated from compute/element.rs"
#endif
#ifndef V2_R_ATM_BASE_M_PER_RAD
#error "V2_R_ATM_BASE_M_PER_RAD must be generated from compute/element.rs"
#endif
#ifndef V2_A_LIVE_DB
#error "V2_A_LIVE_DB must be generated from compute/element.rs"
#endif
#ifndef V2_LINE_MAX_LENGTH_M
#error "V2_LINE_MAX_LENGTH_M must be generated from compute/element.rs"
#endif
#if V2_H0 && V2_H0_SELECTION_SCHEMA != 1
#error "an H0 production role requires selection schema 1"
#endif
#if V2_H0 && V2_H0_SELECTION_EPOCH == 0
#error "an H0 production role requires a nonzero selection epoch"
#endif
#if V2_H0_H_MAX != 0
#error "the reviewed H0 production mechanism has no retained skyline hints"
#endif
#if V2_H0_NODE_CAP != 40 && V2_H0_NODE_CAP != 50 && V2_H0_NODE_CAP != 66 && \
    V2_H0_NODE_CAP != 99
#error "H0 node cap is outside the reviewed V3 theorem instances"
#endif
#if (V2_THETA_MAX_RAD_BITS == 0x3fb657184ae74487ull && V2_H0_NODE_CAP != 40) || \
    (V2_THETA_MAX_RAD_BITS == 0x3fb1df46a2529d39ull && V2_H0_NODE_CAP != 50) || \
    (V2_THETA_MAX_RAD_BITS == 0x3faacee9f37bebd5ull && V2_H0_NODE_CAP != 66) || \
    (V2_THETA_MAX_RAD_BITS == 0x3fa1df46a2529d39ull && V2_H0_NODE_CAP != 99)
#error "H0 theta bits and theorem cap are not the same reviewed V3 pair"
#endif
#if V2_THETA_MAX_RAD_BITS != 0x3fb657184ae74487ull && \
    V2_THETA_MAX_RAD_BITS != 0x3fb1df46a2529d39ull && \
    V2_THETA_MAX_RAD_BITS != 0x3faacee9f37bebd5ull && \
    V2_THETA_MAX_RAD_BITS != 0x3fa1df46a2529d39ull
#error "H0 theta bits are outside the reviewed V3 points"
#endif
#if V2_H0_NODE_CAP > 128
#error "the H0 node mask has exactly two u64 words"
#endif

#define QM_H0_FAULT_NONFINITE 1u
#define QM_H0_FAULT_DEGENERATE_PIECE 2u
#define QM_H0_FAULT_OVERLENGTH_PIECE 4u
#define QM_H0_FAULT_NODE_CAP 8u
#define QM_H0_FAULT_INTERNAL 16u

typedef struct {
    qm_metric_vector position_m;
    double piece_fraction;
    double weight;
    double placement_distance_m;
    double node_x_m;
    double node_distance_m;
    int arm;
} qm_h0_node;

typedef struct {
    qm_metric_vector foot_m;
    qm_metric_vector direction;
    double piece_length_m;
    double foot_from_start_m;
    double h_m;
    double emission_per_m;
    double low_s;
    double high_s;
    double low_u;
    double high_u;
    double bounds[3];
    int bound_count;
    int run_index;
    int cell_index;
    int cell_count;
    double cell_near_s;
    double cell_far_s;
    int cell_arm;
    double current_s;
    int cell_ready;
    int emitted;
    unsigned int fault_bits;
} qm_h0_iterator;

QM_FN double qm_h0_alpha_atm(int index) {
    switch (index) {
        case 0: return V2_ALPHA_ATM_0;
        case 1: return V2_ALPHA_ATM_1;
        case 2: return V2_ALPHA_ATM_2;
        case 3: return V2_ALPHA_ATM_3;
        case 4: return V2_ALPHA_ATM_4;
        case 5: return V2_ALPHA_ATM_5;
        case 6: return V2_ALPHA_ATM_6;
        default: return V2_ALPHA_ATM_7;
    }
}

QM_FN int qm_h0_iterator_init(qm_h0_iterator *iterator, qm_metric_vector start_m,
                              qm_metric_vector end_m, double emission_per_m,
                              double d_floor_m) {
    iterator->fault_bits = 0u;
    iterator->emitted = 0;
    iterator->run_index = 0;
    iterator->cell_index = 0;
    iterator->cell_count = 0;
    iterator->cell_ready = 0;
    if (!qm_vector_is_finite(start_m) || !qm_vector_is_finite(end_m) ||
        !isfinite(emission_per_m) || !isfinite(d_floor_m) || !(d_floor_m > 0.0)) {
        iterator->fault_bits = QM_H0_FAULT_NONFINITE;
        return 0;
    }

    // H0-FRAME-1: piece direction and length.
    const double dx = QM_SUB(end_m.x, start_m.x);
    const double dy = QM_SUB(end_m.y, start_m.y);
    const double length = SR_SQRT(QM_ADD(QM_MUL(dx, dx), QM_MUL(dy, dy)));
    if (length == 0.0) {
        iterator->fault_bits = QM_H0_FAULT_DEGENERATE_PIECE;
        return 0;
    }
    if (length > V2_LINE_MAX_LENGTH_M) {
        iterator->fault_bits = QM_H0_FAULT_OVERLENGTH_PIECE;
        return 0;
    }
    if (!isfinite(length)) {
        iterator->fault_bits = QM_H0_FAULT_NONFINITE;
        return 0;
    }
    const qm_metric_vector direction = {QM_DIV(dx, length), QM_DIV(dy, length)};
    const qm_metric_vector receiver_from_start = {
        QM_SUB(0.0, start_m.x), QM_SUB(0.0, start_m.y)};
    const double foot_from_start = QM_ADD(QM_MUL(receiver_from_start.x, direction.x),
                                          QM_MUL(receiver_from_start.y, direction.y));
    const qm_metric_vector foot = {
        QM_ADD(start_m.x, QM_MUL(foot_from_start, direction.x)),
        QM_ADD(start_m.y, QM_MUL(foot_from_start, direction.y)),
    };
    const qm_metric_vector offset = {
        QM_SUB(0.0, foot.x), QM_SUB(0.0, foot.y)};
    const double d_perp = SR_SQRT(QM_ADD(QM_MUL(offset.x, offset.x),
                                         QM_MUL(offset.y, offset.y)));
    const double h_m = d_perp > d_floor_m ? d_perp : d_floor_m;
    const double start_s = -foot_from_start;
    const double end_s = QM_SUB(length, foot_from_start);
    const double low_s = start_s < end_s ? start_s : end_s;
    const double high_s = start_s > end_s ? start_s : end_s;
    const double low_u = qm_atan(QM_DIV(low_s, h_m));
    const double high_u = qm_atan(QM_DIV(high_s, h_m));
    if (!qm_vector_is_finite(direction) || !qm_vector_is_finite(foot) ||
        !isfinite(d_perp) || !isfinite(h_m) || !isfinite(low_u) || !isfinite(high_u)) {
        iterator->fault_bits = QM_H0_FAULT_NONFINITE;
        return 0;
    }

    iterator->foot_m = foot;
    iterator->direction = direction;
    iterator->piece_length_m = length;
    iterator->foot_from_start_m = foot_from_start;
    iterator->h_m = h_m;
    iterator->emission_per_m = emission_per_m;
    iterator->low_s = low_s;
    iterator->high_s = high_s;
    iterator->low_u = low_u;
    iterator->high_u = high_u;
    iterator->bounds[0] = low_u;
    if (low_s < 0.0 && 0.0 < high_s) {
        iterator->bounds[1] = 0.0;
        iterator->bounds[2] = high_u;
        iterator->bound_count = 3;
    } else {
        iterator->bounds[1] = high_u;
        iterator->bound_count = 2;
    }
    return 1;
}

QM_FN int qm_h0_prepare_cell(qm_h0_iterator *iterator) {
    while (iterator->run_index + 1 < iterator->bound_count) {
        const double u_lo = iterator->bounds[iterator->run_index];
        const double u_hi = iterator->bounds[iterator->run_index + 1];
        // Rust sorts then deduplicates equal-u bounds before opening windows.
        // H0 has at most three already-sorted bounds, so skipping an exact
        // zero-width run is the literal bounded mirror of that authority.
        if (u_hi == u_lo) {
            iterator->run_index++;
            iterator->cell_count = 0;
            iterator->cell_index = 0;
            continue;
        }
        if (iterator->cell_count == 0) {
            const double cells = ceil(QM_DIV(QM_SUB(u_hi, u_lo), V2_THETA_MAX_RAD));
            iterator->cell_count = (int)(cells > 1.0 ? cells : 1.0);
            iterator->cell_index = 0;
        }
        if (iterator->cell_index >= iterator->cell_count) {
            iterator->run_index++;
            iterator->cell_count = 0;
            iterator->cell_index = 0;
            continue;
        }

        // H0-CELL-1: identical equal-u boundary order to LineNodeIterator.
        const double delta_u = QM_SUB(u_hi, u_lo);
        const double u0 = QM_ADD(
            u_lo,
            QM_DIV(QM_MUL(delta_u, (double)iterator->cell_index),
                   (double)iterator->cell_count));
        const double u1 = QM_ADD(
            u_lo,
            QM_DIV(QM_MUL(delta_u, (double)(iterator->cell_index + 1)),
                   (double)iterator->cell_count));
        const double s0 = iterator->cell_index == 0 && u0 == iterator->low_u
                              ? iterator->low_s
                              : QM_MUL(iterator->h_m, qm_tan(u0));
        const double s1 = iterator->cell_index + 1 == iterator->cell_count &&
                                  u1 == iterator->high_u
                              ? iterator->high_s
                              : QM_MUL(iterator->h_m, qm_tan(u1));
        const int arm = QM_MUL(QM_ADD(s0, s1), 0.5) < 0.0 ? -1 : 1;
        iterator->cell_near_s = arm < 0 ? s1 : s0;
        iterator->cell_far_s = arm < 0 ? s0 : s1;
        iterator->cell_arm = arm;
        iterator->current_s = iterator->cell_near_s;
        iterator->cell_ready = 1;
        iterator->cell_index++;
        return 1;
    }
    return 0;
}

// Returns 1 with one node, 0 at the clean end, and -1 on a hard fault.
QM_FN int qm_h0_iterator_next(qm_h0_iterator *iterator, qm_h0_node *node) {
    for (;;) {
        if (!iterator->cell_ready && !qm_h0_prepare_cell(iterator)) return 0;
        const double current_s = iterator->current_s;
        if (current_s == iterator->cell_far_s) {
            iterator->cell_ready = 0;
            continue;
        }
        if (iterator->emitted >= V2_H0_NODE_CAP) {
            iterator->fault_bits |= QM_H0_FAULT_NODE_CAP;
            return -1;
        }

        // H0-CHUNK-1: atmospheric live-range cut in the placement metric.
        const double h2 = QM_MUL(iterator->h_m, iterator->h_m);
        const double current_d = SR_SQRT(QM_ADD(h2, QM_MUL(current_s, current_s)));
        const double far_d = SR_SQRT(
            QM_ADD(h2, QM_MUL(iterator->cell_far_s, iterator->cell_far_s)));
        double alpha_live = 0.0;
        for (int band = 0; band < 8; band++) {
            const double alpha = qm_h0_alpha_atm(band);
            const double attenuation = QM_DIV(QM_MUL(alpha, current_d), 1000.0);
            if (attenuation <= V2_A_LIVE_DB && alpha > alpha_live) alpha_live = alpha;
        }
        if (alpha_live < 0.0 || !isfinite(alpha_live)) {
            iterator->fault_bits |= QM_H0_FAULT_INTERNAL;
            return -1;
        }
        const double step = QM_DIV(
            QM_MUL(QM_MUL(V2_THETA_MAX_RAD, V2_R_ATM_BASE_M_PER_RAD), V2_ALPHA_ATM_7),
            alpha_live);
        const double next_d = QM_SUB(far_d, current_d) > step ? QM_ADD(current_d, step)
                                                              : far_d;
        const double radicand = QM_SUB(QM_MUL(next_d, next_d), h2);
        if (radicand < 0.0 || !isfinite(radicand)) {
            iterator->fault_bits |= QM_H0_FAULT_NONFINITE;
            return -1;
        }
        const double next_s = next_d == far_d
                                  ? iterator->cell_far_s
                                  : QM_MUL((double)iterator->cell_arm, SR_SQRT(radicand));
        iterator->current_s = next_s;
        const double low_s = iterator->cell_arm < 0 ? next_s : current_s;
        const double high_s = iterator->cell_arm < 0 ? current_s : next_s;
        const double chunk_length = QM_SUB(high_s, low_s);
        const double chunk_delta_u = QM_SUB(qm_atan(QM_DIV(high_s, iterator->h_m)),
                                            qm_atan(QM_DIV(low_s, iterator->h_m)));
        const double d2 = QM_DIV(QM_MUL(chunk_length, iterator->h_m), chunk_delta_u);
        double node_radicand = QM_SUB(d2, h2);
        if (node_radicand < 0.0) node_radicand = 0.0;
        const double node_s = QM_MUL((double)iterator->cell_arm, SR_SQRT(node_radicand));
        const qm_metric_vector position = {
            QM_ADD(iterator->foot_m.x, QM_MUL(node_s, iterator->direction.x)),
            QM_ADD(iterator->foot_m.y, QM_MUL(node_s, iterator->direction.y)),
        };
        double node_distance = 0.0;
        if (!qm_node_horizontal_range(position, &node_distance)) {
            iterator->fault_bits |= QM_H0_FAULT_NONFINITE;
            return -1;
        }
        node->position_m = position;
        node->piece_fraction = QM_DIV(QM_ADD(iterator->foot_from_start_m, node_s),
                                      iterator->piece_length_m);
        node->weight = QM_MUL(iterator->emission_per_m, chunk_length);
        node->placement_distance_m = SR_SQRT(d2);
        node->node_x_m = node_s;
        node->node_distance_m = node_distance;
        node->arm = iterator->cell_arm;
        iterator->emitted++;
        if (!qm_vector_is_finite(node->position_m) || !isfinite(node->piece_fraction) ||
            !isfinite(node->weight) || !isfinite(node->placement_distance_m) ||
            !isfinite(node->node_x_m) || !isfinite(node->node_distance_m)) {
            iterator->fault_bits |= QM_H0_FAULT_NONFINITE;
            return -1;
        }
        return 1;
    }
}

// The sole device/host-C implementation of the production H0 candidate/node
// mask loop. Callers own candidate-store validation and visit accounting; this
// function owns every /64, %64, wedge and range decision used to paint tiles.
QM_FN int qm_h0_mask_candidate_nodes(
    qm_metric_vector candidate_a, qm_metric_vector candidate_b, float near_f32,
    const double *node_x, const double *node_distance, int node_count,
    qm_metric_vector foot_vector, qm_metric_vector line_unit,
    unsigned long long *mask, unsigned long long *guarded_candidates) {
    if (!isfinite(near_f32) || node_count < 0 || node_count > V2_H0_NODE_CAP ||
        !qm_vector_is_finite(foot_vector) || !qm_vector_is_finite(line_unit))
        return 0;
    for (int node_index = 0; node_index < node_count; node_index++) {
        // SR-5: reconstruct from the pair frame; never subtract a world-space
        // receiver a second time.
        const qm_metric_vector node_vector = {
            QM_ADD(foot_vector.x, QM_MUL(line_unit.x, node_x[node_index])),
            QM_ADD(foot_vector.y, QM_MUL(line_unit.y, node_x[node_index])),
        };
        const int wedge = qm_candidate_wedge_owns(
            candidate_a, candidate_b, node_vector, near_f32);
        if (wedge == QM_WEDGE_HARD_FAULT) return 0;
        if (wedge == QM_WEDGE_NEAR_GUARDED_DEGENERATE) {
            (*guarded_candidates)++;
            return 1;
        }
        if (wedge != QM_WEDGE_OWNS) continue;
        int hard_fault = 0;
        const int ordered =
            qm_range_ordered(node_distance[node_index], near_f32, &hard_fault);
        if (hard_fault) return 0;
        if (ordered)
            mask[node_index / 64] |= 1ull << (node_index % 64);
    }
    return 1;
}

#endif // QM_H0_NODE_GENERATOR_CUH

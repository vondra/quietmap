//! W1-only point-layer sparse receiver reconstruction (industrial, building).
//!
//! This module is deliberately opt-in through `QM_W1_INDUSTRIAL_POLICY`
//! (`adaptive-stride5`); the machinery is layer-agnostic and a building port
//! exists, but its switch is UNLISTED in the gate until that port passes the
//! drift contract — nothing can activate it. It renders a direct-local
//! surrogate over the whole tile, computes exact physics only at a stride-5
//! anchor lattice, derives the whole-block refinement mask from anchor
//! tri-state, raw-anchor residual range, and surrogate-predicted numeric
//! tri-state, then computes selected blocks exactly. The normal exact point
//! paths, line layers, popup, and W2 remain outside this module and are
//! unchanged when the switch is absent.

use noise_compute::types::Barrier;
use raster_reader::fused_tile_z13::{FusedTileZ13, TILE_PX};
use std::time::Instant;

use crate::accumulator::TileAccumulator;
use crate::scatter_point::{
    scatter_tile_point_direct, scatter_tile_point_exact_receivers, PointScatterStats,
};
use crate::source_point::PointRow;
use crate::wire_hm3::{collapse_lden_surface_u8, NO_DATA};
use noise_compute::propagation::obstacle_index::ObstacleSet;

const STRIDE: usize = 5;
const PAINT_FLOOR_BYTE: u8 = 60;

/// Runtime gate for the isolated W1 candidates. Any other value keeps the
/// exact/default producer path, including the ordinary point receiver
/// scatter of the layer in question. Each point layer carries its own switch
/// so the waves can adopt them independently.
fn policy_enabled(layer: &str) -> bool {
    // Building is ported but not yet accepted: its >6 dB tail failed the
    // contract, so its switch stays unlisted — the module cannot be activated
    // for it until that rung passes.
    let var = match layer {
        "industrial" => "QM_W1_INDUSTRIAL_POLICY",
        _ => return false,
    };
    matches!(std::env::var(var).as_deref(), Ok("adaptive-stride5"))
}

fn policy_applies_at_zoom(zoom: u8, requested: bool) -> bool {
    zoom == 12 && requested
}

/// Whether the opt-in policy may replace the producer at this zoom. The W1
/// candidate is structurally restricted to z12; z13 and every other zoom always
/// use the exact/default path even when the environment variable is present.
pub(crate) fn enabled_for_zoom(zoom: u8, layer: &str) -> bool {
    policy_applies_at_zoom(zoom, policy_enabled(layer))
}

/// Telemetry for one reconstructed tile. It is intentionally separate from
/// the ordinary point-scatter counters so a receipt can report the exact
/// receiver budget without changing existing layer statistics.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ReconstructionStats {
    pub exact_receivers: usize,
    pub total_receivers: usize,
    pub selected_blocks: usize,
    /// The numeric field already includes area median fill and building interiors.
    pub postprocess_applied: bool,
}

impl ReconstructionStats {
    fn exact_fraction(self) -> f64 {
        self.exact_receivers as f64 / self.total_receivers as f64
    }
}

/// Render one industrial tile with the adaptive stride-5 policy.
///
/// The exact receiver mask is built in two passes because the selector needs
/// the exact anchor bytes before it can decide which whole blocks to repair.
/// No reference tile, output tree, or sealed artifact is read here: every
/// selector input comes from this tile's direct surrogate and exact physics
/// calls.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render(
    layer: &str,
    tile: &FusedTileZ13,
    points: &[PointRow],
    barriers: &[Barrier],
    obstacles: &ObstacleSet,
    interior: &crate::source_loader_obstacle::InteriorEstimate,
) -> (Vec<u8>, PointScatterStats, ReconstructionStats) {
    let axis = anchor_axis();
    let anchor_mask = lattice_mask(&axis);
    let anchor_receivers = receiver_indices(&anchor_mask);

    // Phase 1: direct-local field everywhere. `Some(0.0)` is the fixed
    // surrogate arm; it never consults a sealed/reference output.
    let direct_started = Instant::now();
    let mut surrogate_accum = TileAccumulator::new();
    let direct_stats =
        scatter_tile_point_direct(tile, points, barriers, obstacles, &mut surrogate_accum);
    let direct_elapsed = direct_started.elapsed();
    let surrogate_raw = collapse_lden_surface_u8(&surrogate_accum);
    let mut surrogate_cells = surrogate_raw.clone();
    crate::wire_hm3::fill_area_median(&mut surrogate_cells, crate::wire_hm3::AREA_FILL_RADIUS_PX);
    interior.apply(&mut surrogate_cells);

    // Phase 2: exact physics only at the 104×104 anchors. These values drive
    // both the residual correction and the tri-state selector.
    let anchor_started = Instant::now();
    let mut exact_accum = TileAccumulator::new();
    let anchor_stats = scatter_tile_point_exact_receivers(
        tile,
        points,
        barriers,
        obstacles,
        &mut exact_accum,
        &anchor_receivers,
    );
    let anchor_elapsed = anchor_started.elapsed();
    let exact_anchor_cells = collapse_lden_surface_u8(&exact_accum);
    let (raw_numeric, anchor_state) = base_candidate(&surrogate_raw, &exact_anchor_cells, &axis);
    let (final_numeric, _) = base_candidate(&surrogate_cells, &exact_anchor_cells, &axis);
    // Exact anchors are sampled before display transforms. Carry their
    // raw-vs-raw residual onto the final-field surrogate instead of mixing a
    // raw exact byte with a final surrogate byte.
    let numeric = rebase_raw_residual(
        &surrogate_raw,
        &surrogate_cells,
        &raw_numeric,
        &final_numeric,
    );
    let block_flags = refinement_blocks_with_residual_range(
        &anchor_state,
        &raw_numeric,
        &surrogate_raw,
        &exact_anchor_cells,
        &axis,
    );
    // OR the source-proximity rule into the selector's block flags before the
    // mask is built — a block near a source is exact regardless of what the
    // anchors said (they cannot certify their own steep zone).
    let proximity_flags = source_proximity_block_flags(tile, points, &axis);
    let mut selector_flags = block_flags.clone();
    for (flag, near_source) in selector_flags.iter_mut().zip(proximity_flags.iter()) {
        *flag |= *near_source;
    }
    let selected_mask = block_mask(&selector_flags, &axis);

    // Façade donors must be exact: `interior.apply` rewrites every enclosed
    // pixel from its donor's value, so an interpolated donor would inject its
    // full error into every pixel it feeds — the same pathology the line
    // layers fixed by adding their donors to the exact replay set. Donors are
    // outdoor edge pixels (a small, sharply bounded set).
    let donor_mask = interior_donor_mask(interior);

    // Phase 3: exact physics for the selected whole blocks plus the donor
    // pixels. Anchors were already evaluated, so exclude them to avoid
    // double-counting energy.
    let refine_started = Instant::now();
    let mut refine_mask = selected_mask;
    for (selected, donor) in refine_mask.iter_mut().zip(donor_mask.iter()) {
        *selected |= *donor;
    }
    for (selected, anchor) in refine_mask.iter_mut().zip(anchor_mask.iter()) {
        *selected &= !*anchor;
    }
    let refine_receivers = receiver_indices(&refine_mask);
    let refine_stats = scatter_tile_point_exact_receivers(
        tile,
        points,
        barriers,
        obstacles,
        &mut exact_accum,
        &refine_receivers,
    );
    let refine_elapsed = refine_started.elapsed();
    let exact_cells = collapse_lden_surface_u8(&exact_accum);

    // Reconstruct the legacy byte-state field from the raw surrogate. Its
    // median-filled tri-state is the runtime presence contract; the new numeric
    // field below may change amplitudes but must not invent or erase presence.
    let mut legacy_presence = raw_numeric
        .iter()
        .enumerate()
        .map(|(idx, value)| quantise_candidate(*value, predicted_state(&anchor_state, &axis, idx)))
        .collect::<Vec<_>>();
    for (idx, exact) in refine_mask.iter().enumerate() {
        if *exact {
            legacy_presence[idx] = exact_cells[idx];
        }
    }
    for (idx, anchor) in anchor_mask.iter().enumerate() {
        if *anchor {
            legacy_presence[idx] = exact_anchor_cells[idx];
        }
    }
    crate::wire_hm3::fill_area_median(&mut legacy_presence, crate::wire_hm3::AREA_FILL_RADIUS_PX);
    interior.apply(&mut legacy_presence);

    // Direct + interpolated candidate everywhere, exact cells on the union
    // of anchors and selected blocks. The final-field surrogate already
    // includes area fill and interiors; exact cells receive those transforms
    // selectively below.
    let mut candidate = numeric
        .iter()
        .enumerate()
        .map(|(idx, value)| quantise_candidate(*value, predicted_state(&anchor_state, &axis, idx)))
        .collect::<Vec<_>>();
    // The exact receiver UNION — selected blocks AND anchors — pins the numeric
    // field: the residual interpolation must never speak at a pixel whose
    // exact byte is known (measured: anchors left to interpolation were the
    // >6 dB tail, one-sided to silence at sharp façade gradients).
    let mut exact_union_mask = refine_mask.clone();
    for (union_cell, anchor) in exact_union_mask.iter_mut().zip(anchor_mask.iter()) {
        *union_cell |= *anchor;
    }
    for (idx, exact) in exact_union_mask.iter().enumerate() {
        if *exact {
            candidate[idx] = if anchor_mask[idx] {
                exact_anchor_cells[idx]
            } else {
                exact_cells[idx]
            };
        }
    }
    for (candidate_cell, &presence_cell) in candidate.iter_mut().zip(legacy_presence.iter()) {
        *candidate_cell = match presence_cell {
            NO_DATA => NO_DATA,
            0..=59 => (*candidate_cell).min(presence_cell),
            _ => (*candidate_cell).max(PAINT_FLOOR_BYTE),
        };
    }
    // The numeric field was already median-filled above, so the caller must
    // not fill it again. Exact cells still need the same display smoothing as
    // the stock point path; restrict the operation to the exact receiver
    // union so it cannot perturb sparse numeric interpolation.
    fill_selected_area_median(
        &mut candidate,
        &exact_union_mask,
        crate::wire_hm3::AREA_FILL_RADIUS_PX,
    );
    apply_selected_interior(interior, &mut candidate, &exact_union_mask);
    // Median fill can cross the presence threshold or fill an exact NO_DATA
    // cell. Re-apply the preserved runtime presence state after smoothing.
    for (candidate_cell, &presence_cell) in candidate.iter_mut().zip(legacy_presence.iter()) {
        *candidate_cell = match presence_cell {
            NO_DATA => NO_DATA,
            0..=59 => (*candidate_cell).min(presence_cell),
            _ => (*candidate_cell).max(PAINT_FLOOR_BYTE),
        };
    }

    let exact_receivers = anchor_mask.iter().filter(|&&value| value).count()
        + refine_mask.iter().filter(|&&value| value).count();
    let reconstruction_stats = ReconstructionStats {
        exact_receivers,
        total_receivers: TILE_PX * TILE_PX,
        selected_blocks: selector_flags.iter().filter(|&&value| value).count(),
        postprocess_applied: true,
    };
    let stats = combine_stats(
        direct_stats,
        combine_stats(anchor_stats, refine_stats, points.len()),
        points.len(),
    );
    eprintln!(
        "[point-w1 {layer}] stride={STRIDE} exact_receivers={}/{} fraction={:.6}% selected_blocks={} \
         direct_pairs={} exact_pairs={} phase_ms={:.1}/{:.1}/{:.1}",
        reconstruction_stats.exact_receivers,
        reconstruction_stats.total_receivers,
        reconstruction_stats.exact_fraction() * 100.0,
        reconstruction_stats.selected_blocks,
        direct_stats.pairs,
        anchor_stats.pairs + refine_stats.pairs,
        direct_elapsed.as_secs_f64() * 1000.0,
        anchor_elapsed.as_secs_f64() * 1000.0,
        refine_elapsed.as_secs_f64() * 1000.0,
    );
    (candidate, stats, reconstruction_stats)
}

fn combine_stats(a: PointScatterStats, b: PointScatterStats, rows: usize) -> PointScatterStats {
    PointScatterStats {
        rows,
        path_calls: a.path_calls + b.path_calls,
        skipped_calls: a.skipped_calls + b.skipped_calls,
        pairs: a.pairs + b.pairs,
        walked_pairs: a.walked_pairs + b.walked_pairs,
        raster_samples: a.raster_samples + b.raster_samples,
    }
}

/// Apply the ordinary AREA median fill only to exact receivers.
///
/// Numeric interpolated cells already use a final-field surrogate and must
/// not be raised a second time. Exact cells are the only cells that still
/// need the stock point-path smoothing before the tile is written.
fn fill_selected_area_median(cells: &mut [u8], selected: &[bool], radius: usize) {
    debug_assert_eq!(cells.len(), TILE_PX * TILE_PX);
    debug_assert_eq!(selected.len(), TILE_PX * TILE_PX);
    let src = cells.to_vec();
    let side = 2 * radius + 1;
    let mut window = Vec::with_capacity(side * side);
    for py in 0..TILE_PX {
        let y0 = py.saturating_sub(radius);
        let y1 = (py + radius).min(TILE_PX - 1);
        for px in 0..TILE_PX {
            let idx = py * TILE_PX + px;
            if !selected[idx] {
                continue;
            }
            let x0 = px.saturating_sub(radius);
            let x1 = (px + radius).min(TILE_PX - 1);
            window.clear();
            let mut window_cells = 0usize;
            for wy in y0..=y1 {
                let row = wy * TILE_PX;
                for wx in x0..=x1 {
                    window_cells += 1;
                    let value = src[row + wx];
                    if value != NO_DATA {
                        window.push(value);
                    }
                }
            }
            if window.len() < 2 {
                continue;
            }
            window.sort_unstable();
            let median = window[window.len() / 2];
            if (src[idx] != NO_DATA && median > src[idx])
                || (src[idx] == NO_DATA && window.len() * 4 >= window_cells * 3)
            {
                cells[idx] = median;
            }
        }
    }
}

/// Apply the building-envelope transform only to exact cells. The numeric
/// surrogate has already been transformed; applying it to every cell would
/// subtract the façade delta twice from enclosed interpolation.
fn apply_selected_interior(
    interior: &crate::source_loader_obstacle::InteriorEstimate,
    cells: &mut [u8],
    selected: &[bool],
) {
    use noise_compute::envelope::EnvelopeClass;

    debug_assert_eq!(cells.len(), TILE_PX * TILE_PX);
    debug_assert_eq!(selected.len(), TILE_PX * TILE_PX);
    for (index, class) in interior.classes().iter().copied().enumerate() {
        let raw_donor = interior.donors()[index];
        let donor = if raw_donor == crate::source_loader_obstacle::NO_DONOR {
            // No reachable donor: an exact enclosed pixel mirrors the stock
            // path's NO_DATA; a non-exact one keeps its interpolated value.
            if selected[index] {
                cells[index] = NO_DATA;
            }
            continue;
        } else {
            raw_donor as usize
        };
        // Transform when the enclosed pixel is exact OR ITS DONOR is: a
        // non-exact enclosed pixel still inherits its donor's value, so an
        // exact donor must propagate through it.
        if !selected[index] && !selected[donor] {
            continue;
        }
        let Some(delta) = EnvelopeClass::from_u8(class).delta_db() else {
            continue;
        };
        let facade = crate::wire_hm3::dequantise_lden(cells[donor]);
        cells[index] = if facade.is_finite() {
            crate::wire_hm3::quantise_lden((facade - delta).max(0.0))
        } else {
            NO_DATA
        };
    }
}

fn anchor_axis() -> Vec<usize> {
    let mut axis = (0..TILE_PX).step_by(STRIDE).collect::<Vec<_>>();
    if axis.last().copied() != Some(TILE_PX - 1) {
        axis.push(TILE_PX - 1);
    }
    axis
}

fn lattice_mask(axis: &[usize]) -> Vec<bool> {
    let mut mask = vec![false; TILE_PX * TILE_PX];
    for &py in axis {
        for &px in axis {
            mask[py * TILE_PX + px] = true;
        }
    }
    mask
}

fn receiver_indices(mask: &[bool]) -> Vec<usize> {
    mask.iter()
        .enumerate()
        .filter_map(|(index, &selected)| selected.then_some(index))
        .collect()
}

/// Build the byte candidate and exact-anchor states. Residuals are bilinear
/// in HM3 byte units (equivalent to 0.5 dB units) and are interpolated only
/// when at least one corner residual is numeric, renormalizing finite weights.
fn base_candidate(surrogate: &[u8], exact_anchor: &[u8], axis: &[usize]) -> (Vec<f64>, Vec<u8>) {
    let n = axis.len();
    let mut correction = vec![0.0; TILE_PX * TILE_PX];
    let mut correction_valid = vec![false; TILE_PX * TILE_PX];
    for by in 0..n - 1 {
        let y0 = axis[by];
        let y1 = axis[by + 1];
        for bx in 0..n - 1 {
            let x0 = axis[bx];
            let x1 = axis[bx + 1];
            let corners = [
                (y0 * TILE_PX + x0, y0 * TILE_PX + x1),
                (y1 * TILE_PX + x0, y1 * TILE_PX + x1),
            ];
            let anchor_indices = [corners[0].0, corners[0].1, corners[1].0, corners[1].1];
            let residual = anchor_indices.map(|idx| {
                if surrogate[idx] == NO_DATA || exact_anchor[idx] == NO_DATA {
                    None
                } else {
                    Some(exact_anchor[idx] as f64 - surrogate[idx] as f64)
                }
            });
            let height = (y1 - y0) as f64;
            let width = (x1 - x0) as f64;
            for py in y0..=y1 {
                let fy = (py - y0) as f64 / height;
                for px in x0..=x1 {
                    let fx = (px - x0) as f64 / width;
                    // Match Python finite_bilinear: interpolate only the
                    // available corner residuals, then divide by their
                    // interpolated finite-weight denominator.
                    let weights = [
                        (1.0 - fy) * (1.0 - fx),
                        (1.0 - fy) * fx,
                        fy * (1.0 - fx),
                        fy * fx,
                    ];
                    let mut numerator = 0.0;
                    let mut denominator = 0.0;
                    for (weight, value) in weights.into_iter().zip(residual) {
                        if let Some(value) = value {
                            numerator += weight * value;
                            denominator += weight;
                        }
                    }
                    if denominator > 1e-12 {
                        let idx = py * TILE_PX + px;
                        correction[idx] = numerator / denominator;
                        correction_valid[idx] = true;
                    }
                }
            }
        }
    }

    let mut numeric = vec![0.0; TILE_PX * TILE_PX];
    for py in 0..TILE_PX {
        for px in 0..TILE_PX {
            let idx = py * TILE_PX + px;
            let target = bilinear_anchor_value(exact_anchor, axis, py, px);
            numeric[idx] = if surrogate[idx] == NO_DATA {
                target.max(0.0)
            } else {
                surrogate[idx] as f64
                    + if correction_valid[idx] {
                        correction[idx]
                    } else {
                        0.0
                    }
            };
        }
    }
    let anchor_state = axis
        .iter()
        .flat_map(|&py| {
            axis.iter().map(move |&px| {
                let value = exact_anchor[py * TILE_PX + px];
                if value == NO_DATA {
                    0
                } else if value < PAINT_FLOOR_BYTE {
                    1
                } else {
                    2
                }
            })
        })
        .collect();
    (numeric, anchor_state)
}

fn rebase_raw_residual(
    raw_surrogate: &[u8],
    final_surrogate: &[u8],
    raw_numeric: &[f64],
    final_numeric: &[f64],
) -> Vec<f64> {
    raw_surrogate
        .iter()
        .zip(final_surrogate)
        .zip(raw_numeric)
        .zip(final_numeric)
        .map(|(((raw_byte, final_byte), raw_value), final_value)| {
            if *raw_byte == NO_DATA || *final_byte == NO_DATA {
                *final_value
            } else {
                *final_byte as f64 + (*raw_value - *raw_byte as f64)
            }
        })
        .collect()
}

fn refinement_blocks_with_residual_range(
    anchor_state: &[u8],
    numeric: &[f64],
    surrogate: &[u8],
    exact_anchor: &[u8],
    axis: &[usize],
) -> Vec<bool> {
    let mut flags = refinement_blocks(anchor_state, numeric, axis);
    let blocks = axis.len() - 1;
    for by in 0..blocks {
        for bx in 0..blocks {
            let corners = [
                (axis[by], axis[bx]),
                (axis[by], axis[bx + 1]),
                (axis[by + 1], axis[bx]),
                (axis[by + 1], axis[bx + 1]),
            ];
            let mut min_residual = f64::INFINITY;
            let mut max_residual = f64::NEG_INFINITY;
            for (py, px) in corners {
                let index = py * TILE_PX + px;
                let residual = if surrogate[index] == NO_DATA || exact_anchor[index] == NO_DATA {
                    0.0
                } else {
                    exact_anchor[index] as f64 - surrogate[index] as f64
                };
                min_residual = min_residual.min(residual);
                max_residual = max_residual.max(residual);
            }
            // 16 HM3 bytes = 8 dB: threshold from the implementable sparse
            // sweep, priced only from raw anchors.
            if max_residual - min_residual > 16.0 {
                flags[by * blocks + bx] = true;
            }
        }
    }
    flags
}

fn bilinear_anchor_value(cells: &[u8], axis: &[usize], py: usize, px: usize) -> f64 {
    let (iy, fy) = interpolation_axis(axis, py);
    let (ix, fx) = interpolation_axis(axis, px);
    let y0 = axis[iy];
    let y1 = axis[iy + 1];
    let x0 = axis[ix];
    let x1 = axis[ix + 1];
    let value = |y: usize, x: usize| {
        let byte = cells[y * TILE_PX + x];
        if byte == NO_DATA {
            -10.0
        } else {
            byte as f64
        }
    };
    let top = value(y0, x0) + (value(y0, x1) - value(y0, x0)) * fx;
    let bottom = value(y1, x0) + (value(y1, x1) - value(y1, x0)) * fx;
    top + (bottom - top) * fy
}

fn interpolation_axis(axis: &[usize], coordinate: usize) -> (usize, f64) {
    if coordinate == *axis.last().expect("anchor axis is non-empty") {
        return (axis.len() - 2, 1.0);
    }
    let upper = axis.partition_point(|&value| value <= coordinate);
    let lower = upper.saturating_sub(1);
    let span = (axis[upper] - axis[lower]) as f64;
    (lower, (coordinate - axis[lower]) as f64 / span)
}

fn predicted_state(anchor_state: &[u8], axis: &[usize], pixel: usize) -> u8 {
    let py = pixel / TILE_PX;
    let px = pixel % TILE_PX;
    let iy = nearest_axis_index(axis, py);
    let ix = nearest_axis_index(axis, px);
    anchor_state[iy * axis.len() + ix]
}

fn nearest_axis_index(axis: &[usize], coordinate: usize) -> usize {
    let upper = axis
        .partition_point(|&value| value < coordinate)
        .min(axis.len() - 1);
    let lower = upper.saturating_sub(1);
    if coordinate - axis[lower] > axis[upper] - coordinate {
        upper
    } else {
        lower
    }
}

fn quantise_candidate(value: f64, state: u8) -> u8 {
    if state == 0 {
        return NO_DATA;
    }
    let rounded = value.round().clamp(0.0, 254.0) as u8;
    match state {
        1 => rounded.min(PAINT_FLOOR_BYTE - 1),
        _ => rounded.max(PAINT_FLOOR_BYTE),
    }
}

fn refinement_blocks(anchor_state: &[u8], numeric: &[f64], axis: &[usize]) -> Vec<bool> {
    let n = axis.len();
    let mut flags = vec![false; (n - 1) * (n - 1)];
    for by in 0..n - 1 {
        for bx in 0..n - 1 {
            let top_left = anchor_state[by * n + bx];
            let top_right = anchor_state[by * n + bx + 1];
            let bottom_left = anchor_state[(by + 1) * n + bx];
            let bottom_right = anchor_state[(by + 1) * n + bx + 1];
            let mixed_anchor = [top_left, top_right, bottom_left, bottom_right]
                .into_iter()
                .min()
                != [top_left, top_right, bottom_left, bottom_right]
                    .into_iter()
                    .max();
            let mut min_state = 2u8;
            let mut max_state = 0u8;
            for py in axis[by]..=axis[by + 1] {
                for px in axis[bx]..=axis[bx + 1] {
                    let value = numeric[py * TILE_PX + px];
                    let state = if value < 0.0 {
                        0
                    } else if value < PAINT_FLOOR_BYTE as f64 {
                        1
                    } else {
                        2
                    };
                    min_state = min_state.min(state);
                    max_state = max_state.max(state);
                }
            }
            flags[by * (n - 1) + bx] = mixed_anchor || min_state != max_state;
        }
    }
    flags
}

fn block_mask(flags: &[bool], axis: &[usize]) -> Vec<bool> {
    let blocks = axis.len() - 1;
    let mut mask = vec![false; TILE_PX * TILE_PX];
    for by in 0..blocks {
        for bx in 0..blocks {
            if !flags[by * blocks + bx] {
                continue;
            }
            for py in axis[by]..=axis[by + 1] {
                for px in axis[bx]..=axis[bx + 1] {
                    mask[py * TILE_PX + px] = true;
                }
            }
        }
    }
    mask
}

/// Whole stride-blocks near any point source are always exact. Façade fields
/// change by many dB per pixel next to their own sources, so no anchor
/// interpolation is admissible there regardless of how benign the anchors
/// look — the same principle as the line layers' selected exact tail (a
/// source-adjacent block cannot be certified from outside). The radius scales
/// with the source's own footprint (`exclusion_radius_m`, the self-screening
/// disc): a large building's steep zone spans its footprint, not a fixed few
/// pixels, plus a fixed margin so small façades are covered too.
/// Mask of every pixel that is the façade donor of at least one enclosed
/// pixel in `interior` — the exact set `interior.apply` reads its values from.
fn interior_donor_mask(interior: &crate::source_loader_obstacle::InteriorEstimate) -> Vec<bool> {
    let mut mask = vec![false; TILE_PX * TILE_PX];
    for &donor in interior.donors() {
        if donor != crate::source_loader_obstacle::NO_DONOR {
            mask[donor as usize] = true;
        }
    }
    mask
}

fn source_proximity_block_flags(
    tile: &FusedTileZ13,
    points: &[PointRow],
    axis: &[usize],
) -> Vec<bool> {
    // The margin must also cover the area-fill median window
    // (`AREA_FILL_RADIUS_PX`, 3 px): a selected block whose smoothing halo is
    // still surrogate would mix surrogate neighbours into the median where the
    // stock path mixes exact ones.
    const SOURCE_PROXIMITY_MARGIN_PX: f64 = 4.0 + crate::wire_hm3::AREA_FILL_RADIUS_PX as f64;
    const SOURCE_PROXIMITY_MAX_PX: f64 = 64.0;
    let blocks = axis.len() - 1;
    let mut flags = vec![false; blocks * blocks];
    let m_per_px = (tile.bbox.north_lat - tile.bbox.south_lat) * 111_320.0 / TILE_PX as f64;
    for point in points {
        let radius_px = ((point.exclusion_radius_m / m_per_px) + SOURCE_PROXIMITY_MARGIN_PX)
            .clamp(0.0, SOURCE_PROXIMITY_MAX_PX);
        debug_assert!(
            radius_px.is_finite(),
            "degenerate tile bbox: non-finite metres-per-pixel"
        );
        let px = crate::scatter_band::lon_to_px(&tile.bbox, point.lon) as f64;
        let py = crate::scatter_band::lat_to_py(&tile.bbox, point.lat) as f64;
        for by in 0..blocks {
            let by_lo = axis[by] as f64;
            let by_hi = axis[by + 1] as f64;
            if py + radius_px < by_lo || py - radius_px > by_hi {
                continue;
            }
            for bx in 0..blocks {
                let bx_lo = axis[bx] as f64;
                let bx_hi = axis[bx + 1] as f64;
                if px + radius_px < bx_lo || px - radius_px > bx_hi {
                    continue;
                }
                flags[by * blocks + bx] = true;
            }
        }
    }
    flags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_is_structurally_w1_z12_only() {
        assert!(policy_applies_at_zoom(12, true));
        assert!(!policy_applies_at_zoom(13, true));
        assert!(!policy_applies_at_zoom(11, true));
        assert!(!policy_applies_at_zoom(12, false));
    }

    #[test]
    fn stride_five_axis_and_mask_have_canonical_boundaries() {
        let axis = anchor_axis();
        assert_eq!(axis.len(), 104);
        assert_eq!(axis[0], 0);
        assert_eq!(axis[axis.len() - 2], 510);
        assert_eq!(axis[axis.len() - 1], 511);
        let mask = lattice_mask(&axis);
        assert_eq!(mask.iter().filter(|&&set| set).count(), 104 * 104);
        assert!(mask[0]);
        assert!(mask[511 * TILE_PX + 511]);
        assert!(!mask[1]);
    }

    #[test]
    fn mixed_anchor_state_selects_only_the_inclusive_block() {
        let axis = vec![0, 5, 511];
        let mut states = vec![1u8; axis.len() * axis.len()];
        states[0] = 0;
        let numeric = vec![20.0; TILE_PX * TILE_PX];
        let flags = refinement_blocks(&states, &numeric, &axis);
        assert!(flags[0]);
        assert!(!flags[1]);
        assert!(!flags[2]);
        assert!(!flags[3]);
        let mask = block_mask(&flags, &axis);
        assert!(mask[0]);
        assert!(mask[5 * TILE_PX + 5]);
        assert!(!mask[6 * TILE_PX + 6]);
    }

    #[test]
    fn numeric_mixed_state_selects_without_exact_non_anchor_values() {
        let axis = vec![0, 5, 511];
        let states = vec![1u8; axis.len() * axis.len()];
        let mut numeric = vec![20.0; TILE_PX * TILE_PX];
        numeric[2 * TILE_PX + 2] = 60.0;
        let flags = refinement_blocks(&states, &numeric, &axis);
        assert!(flags[0]);
        assert!(!flags[1]);
    }
    #[test]
    fn raw_residual_rebase_keeps_final_surrogate_base() {
        let raw_surrogate = [10, NO_DATA];
        let final_surrogate = [15, 20];
        let raw_numeric = [13.5, 7.0];
        let final_numeric = [18.0, 22.0];
        let rebased = rebase_raw_residual(
            &raw_surrogate,
            &final_surrogate,
            &raw_numeric,
            &final_numeric,
        );
        assert_eq!(rebased, vec![18.5, 22.0]);
    }

    #[test]
    fn residual_range_selects_only_the_high_variation_block() {
        let axis = vec![0, 5, 511];
        let states = vec![1u8; axis.len() * axis.len()];
        let numeric = vec![20.0; TILE_PX * TILE_PX];
        let surrogate = vec![20u8; TILE_PX * TILE_PX];
        let mut exact = surrogate.clone();
        exact[0] = 40;
        let flags =
            refinement_blocks_with_residual_range(&states, &numeric, &surrogate, &exact, &axis);
        assert!(flags[0]);
        assert!(!flags[1]);
        assert!(!flags[2]);
        assert!(!flags[3]);
    }

    #[test]
    fn finite_corner_fixture_matches_python_candidate_and_mask() {
        let axis = anchor_axis();
        let mut surrogate = vec![NO_DATA; TILE_PX * TILE_PX];
        let mut exact = vec![NO_DATA; TILE_PX * TILE_PX];
        surrogate[0] = 10;
        exact[0] = 14;
        surrogate[5] = 20;
        surrogate[5 * TILE_PX] = 30;
        exact[5 * TILE_PX] = 38;
        surrogate[5 * TILE_PX + 5] = 40;
        exact[5 * TILE_PX + 5] = 52;
        let interior = 2 * TILE_PX + 2;
        surrogate[interior] = 100;

        let (numeric, states) = base_candidate(&surrogate, &exact, &axis);
        let expected = 100.0 + (0.36 * 4.0 + 0.24 * 8.0 + 0.16 * 12.0) / 0.76;
        assert!((numeric[interior] - expected).abs() < 1e-9);
        assert_eq!(
            quantise_candidate(numeric[interior], predicted_state(&states, &axis, interior)),
            59
        );

        let flags = refinement_blocks(&states, &numeric, &axis);
        let mask = block_mask(&flags, &axis);
        assert!(mask[interior]);
        assert!(mask[2 * TILE_PX + 6]);
        assert!(!mask[6 * TILE_PX + 12]);
    }
}

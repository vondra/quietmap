//! One road or rail piece at the popup receiver: the line quadrature's nodes, each on its own ray,
//! summed into per-period, per-variant band transfers, plus the trace of its loudest node.

use crate::propagation::line_quadrature::{
    line_quadrature_nodes, LineQuadratureNode, LinePieceGeometry, ReceiverSkylineArc,
};
use crate::propagation::obstacle_index::ObstacleSet;
use crate::propagation::ray_transfer::{
    evaluate_ray_transfer, RayDetail, RayReceiver, RayScratch, RaySource, VariantBands,
    VARIANT_FULL, VARIANT_NO_SCREENING, VARIANT_NO_TERRAIN,
};
use crate::types::{RasterSampler, ScreeningFanIntervalTrace, ScreeningFanTrace, NUM_BANDS};

/// The piece as prepared: endpoints, the source height above its ground, and a bridge flag.
pub struct LinePiece {
    pub start_lat: f64,
    pub start_lon: f64,
    pub end_lat: f64,
    pub end_lon: f64,
    pub source_height_m: f64,
    pub on_bridge: bool,
}

/// Received energy per unit `10^(L_W′/10)` for each period, variant and band: the quadrature sum
/// `Σ Δφ·T / (10^1.1·d⊥)`. Multiply by the emission (and A-weighting) to get band energies.
pub struct LinePieceTransfer {
    pub periods: [VariantBands; 3],
    /// The loudest node's ray in full, when a trace was asked for.
    pub loudest_node: Option<RayDetail>,
    /// Every node as a fan slice, when a trace was asked for.
    pub fan: Option<ScreeningFanTrace>,
}

/// Per-thread buffers of [`evaluate_line_piece`].
#[derive(Default)]
pub struct LinePieceScratch {
    nodes: Vec<LineQuadratureNode>,
    ray: RayScratch,
}

/// Evaluates `piece` at `receiver`. `loudness_weights` rank the nodes for the trace (the day
/// emission, A-weighted, linear); `None` when no trace is wanted. `None` for a piece of zero
/// length.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_line_piece(
    receiver: &RayReceiver,
    piece: &LinePiece,
    cp_lat: f64,
    cp_lon: f64,
    obstacles: &ObstacleSet,
    rasters: &dyn RasterSampler,
    scratch: &mut LinePieceScratch,
    loudness_weights: Option<&[f64; NUM_BANDS]>,
) -> Option<LinePieceTransfer> {
    let m_per_deg_lon = grid::geo::m_per_deg_lon(receiver.lat.to_radians());
    let local = |lat: f64, lon: f64, altitude: f64| {
        [
            grid::geo::wrapped_longitude_delta(receiver.lon, lon) * m_per_deg_lon,
            (lat - receiver.lat) * grid::geo::M_PER_DEG_LAT,
            altitude - receiver.altitude_m,
        ]
    };
    let start_altitude = rasters.elevation(piece.start_lat, piece.start_lon) + piece.source_height_m;
    let end_altitude = rasters.elevation(piece.end_lat, piece.end_lon) + piece.source_height_m;
    let geometry = LinePieceGeometry::new(
        local(piece.start_lat, piece.start_lon, start_altitude),
        local(piece.end_lat, piece.end_lon, end_altitude),
    )?;
    let mut skyline = |lo: f64, hi: f64, radius: f64, visit: &mut dyn FnMut(ReceiverSkylineArc)| {
        obstacles.skyline_arcs_within(
            receiver.lat,
            receiver.lon,
            0.0,
            radius,
            piece.source_height_m.max(0.0),
            0.0,
            Some((lo, hi)),
            None,
            &mut |arc| {
                visit(ReceiverSkylineArc {
                    lo_rad: arc.lo,
                    hi_rad: arc.hi,
                    nearest_m: f64::from(arc.near_m),
                })
            },
        );
    };
    line_quadrature_nodes(&geometry, &mut skyline, &mut scratch.nodes);
    let divergence = geometry.divergence_factor();
    let longitude_span = grid::geo::wrapped_longitude_delta(piece.start_lon, piece.end_lon);
    let source_at = |along_m: f64| {
        let fraction = along_m / geometry.length_m();
        RaySource {
            lat: piece.start_lat + fraction * (piece.end_lat - piece.start_lat),
            lon: grid::geo::normalize_longitude(piece.start_lon + fraction * longitude_span),
            height_m: piece.source_height_m,
            on_bridge: piece.on_bridge,
            exclusion_radius_m: 0.0,
        }
    };
    let mut periods = [[[0.0; NUM_BANDS]; crate::propagation::ray_transfer::VARIANT_COUNT]; 3];
    let mut loudest: Option<(f64, usize)> = None;
    let mut slices = Vec::new();
    for (index, node) in scratch.nodes.iter().enumerate() {
        let source = source_at(node.along_m);
        let transfer = evaluate_ray_transfer(
            receiver,
            &source,
            obstacles,
            node.obstacles_on_ray,
            rasters,
            &mut scratch.ray,
            None,
        );
        let weight = node.weight_rad * divergence;
        for (period, sum) in periods.iter_mut().enumerate() {
            for (variant, bands) in sum.iter_mut().enumerate() {
                for (band, value) in bands.iter_mut().enumerate() {
                    *value += weight * transfer.periods[period][variant][band];
                }
            }
        }
        if let Some(weights) = loudness_weights {
            let full = &transfer.periods[0][VARIANT_FULL];
            let loudness: f64 = (0..NUM_BANDS).map(|b| weights[b] * weight * full[b]).sum();
            if loudest.is_none_or(|(best, _)| loudness > best) {
                loudest = Some((loudness, index));
            }
            let t = &transfer.periods[0];
            let ratio_db = |variant: usize| 10.0 * (t[variant][4] / t[VARIANT_FULL][4]).log10();
            slices.push((*node, ratio_db(VARIANT_NO_TERRAIN), ratio_db(VARIANT_NO_SCREENING)));
        }
    }
    let (loudest_node, fan) = match loudest {
        None => (None, None),
        Some((_, index)) => {
            let mut detail = None;
            let source = source_at(scratch.nodes[index].along_m);
            evaluate_ray_transfer(
                receiver,
                &source,
                obstacles,
                scratch.nodes[index].obstacles_on_ray,
                rasters,
                &mut scratch.ray,
                Some(&mut detail),
            );
            let cp = local(cp_lat, cp_lon, receiver.altitude_m);
            let cp_azimuth = cp[1].atan2(cp[0]);
            (detail, Some(fan_trace(&geometry, &slices, index, cp_azimuth)))
        }
    };
    Some(LinePieceTransfer {
        periods,
        loudest_node,
        fan,
    })
}

/// The nodes as the popup fan: horizontal azimuth offsets from the closest point's ray, and each
/// node's 1 kHz terrain and screening effect (the level its removal would add).
fn fan_trace(
    geometry: &LinePieceGeometry,
    slices: &[(LineQuadratureNode, f64, f64)],
    loudest: usize,
    cp_azimuth: f64,
) -> ScreeningFanTrace {
    use crate::propagation::obstacle_index::wrap_pi as wrap_to_pi;
    let offset = |along: f64| wrap_to_pi(geometry.azimuth_at(along) - cp_azimuth).to_degrees();
    let total: f64 = slices.iter().map(|(node, _, _)| node.weight_rad).sum();
    let mut blocked = 0.0;
    let intervals = slices
        .iter()
        .enumerate()
        .map(|(index, (node, terrain_db, screen_db))| {
            let (a, b) = (offset(node.along_lo_m), offset(node.along_hi_m));
            let is_blocked = node.obstacles_on_ray && *screen_db > 0.0;
            if is_blocked {
                blocked += node.weight_rad;
            }
            ScreeningFanIntervalTrace {
                from_deg: a.min(b),
                to_deg: a.max(b),
                blocked: is_blocked,
                obstacle: None,
                terrain_db: terrain_db.max(0.0),
                screen_db: screen_db.max(0.0),
                contains_cp: index == loudest,
            }
        })
        .collect();
    ScreeningFanTrace {
        span_deg: (offset(geometry.length_m()) - offset(0.0)).abs(),
        blocked_fraction: if total > 0.0 { blocked / total } else { 0.0 },
        intervals,
        intervals_omitted: 0,
        omitted_fraction: 0.0,
        quadrature: "line_point_sum",
    }
}

//! CNOSSOS-EU point sum of one straight line piece (Directive 2015/996 §2.5.3): quadrature nodes
//! uniform in the in-plane angle, each evaluated on its own ray; the one line rule of popup and
//! painter.
//!
//! For a straight piece and a receiver R, `dx/r² = dφ/d⊥` with φ the angle in the plane that holds
//! the line and R and d⊥ the 3D distance from R to the line, so the incoherent point sum of
//! `A_div = 20·lg r + 11` is exactly `E = W′/(10^1.1·d⊥)·∫ 10^(−A_path(φ)/10) dφ`. The integral is
//! taken with [`LINE_BUCKET_COUNT`] buckets of equal Δφ, each node on its own ray. A bucket that
//! spans more than [`WIDE_BUCKET_MIN_AZIMUTH_SPAN_RAD`] of horizontal azimuth replaces its node by
//! geometry-placed nodes: the obstacle edges standing in front of the piece mark a blocked mask
//! over the bucket's azimuths, and every blocked run and clear gap gets its own nodes, each
//! weighted by its own Δφ (uniform nodes alias against building rows at any affordable count).

use super::obstacle_index::wrap_pi as wrap_to_pi;
use std::f64::consts::PI;

/// Buckets per line piece. Five on a 250 m piece match a 401-node point sum within ±0.094 dB over
/// G = 0, 0.5, 1 and 5–500 m (research line-quadrature measurement, 2026-09-24), where the
/// closest-point ray applied to the whole piece is up to +1.75 dB loud.
pub const LINE_BUCKET_COUNT: usize = 5;
/// A bucket wider than this in horizontal azimuth gets geometry-placed nodes. Measured
/// 2026-08-08 on the eleven screening-fixture scenes: at 3° the worst receiver sits on the
/// 0.85 dB plateau that finer gates do not lower, at 5° it exceeds 1 dB. Spelled
/// `to_radians` because the CUDA build reads the degree literal.
pub const WIDE_BUCKET_MIN_AZIMUTH_SPAN_RAD: f64 = 3.0_f64.to_radians();
/// Bins of the blocked-direction mask over one wide bucket.
pub const WIDE_BUCKET_MASK_BINS: usize = 128;
/// Widest azimuth one geometry-placed node may stand for, and the most nodes one run may take
/// (from the arc-screening rule these nodes replace, 2026-08-05).
pub const WIDE_BUCKET_PART_MAX_SPAN_RAD: f64 = 0.26;
pub const WIDE_BUCKET_MAX_PARTS_PER_RUN: usize = 9;
/// Receiver-to-line perpendicular floor (m): a receiver on the line itself is evaluated as a
/// line this far away, exact for a line at that distance.
pub const LINE_PERPENDICULAR_FLOOR_M: f64 = 0.5;
/// `10^1.1`: the point divergence `20·lg r + 11` of (2.5.12) folded into the line integral.
pub const POINT_DIVERGENCE_LINEAR: f64 = 12.589_254_117_941_673;

/// How a line source radiates around its own direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineDirectivity {
    Omnidirectional,
    /// The CNOSSOS-EU rail horizontal directivity (2.3.15), `10·lg(0.01 + 0.99·sin²ψ)` with ψ the
    /// angle between the track and the ray (W4's D1 source input). With φ the in-plane angle
    /// from the perpendicular foot, sin²ψ = cos²φ, so a node's weight is its closed-form integral.
    TrackDipole,
}

impl LineDirectivity {
    /// `∫ directivity dφ` over the in-plane angles between `a` and `b`: a node's weight.
    pub fn weight(self, a: f64, b: f64) -> f64 {
        let (lo, hi) = (a.min(b), a.max(b));
        match self {
            LineDirectivity::Omnidirectional => hi - lo,
            LineDirectivity::TrackDipole => {
                0.01 * (hi - lo) + 0.99 * (0.5 * (hi - lo) + 0.25 * ((2.0 * hi).sin() - (2.0 * lo).sin()))
            }
        }
    }

    /// The directivity of one ray whose source point sees the receiver at `sin²ψ` to the line.
    pub fn factor(self, sin_squared_to_line: f64) -> f64 {
        match self {
            LineDirectivity::Omnidirectional => 1.0,
            LineDirectivity::TrackDipole => 0.01 + 0.99 * sin_squared_to_line,
        }
    }
}

/// A straight piece in a receiver-centred frame: x east, y north, z altitude above the receiver,
/// all metres. Source altitudes at the two ends make it a 3D line (#28: the in-plane angle, not
/// the horizontal one, carries the finite-line term).
#[derive(Debug, Clone, Copy)]
pub struct LinePieceGeometry {
    start: [f64; 3],
    unit: [f64; 3],
    length_m: f64,
    foot_along_m: f64,
    perpendicular_m: f64,
    start_angle_rad: f64,
    end_angle_rad: f64,
}

/// One quadrature node: the position along the piece of its ray's source point, the stretch
/// `[along_lo_m, along_hi_m]` it stands for, its weight (Δφ, or the directivity's integral over
/// it), and whether vector obstacles can stand on its ray (false inside a clear gap of the mask).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineQuadratureNode {
    pub along_m: f64,
    pub along_lo_m: f64,
    pub along_hi_m: f64,
    pub weight_rad: f64,
    pub obstacles_on_ray: bool,
}

/// The obstacle query of a wide bucket: `(wedge_lo, wedge_hi, radius_m, visit)` hands every
/// edge within `radius_m` of the receiver whose arc may meet the wedge to `visit`.
pub type ReceiverSkylineQuery<'a> = dyn FnMut(f64, f64, f64, &mut dyn FnMut(ReceiverSkylineArc)) + 'a;

/// One obstacle edge seen from the receiver: its short azimuth arc `[lo, hi]` (radians,
/// `atan2(north, east)`) and its nearest horizontal range.
#[derive(Debug, Clone, Copy)]
pub struct ReceiverSkylineArc {
    pub lo_rad: f64,
    pub hi_rad: f64,
    pub nearest_m: f64,
}

impl LinePieceGeometry {
    /// `None` for a piece shorter than a millimetre, which radiates nothing.
    pub fn new(start: [f64; 3], end: [f64; 3]) -> Option<Self> {
        let along = [end[0] - start[0], end[1] - start[1], end[2] - start[2]];
        let length_m = (along[0] * along[0] + along[1] * along[1] + along[2] * along[2]).sqrt();
        if length_m < 1e-3 {
            return None;
        }
        let unit = [along[0] / length_m, along[1] / length_m, along[2] / length_m];
        // The receiver is the origin: the foot of the perpendicular from it to the line.
        let foot_along_m = -(start[0] * unit[0] + start[1] * unit[1] + start[2] * unit[2]);
        let foot = [
            start[0] + foot_along_m * unit[0],
            start[1] + foot_along_m * unit[1],
            start[2] + foot_along_m * unit[2],
        ];
        let perpendicular_m = (foot[0] * foot[0] + foot[1] * foot[1] + foot[2] * foot[2])
            .sqrt()
            .max(LINE_PERPENDICULAR_FLOOR_M);
        Some(Self {
            start,
            unit,
            length_m,
            foot_along_m,
            perpendicular_m,
            start_angle_rad: ((0.0 - foot_along_m) / perpendicular_m).atan(),
            end_angle_rad: ((length_m - foot_along_m) / perpendicular_m).atan(),
        })
    }

    pub fn length_m(&self) -> f64 {
        self.length_m
    }

    pub fn perpendicular_m(&self) -> f64 {
        self.perpendicular_m
    }

    /// θ of the free-field line level `L_W′ + 10·lg θ − 10·lg d⊥ − 11`.
    pub fn subtended_angle_rad(&self) -> f64 {
        self.end_angle_rad - self.start_angle_rad
    }

    /// `1/(10^1.1·d⊥)`: multiplies Σ Δφ·10^(−A_path/10) into received energy per unit L_W′.
    pub fn divergence_factor(&self) -> f64 {
        1.0 / (POINT_DIVERGENCE_LINEAR * self.perpendicular_m)
    }

    pub fn in_plane_angle_at(&self, along_m: f64) -> f64 {
        ((along_m - self.foot_along_m) / self.perpendicular_m).atan()
    }

    pub fn along_at_in_plane_angle(&self, angle_rad: f64) -> f64 {
        (self.foot_along_m + self.perpendicular_m * angle_rad.tan()).clamp(0.0, self.length_m)
    }

    /// The 3D point at `along_m`, receiver-relative.
    pub fn point_at(&self, along_m: f64) -> [f64; 3] {
        [
            self.start[0] + along_m * self.unit[0],
            self.start[1] + along_m * self.unit[1],
            self.start[2] + along_m * self.unit[2],
        ]
    }

    /// Where the horizontal ray from the receiver at `azimuth_rad` meets the piece's ground
    /// track, clamped to the piece; `None` for a ray parallel to it.
    pub fn along_at_azimuth(&self, azimuth_rad: f64) -> Option<f64> {
        let (dx, dy) = (azimuth_rad.cos(), azimuth_rad.sin());
        let (ux, uy) = (self.unit[0] * self.length_m, self.unit[1] * self.length_m);
        let denominator = dx * uy - dy * ux;
        if denominator.abs() < 1e-12 {
            return None;
        }
        let fraction = (dy * self.start[0] - dx * self.start[1]) / denominator;
        Some(fraction.clamp(0.0, 1.0) * self.length_m)
    }

    pub fn azimuth_at(&self, along_m: f64) -> f64 {
        let p = self.point_at(along_m);
        p[1].atan2(p[0])
    }

    fn horizontal_range_at(&self, along_m: f64) -> f64 {
        let p = self.point_at(along_m);
        p[0].hypot(p[1])
    }
}

/// The quadrature nodes of one piece, weighted by `directivity`; `skyline` is asked only for wide
/// buckets.
pub fn line_quadrature_nodes(
    geometry: &LinePieceGeometry,
    directivity: LineDirectivity,
    skyline: &mut ReceiverSkylineQuery<'_>,
    nodes: &mut Vec<LineQuadratureNode>,
) {
    nodes.clear();
    let bucket_angle = geometry.subtended_angle_rad() / LINE_BUCKET_COUNT as f64;
    for bucket in 0..LINE_BUCKET_COUNT {
        let angle_lo = geometry.start_angle_rad + bucket as f64 * bucket_angle;
        let angle_hi = angle_lo + bucket_angle;
        let along_lo = geometry.along_at_in_plane_angle(angle_lo);
        let along_hi = geometry.along_at_in_plane_angle(angle_hi);
        let centre = LineQuadratureNode {
            along_m: geometry.along_at_in_plane_angle(angle_lo + 0.5 * bucket_angle),
            along_lo_m: along_lo,
            along_hi_m: along_hi,
            weight_rad: directivity.weight(angle_lo, angle_hi),
            obstacles_on_ray: true,
        };
        let before = nodes.len();
        push_wide_bucket_nodes(geometry, directivity, along_lo, along_hi, angle_lo, angle_hi, skyline, nodes);
        if nodes.len() == before {
            nodes.push(centre);
        }
    }
}

/// Geometry-placed nodes of one bucket, or nothing when the bucket keeps its centre node
/// (narrow, degenerate, or nothing in front of it).
#[allow(clippy::too_many_arguments)]
fn push_wide_bucket_nodes(
    geometry: &LinePieceGeometry,
    directivity: LineDirectivity,
    along_lo: f64,
    along_hi: f64,
    angle_lo: f64,
    angle_hi: f64,
    skyline: &mut ReceiverSkylineQuery<'_>,
    nodes: &mut Vec<LineQuadratureNode>,
) {
    let azimuth_a = geometry.azimuth_at(along_lo);
    let turn = wrap_to_pi(geometry.azimuth_at(along_hi) - azimuth_a);
    let span = turn.abs();
    if span < WIDE_BUCKET_MIN_AZIMUTH_SPAN_RAD {
        return;
    }
    let (span_lo, span_hi) = if turn < 0.0 {
        (azimuth_a + turn, azimuth_a)
    } else {
        (azimuth_a, azimuth_a + turn)
    };
    let (range_lo, range_hi) = (
        geometry.horizontal_range_at(along_lo),
        geometry.horizontal_range_at(along_hi),
    );
    let chord = {
        let (a, b) = (geometry.point_at(along_lo), geometry.point_at(along_hi));
        (a[0] - b[0]).hypot(a[1] - b[1])
    };
    let centre_range =
        geometry.horizontal_range_at(geometry.along_at_in_plane_angle(0.5 * (angle_lo + angle_hi)));
    let need_radius = range_lo.min(range_hi).min(centre_range) + chord;
    let bin_width = span / WIDE_BUCKET_MASK_BINS as f64;
    let mut blocked = [false; WIDE_BUCKET_MASK_BINS];
    skyline(span_lo, span_hi, need_radius, &mut |arc| {
        mark_blocked_bins(geometry, arc, need_radius, span_lo, span_hi, bin_width, &mut blocked);
    });
    if !blocked.iter().any(|&b| b) {
        return;
    }
    // In-plane angle at a mask azimuth, pinned to the bucket's own ends so the node weights
    // add up to exactly the bucket's Δφ.
    let angle_at_azimuth = |azimuth: f64, fallback: f64| -> f64 {
        geometry
            .along_at_azimuth(azimuth)
            .map(|along| geometry.in_plane_angle_at(along.clamp(along_lo.min(along_hi), along_lo.max(along_hi))))
            .unwrap_or(fallback)
    };
    // Mask bins run from span_lo upward; which bucket end that is depends on the turn sign.
    let (angle_at_span_lo, angle_at_span_hi) = if turn < 0.0 {
        (angle_hi, angle_lo)
    } else {
        (angle_lo, angle_hi)
    };
    let mut bin = 0;
    while bin < WIDE_BUCKET_MASK_BINS {
        let run_blocked = blocked[bin];
        let mut run_end = bin;
        while run_end < WIDE_BUCKET_MASK_BINS && blocked[run_end] == run_blocked {
            run_end += 1;
        }
        let run_lo = span_lo + bin as f64 * bin_width;
        let run_hi = if run_end == WIDE_BUCKET_MASK_BINS {
            span_hi
        } else {
            span_lo + run_end as f64 * bin_width
        };
        let parts = ((run_hi - run_lo) / WIDE_BUCKET_PART_MAX_SPAN_RAD)
            .ceil()
            .clamp(1.0, WIDE_BUCKET_MAX_PARTS_PER_RUN as f64) as usize;
        let step = (run_hi - run_lo) / parts as f64;
        for part in 0..parts {
            let part_lo = run_lo + part as f64 * step;
            let part_hi = if part + 1 == parts && run_end == WIDE_BUCKET_MASK_BINS {
                span_hi
            } else {
                part_lo + step
            };
            let edge_lo = if bin == 0 && part == 0 {
                angle_at_span_lo
            } else {
                angle_at_azimuth(part_lo, angle_at_span_lo)
            };
            let edge_hi = if run_end == WIDE_BUCKET_MASK_BINS && part + 1 == parts {
                angle_at_span_hi
            } else {
                angle_at_azimuth(part_hi, angle_at_span_hi)
            };
            let along = geometry
                .along_at_azimuth(0.5 * (part_lo + part_hi))
                .unwrap_or_else(|| geometry.along_at_in_plane_angle(0.5 * (edge_lo + edge_hi)));
            let (stretch_a, stretch_b) = (
                geometry.along_at_in_plane_angle(edge_lo),
                geometry.along_at_in_plane_angle(edge_hi),
            );
            nodes.push(LineQuadratureNode {
                along_m: along,
                along_lo_m: stretch_a.min(stretch_b),
                along_hi_m: stretch_a.max(stretch_b),
                weight_rad: directivity.weight(edge_lo, edge_hi),
                obstacles_on_ray: run_blocked,
            });
        }
        bin = run_end;
    }
}

/// Marks the mask bins one edge blocks: the parts of its arc inside the span whose edge stands
/// in front of the piece (at least a metre nearer than the source point seen there).
fn mark_blocked_bins(
    geometry: &LinePieceGeometry,
    arc: ReceiverSkylineArc,
    need_radius: f64,
    span_lo: f64,
    span_hi: f64,
    bin_width: f64,
    blocked: &mut [bool; WIDE_BUCKET_MASK_BINS],
) {
    if arc.nearest_m > need_radius || arc.nearest_m < 1.0 {
        return;
    }
    for shift in [0.0, 2.0 * PI, -2.0 * PI] {
        let piece_lo = (arc.lo_rad + shift).max(span_lo);
        let piece_hi = (arc.hi_rad + shift).min(span_hi);
        if piece_hi <= piece_lo {
            continue;
        }
        let Some(along) = geometry.along_at_azimuth(0.5 * (piece_lo + piece_hi)) else {
            continue;
        };
        if geometry.horizontal_range_at(along) - arc.nearest_m <= 1.0 {
            continue;
        }
        let first = ((piece_lo - span_lo) / bin_width).floor().max(0.0) as usize;
        let last = (((piece_hi - span_lo) / bin_width).ceil() as usize)
            .saturating_sub(1)
            .max(first)
            .min(WIDE_BUCKET_MASK_BINS - 1);
        for flag in &mut blocked[first..=last] {
            *flag = true;
        }
    }
}

#[cfg(test)]
#[path = "line_quadrature_tests.rs"]
mod tests;

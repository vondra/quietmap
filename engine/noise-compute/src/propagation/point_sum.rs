//! CNOSSOS-EU point-source decomposition of a straight line source (Directive 2015/996 Annex II
//! §2.5.3): free-field-exact point nodes of any spacing — the fine reference the line quadrature
//! is measured against (`point-sum-oracle`, the quadrature tests).

use super::line_quadrature::LINE_PERPENDICULAR_FLOOR_M;

/// CNOSSOS-EU (2.5.12) point-source divergence offset: `10·lg 4π` = 10.99 dB, printed as 11 in
/// the Directive and used verbatim by its reference implementation (NoiseModelling `getADiv`).
pub const POINT_SOURCE_DIVERGENCE_OFFSET_DB: f64 = 11.0;

/// One point source standing in for a stretch of a line source.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineNode {
    /// Node position in the caller's metric frame, `[x, y, z]` metres.
    pub position_m: [f64; 3],
    /// Position on the piece, 0 at its start and 1 at its end.
    pub piece_fraction: f64,
    /// Line length the node carries [m]: its sound power is `L_W′ + 10·lg(length_m)`.
    pub length_m: f64,
    /// Node–receiver distance of the placement geometry [m].
    pub slant_distance_m: f64,
}

/// Cell limits of the decomposition: a cell never subtends more than `max_angle_rad` at the
/// receiver and never carries more than `max_length_m` of line. They bound only how far apart
/// the path effects (ground, screening, air absorption) are sampled; free field is exact for
/// any partition.
#[derive(Debug, Clone, Copy)]
pub struct NodeSpacing {
    pub max_angle_rad: f64,
    pub max_length_m: f64,
}

/// Decompose the straight 3D line `start_m → end_m` into point sources for `receiver_m`.
///
/// Each cell `[s0, s1]` (along-line coordinates from the receiver's perpendicular foot, one
/// side of the foot only) gets one node at the distance `d` with `(s1 − s0)/d² = ∫ds/(D² + s²)`,
/// `D` the 3D perpendicular distance to the infinite line. The inverse-square node sum is
/// therefore the exact free-field line integral, whatever the cell partition. A receiver on
/// the line itself is evaluated as a line `LINE_PERPENDICULAR_FLOOR_M` away. A zero-length piece radiates
/// nothing and yields no node.
#[must_use]
pub fn line_nodes(
    start_m: [f64; 3],
    end_m: [f64; 3],
    receiver_m: [f64; 3],
    spacing: NodeSpacing,
) -> Vec<LineNode> {
    let along = sub(end_m, start_m);
    let length = norm(along);
    if length == 0.0 {
        return Vec::new();
    }
    let unit = scale(along, 1.0 / length);
    let foot_from_start = dot(sub(receiver_m, start_m), unit);
    let foot = add(start_m, scale(unit, foot_from_start));
    let lever = norm(sub(receiver_m, foot)).max(LINE_PERPENDICULAR_FLOOR_M);
    let (low, high) = (-foot_from_start, length - foot_from_start);

    let mut sides = vec![(low, high)];
    if low < 0.0 && high > 0.0 {
        sides = vec![(low, 0.0), (0.0, high)];
    }
    let mut nodes = Vec::new();
    for (side_low, side_high) in sides {
        let (angle_low, angle_high) = ((side_low / lever).atan(), (side_high / lever).atan());
        let angle_cells = ((angle_high - angle_low) / spacing.max_angle_rad)
            .ceil()
            .max(1.0) as usize;
        for angle_cell in 0..angle_cells {
            let cell_low = if angle_cell == 0 {
                side_low
            } else {
                lever * (angle_low + (angle_high - angle_low) * angle_cell as f64 / angle_cells as f64).tan()
            };
            let cell_high = if angle_cell + 1 == angle_cells {
                side_high
            } else {
                lever
                    * (angle_low + (angle_high - angle_low) * (angle_cell + 1) as f64 / angle_cells as f64)
                        .tan()
            };
            let pieces = ((cell_high - cell_low) / spacing.max_length_m).ceil().max(1.0) as usize;
            for piece in 0..pieces {
                let s0 = cell_low + (cell_high - cell_low) * piece as f64 / pieces as f64;
                let s1 = cell_low + (cell_high - cell_low) * (piece + 1) as f64 / pieces as f64;
                if s1 <= s0 {
                    continue;
                }
                // atan(s1/D) − atan(s0/D) in its cancellation-free form; s0·s1 ≥ 0 on one side.
                let subtended = ((s1 - s0) * lever / (lever * lever + s0 * s1)).atan();
                let distance_sq = (s1 - s0) * lever / subtended;
                let side = if s0 + s1 < 0.0 { -1.0 } else { 1.0 };
                let node_s = side * (distance_sq - lever * lever).max(0.0).sqrt();
                nodes.push(LineNode {
                    position_m: add(foot, scale(unit, node_s)),
                    piece_fraction: (foot_from_start + node_s) / length,
                    length_m: s1 - s0,
                    slant_distance_m: distance_sq.sqrt(),
                });
            }
        }
    }
    nodes
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: [f64; 3], k: f64) -> [f64; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn norm(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

#[cfg(test)]
#[path = "point_sum_tests.rs"]
mod tests;

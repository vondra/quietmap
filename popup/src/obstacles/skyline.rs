//! The receiver skyline of the line quadrature (dev4 `skyline.rs`): every edge near the receiver
//! whose directions may meet a wedge, as the short arc of azimuths it occupies and its nearest
//! range. Azimuths are atan2(north, east) in radians, the convention of `physics::line`.

use super::{Located, Scene};
use physics::line::{SkylineArc, wrap_to_pi};
use tiles::obstacles::CELL_STEPS;

/// Distance from the origin to the segment `a-b` (origin-relative metres).
fn origin_to_segment_distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
    let length2 = ex * ex + ey * ey;
    let t = if length2 > 0.0 {
        (-(a[0] * ex + a[1] * ey) / length2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (x, y) = (a[0] + t * ex, a[1] + t * ey);
    (x * x + y * y).sqrt()
}

impl Scene<'_> {
    /// Hands `visit` every edge within `radius_m` of `origin` (click metres) that can still break
    /// a sight line inside the wedge `[lo_rad, hi_rad]` (under a half-turn), as the short arc
    /// between its endpoint azimuths (`lo_rad <= hi_rad`, possibly beyond (-pi, pi]) and its
    /// nearest range. An edge listed in several cells is handed over once per cell, identically.
    ///
    /// Prunes, all exact: cells beyond the radius; cells with all four corners strictly outside
    /// one of the wedge's two half-planes; cells whose tallest outline is no taller than
    /// `los_floor_m`, the lowest the sight line runs (the source height); walls no taller than
    /// `los_floor_m` on their own height (a 3 m wall sharing a cell with a 20 m building must not
    /// block); edges through the origin. There is no lower distance bound: a wall 0.4 m from the
    /// receiver still screens (dev4 z13 4417/2775 pixel 62174 read 16 dB loud without it).
    ///
    /// Cells of tiles that were not read are skipped: every source lies inside the read block
    /// (sources are split at tile edges), the block is convex and holds the receiver, so an edge
    /// outside it stands behind the source at every azimuth it shares with the wedge and can
    /// never block (`mark_blocked_bins` needs the edge in front of the source).
    pub fn skyline_arcs(
        &self,
        origin: [f64; 2],
        lo_rad: f64,
        hi_rad: f64,
        radius_m: f64,
        los_floor_m: f64,
        visit: &mut dyn FnMut(SkylineArc),
    ) {
        let [ox, oy] = self.lattice.steps(origin);
        let [east_m, north_m] = self.lattice.metres_per_step;
        let cell_steps = CELL_STEPS as f64;
        let cells = |centre: f64, half: f64| {
            ((centre - half) / cell_steps).floor() as i64
                ..=((centre + half) / cell_steps).floor() as i64
        };
        let (low, high) = ([lo_rad.cos(), lo_rad.sin()], [hi_rad.cos(), hi_rad.sin()]);
        let relative = |v: [f64; 2]| [(v[0] - ox) * east_m, (oy - v[1]) * north_m];
        for row in cells(oy, radius_m / north_m) {
            // Metres north of the origin of the row's south and north edges (rows grow south).
            let south = (oy - (row + 1) as f64 * cell_steps) * north_m;
            let north = (oy - row as f64 * cell_steps) * north_m;
            let dy = south.max(-north).max(0.0);
            for column in cells(ox, radius_m / east_m) {
                let west = (column as f64 * cell_steps - ox) * east_m;
                let east = ((column + 1) as f64 * cell_steps - ox) * east_m;
                let dx = west.max(-east).max(0.0);
                if dx * dx + dy * dy > radius_m * radius_m {
                    continue;
                }
                let corners = [[west, south], [east, south], [west, north], [east, north]];
                // Outside the low edge: clockwise of it; outside the high edge: anticlockwise.
                let below = corners.iter().all(|c| low[0] * c[1] - low[1] * c[0] < 0.0);
                let above = corners
                    .iter()
                    .all(|c| c[0] * high[1] - c[1] * high[0] < 0.0);
                if below || above {
                    continue;
                }
                let Located::Read(tile, index) = self.locate([column, row]) else {
                    continue;
                };
                if tile.cell_max_height_m[index] <= los_floor_m {
                    continue;
                }
                for run in tile.obstacles.cell_runs(index) {
                    let record = tile.obstacles.outline(run.outline as usize);
                    if !record.kind.is_building() && record.height_m <= los_floor_m {
                        continue;
                    }
                    let first = record.first_vertex + usize::from(run.first_edge);
                    let mut a = relative(tile.vertex(first));
                    let mut azimuth_a = a[1].atan2(a[0]);
                    for next in first + 1..=first + usize::from(run.edge_count) {
                        let b = relative(tile.vertex(next));
                        let azimuth_b = b[1].atan2(b[0]);
                        let nearest_m = origin_to_segment_distance(a, b);
                        if (1e-6..=radius_m).contains(&nearest_m) {
                            // The short arc between the endpoints: the directions that hit the edge.
                            let unwrapped_b = azimuth_a + wrap_to_pi(azimuth_b - azimuth_a);
                            visit(SkylineArc {
                                lo_rad: azimuth_a.min(unwrapped_b),
                                hi_rad: azimuth_a.max(unwrapped_b),
                                nearest_m,
                            });
                        }
                        (a, azimuth_a) = (b, azimuth_b);
                    }
                }
            }
        }
    }
}

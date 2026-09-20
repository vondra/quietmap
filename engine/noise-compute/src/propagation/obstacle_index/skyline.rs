//! Gather nearby obstacle arcs with grazing, annulus and wedge pruning.

use super::{origin_to_segment_dist, wrap_pi, ObstacleIndex, ObstacleKind};
use crate::propagation::screening_source_id::ScreeningSourceId;

/// Directions occupied by one edge, used to build a receiver's skyline.
///
/// `lo`/`hi` are absolute azimuths (`atan2(north, east)`, radians) of the SHORT
/// arc between the edge's endpoints, unwrapped so `lo <= hi` and `hi - lo < π`:
/// an edge subtends less than a half-turn from any point off it, so the pair is
/// an ordinary interval on the line, never a wrap-around case. `near_m` is the
/// nearest range from the origin to the edge — the "does this stand in FRONT of
/// the source" test, which replaces a second ray query per candidate.
#[derive(Clone, Copy, Debug)]
pub struct SkylineArc {
    /// Stable flattened edge identity — the ordinal [`SeenEdges`] dedupes on.
    pub source_id: ScreeningSourceId,
    pub lo: f64,
    pub hi: f64,
    pub near_m: f32,
    /// Edge height above its own local ground (m).
    pub height_m: f32,
}

/// Skyline edge deduplication; clear only the words touched by a receiver.
#[derive(Default)]
pub struct SeenEdges {
    bits: Vec<u64>,
    dirty_words: Vec<u32>,
}

impl SeenEdges {
    /// Marks `ordinal`; `false` when it was already marked.
    fn insert(&mut self, ordinal: u64) -> bool {
        let word = (ordinal / 64) as usize;
        if word >= self.bits.len() {
            self.bits.resize(word + 1, 0);
        }
        let bit = 1_u64 << (ordinal % 64);
        let w = &mut self.bits[word];
        if *w & bit != 0 {
            return false;
        }
        if *w == 0 {
            self.dirty_words.push(word as u32);
        }
        *w |= bit;
        true
    }

    pub fn clear(&mut self) {
        for &word in &self.dirty_words {
            self.bits[word as usize] = 0;
        }
        self.dirty_words.clear();
    }
}

impl ObstacleIndex {
    /// Every edge within `radius_m` of `(lat, lon)` that can still break a line
    /// of sight there, as a [`SkylineArc`] handed to `visit`.
    ///
    /// The AREA sibling of [`Self::crossings`]: a ray query answers "what does
    /// THIS ray hit", this one answers "what stands around this point, and in
    /// which directions" — the receiver skyline every segment of that receiver
    /// then clips its own angular span against, instead of re-running an area
    /// query per (segment, receiver) pair.
    ///
    /// THREE prunes, all exact-by-construction — two O(1) per grid cell, one
    /// per BARRIER edge:
    ///
    /// * empty cells (`cell_max_h == 0`) cost one CSR compare;
    /// * the ISO 9613-2 §7.3 GRAZING prune — an obstacle rising `h` above the
    ///   sight line at distance `b` from this end of a path much longer than
    ///   `b` bends the path by `δ ≈ h²/(2b)`, and `diffraction::maekawa_bands`
    ///   zeroes every band whose `δ ≤ λ/4 − δ*` (flat ground: `δ ≤ λ/8`). So a
    ///   cell whose TALLEST edge cannot reach `h ≥ sqrt(2·δ_min·b)` cannot
    ///   produce a single dB in any band, whatever its geometry — skip it whole.
    ///   `los_floor_m` is the LOWEST the sight line ever runs above local
    ///   ground on such a path — the SOURCE height, not the receiver's: the
    ///   line drops from receiver height to source height as it goes out, so an
    ///   obstacle only shorter than the receiver can still break it further
    ///   along. Gating on the receiver's height would silently drop every 3 m
    ///   noise wall and low building. `delta_min_m` is the caller's δ floor.
    /// * a BARRIER edge no taller than `los_floor_m` — a wall standing entirely
    ///   under the sight line blocks nothing, and it is admitted or not on its
    ///   own height, never on the tallest edge sharing its cell.
    ///
    /// An edge listed in several cells is visited several times; `seen` lets
    /// each edge through once per skyline (see [`SeenEdges`]) — `None` for a
    /// capped merge, whose cross-stratum last resort can drop the record a
    /// repeat would restore.
    #[allow(clippy::too_many_arguments)]
    pub fn skyline_arcs_within(
        &self,
        edge_ordinal_base: u64,
        lat: f64,
        lon: f64,
        min_radius_m: f64,
        radius_m: f64,
        los_floor_m: f64,
        delta_min_m: f64,
        wedge: Option<(f64, f64)>,
        mut seen: Option<&mut SeenEdges>,
        visit: &mut impl FnMut(SkylineArc),
    ) {
        if self.edges.is_empty() {
            return;
        }
        let (ox, oy) = self.to_local(lat, lon);
        let inv_cell = 1.0 / self.cell_m;
        let cell_range = |lo: f64, hi: f64, base: f64, n: usize| -> Option<(usize, usize)> {
            let c0 = ((lo - base) * inv_cell).floor();
            let c1 = ((hi - base) * inv_cell).floor();
            if c1 < 0.0 || c0 > (n - 1) as f64 {
                return None; // query box entirely outside the grid slab
            }
            Some((c0.max(0.0) as usize, c1.min((n - 1) as f64) as usize))
        };
        let Some((cx0, cx1)) = cell_range(ox - radius_m, ox + radius_m, self.min_x, self.cols)
        else {
            return;
        };
        let Some((cy0, cy1)) = cell_range(oy - radius_m, oy + radius_m, self.min_y, self.rows)
        else {
            return;
        };
        let r2 = radius_m * radius_m;
        // Wedge reject. A segment can only be clipped by obstacles inside its
        // OWN angular span, so gathering the whole disk collects area no query
        // can read — ~180× for a rail segment 3 km out with a 2° span. The
        // span is under a half turn, so the wedge is the intersection of two
        // half-planes and a cell is rejected when all four of its corners sit
        // strictly outside one of them. Cross products only: no `atan2` in a
        // loop that runs per cell.
        let wedge_dirs = wedge.map(|(lo, hi)| ((lo.cos(), lo.sin()), (hi.cos(), hi.sin())));

        for cy in cy0..=cy1 {
            let row = cy * self.cols;
            // Nearest point of this cell ROW to the origin, then of the cell —
            // the largest `b` lower bound the grid can give without touching an
            // edge, which is what makes the grazing prune tight.
            let y_lo = self.min_y + cy as f64 * self.cell_m;
            let dy = (y_lo - oy).max(oy - (y_lo + self.cell_m)).max(0.0);
            for cx in cx0..=cx1 {
                let cell = row + cx;
                let lo = self.cell_starts[cell] as usize;
                let hi = self.cell_starts[cell + 1] as usize;
                if lo == hi {
                    continue;
                }
                let x_lo = self.min_x + cx as f64 * self.cell_m;
                let dx = (x_lo - ox).max(ox - (x_lo + self.cell_m)).max(0.0);
                let b2 = dx * dx + dy * dy;
                if b2 > r2 {
                    continue;
                }
                if let Some(((lx, ly), (hx, hy))) = wedge_dirs {
                    let (cx0, cy0c) = (x_lo - ox, y_lo - oy);
                    let (cx1, cy1c) = (cx0 + self.cell_m, cy0c + self.cell_m);
                    let corners = [(cx0, cy0c), (cx1, cy0c), (cx0, cy1c), (cx1, cy1c)];
                    // Outside the LOW edge: the corner is clockwise of it.
                    let all_below = corners.iter().all(|&(px, py)| lx * py - ly * px < 0.0);
                    // Outside the HIGH edge: the corner is anticlockwise of it.
                    let all_above = corners.iter().all(|&(px, py)| px * hy - py * hx < 0.0);
                    if all_below || all_above {
                        continue;
                    }
                }
                // Already covered by an earlier, smaller-radius pass: the cell's
                // FARTHEST corner is inside it, so every edge it holds was
                // visited then. Growing a skyline is an annulus walk, never a
                // re-walk (`ArcSkyline::ensure`).
                if min_radius_m > 0.0 {
                    let fx = (x_lo - ox).abs().max((x_lo + self.cell_m - ox).abs());
                    let fy = (y_lo - oy).abs().max((y_lo + self.cell_m - oy).abs());
                    if fx * fx + fy * fy <= min_radius_m * min_radius_m {
                        continue;
                    }
                }
                let h = self.cell_max_h[cell] as f64 - los_floor_m;
                if h <= 0.0 || h * h < 2.0 * delta_min_m * b2.sqrt() {
                    continue; // grazing: zero dB in every band, whatever the edge
                }
                for &eref in &self.edge_refs[lo..hi] {
                    let e = self.edges[eref as usize];
                    // A WALL is pruned on its OWN height, a building only on the
                    // cell's tallest edge. The wall slice this index replaced
                    // tested every wall against the sight-line floor, and the
                    // cell maximum is no substitute: a 3 m wall sharing a cell
                    // with a 20 m building would start blocking directions it
                    // cannot reach. Buildings keep the cell prune alone — the
                    // bound their footprints have always been screened by.
                    if e.kind() == ObstacleKind::Barrier && f64::from(e.height_m) <= los_floor_m {
                        continue;
                    }
                    let (ex0, ey0) = (e.x0 as f64 - ox, e.y0 as f64 - oy);
                    let (ex1, ey1) = (e.x1 as f64 - ox, e.y1 as f64 - oy);
                    let near_m = origin_to_segment_dist(ex0, ey0, ex1, ey1);
                    if near_m > radius_m || near_m < 1e-6 {
                        continue; // out of range, or the origin sits ON the edge
                    }
                    // Marked only once ADMITTED: an edge this growth's radius or
                    // floor pruned must still get through on a later, larger one.
                    let ordinal = edge_ordinal_base
                        .checked_add(u64::from(eref))
                        .expect("flattened obstacle edge ordinal overflow");
                    if seen
                        .as_deref_mut()
                        .is_some_and(|seen| !seen.insert(ordinal))
                    {
                        continue;
                    }
                    let a0 = ey0.atan2(ex0);
                    let a1 = ey1.atan2(ex1);
                    // The SHORT arc between the endpoints: the set of directions
                    // that hit this edge. Taking it per EDGE (not a per-footprint
                    // hull) is exact for concave outlines too — a ray leaving the
                    // origin hits a closed ring iff it hits one of its edges.
                    let r1 = a0 + wrap_pi(a1 - a0);
                    visit(SkylineArc {
                        source_id: ScreeningSourceId::obstacle(ordinal)
                            .expect("flattened obstacle edge ordinal entered wall namespace"),
                        lo: a0.min(r1),
                        hi: a0.max(r1),
                        near_m: near_m as f32,
                        height_m: e.height_m,
                    });
                }
            }
        }
    }
}

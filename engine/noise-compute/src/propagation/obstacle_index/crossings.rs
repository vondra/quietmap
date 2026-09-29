//! Walk ray cells, prune impossible crossings, and deduplicate exact edge hits.

use super::geometry::{ray_cell_aabb, ray_cell_aabb_may_overlap, segment_intersection_t};
use super::{CellGate, CellPrune, CrossingCandidate, CrossingScratch, ObstacleIndex, ObstacleKind};

impl ObstacleIndex {
    /// Exact crossings of the ray `src→rcv`, endpoint-exclusive
    /// (`t ∈ (0, 1)`), appended to `out` (cleared first), sorted by `t` and
    /// deduped to one candidate per (obstacle, chainage). Endpoint
    /// exclusivity drops hits AT the endpoints only; a footprint CONTAINING
    /// an endpoint still reports its entry/exit edge — filtering the
    /// source's own building is the caller's job (`exclusion_radius_m`
    /// semantics live in `path_effects`, not here).
    pub fn crossings(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
        out: &mut Vec<CrossingCandidate>,
    ) {
        self.walk_ray(src_lat, src_lon, rcv_lat, rcv_lon, CellGate::All, out);
    }

    /// [`Self::crossings`] with the per-cell branch-and-bound prune.
    ///
    /// `prune.floor_m` MUST be the floor of the loop this accelerates — the
    /// consumer that ranks these candidates. For `path_effects` §5b that floor
    /// is [`crate::constants::PENUMBRA_DELTA_FLOOR_M`], NOT zero: that loop
    /// deliberately keeps below-sight-line near misses with a negative δ
    /// (fix-pack Fix 2), and a prune floored at 0 would delete exactly the
    /// geometry a noise wall exists to create. A prune whose floor sits above
    /// its loop's floor is unsound however tight its bound is.
    ///
    /// The floor is δ\*-FREE on purpose. The rejection threshold is DECREASING
    /// in δ\*, so assuming a δ\* larger than the true one rejects paths that
    /// still carry energy; only a proven LOWER bound on δ\* is admissible, and
    /// at prune time δ\* is not yet computed. The infimum over all δ\* is
    /// −λ/20 at the longest wavelength in the model, which is exactly this
    /// constant.
    pub fn crossings_pruned(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
        prune: &CellPrune<'_>,
        out: &mut Vec<CrossingCandidate>,
    ) {
        self.walk_ray(
            src_lat,
            src_lon,
            rcv_lat,
            rcv_lon,
            CellGate::Delta(prune),
            out,
        );
    }

    /// This index's crossings of the ray under an `All` or `Delta` gate,
    /// t-sorted into `out`. A `TallestBuilding` answer lives in the scratch
    /// this helper does not return — [`super::ObstacleSet::max_height_crossed`]
    /// walks that one.
    fn walk_ray(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
        gate: CellGate<'_>,
        out: &mut Vec<CrossingCandidate>,
    ) {
        debug_assert!(!matches!(gate, CellGate::TallestBuilding));
        out.clear();
        self.append_crossings(
            src_lat,
            src_lon,
            rcv_lat,
            rcv_lon,
            gate,
            &mut CrossingScratch::default(),
            out,
        );
    }

    /// [`Self::crossings`] without the clear: appends this index's hits under
    /// `gate` and sort+dedups ONLY the appended tail, so [`super::ObstacleSet`] can
    /// chain per-cell indexes into one buffer with zero per-ray allocation.
    pub(super) fn append_crossings(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
        gate: CellGate<'_>,
        scratch: &mut CrossingScratch,
        out: &mut Vec<CrossingCandidate>,
    ) {
        let start = out.len();
        if self.edges.is_empty() {
            return;
        }
        let generation = scratch.begin_ray();
        let (sx, sy) = self.to_local(src_lat, src_lon);
        let (rx, ry) = self.to_local(rcv_lat, rcv_lon);
        let (dx, dy) = (rx - sx, ry - sy);

        // DDA over grid cells (Amanatides & Woo), clamped to the grid slab.
        let inv_cell = 1.0 / self.cell_m;
        let mut cx = (((sx - self.min_x) * inv_cell).floor() as i64).clamp(0, self.cols as i64 - 1);
        let mut cy = (((sy - self.min_y) * inv_cell).floor() as i64).clamp(0, self.rows as i64 - 1);
        let end_cx = (((rx - self.min_x) * inv_cell).floor() as i64).clamp(0, self.cols as i64 - 1);
        let end_cy = (((ry - self.min_y) * inv_cell).floor() as i64).clamp(0, self.rows as i64 - 1);

        let step_x: i64 = if dx >= 0.0 { 1 } else { -1 };
        let step_y: i64 = if dy >= 0.0 { 1 } else { -1 };
        // Δt of one cell step per axis (Amanatides & Woo tDelta).
        let t_delta_x = if dx != 0.0 {
            (self.cell_m / dx).abs()
        } else {
            f64::INFINITY
        };
        let t_delta_y = if dy != 0.0 {
            (self.cell_m / dy).abs()
        } else {
            f64::INFINITY
        };
        let next_x_boundary = self.min_x + (cx + i64::from(dx >= 0.0)) as f64 * self.cell_m;
        let next_y_boundary = self.min_y + (cy + i64::from(dy >= 0.0)) as f64 * self.cell_m;
        let mut t_max_x = if dx != 0.0 {
            ((next_x_boundary - sx) / dx).abs()
        } else {
            f64::INFINITY
        };
        let mut t_max_y = if dy != 0.0 {
            ((next_y_boundary - sy) / dy).abs()
        } else {
            f64::INFINITY
        };

        // An edge spans every supercover cell it passes through, so the ray
        // can re-test it in each of them. The generation-tagged direct-mapped
        // 64-slot table records EVERY edge that reaches the exact predicate,
        // not only hits: ray and edge are immutable within this walk, so
        // repeating the predicate cannot change its answer. An AABB rejection
        // is deliberately not remembered: the same edge can span a later DDA
        // cell that contains the true crossing. A hash collision merely evicts
        // the older entry and performs an extra test; it can never suppress a
        // distinct edge. CORRECTNESS still belongs to the post-sort dedup below
        // (a shared ring vertex can hit two edges of one footprint at one
        // chainage).

        let mut guard = (self.cols + self.rows) as i64 + 4;
        // Chainage the ray entered the current cell at, and a monotone pointer
        // into the profile samples — both only ever advance, so the windowed
        // terrain max costs O(samples) over the whole walk.
        let mut t_enter = 0.0_f64;
        let mut win_lo = 0usize;
        loop {
            let cell = cy as usize * self.cols + cx as usize;
            let mut lo = self.cell_starts[cell] as usize;
            let hi = self.cell_starts[cell + 1] as usize;
            if hi > lo {
                // The DDA visit covers this closed ray interval. Both ends
                // are retained because the edge supercover and this DDA walk
                // meet at cell boundaries. Their accumulated and cross-product
                // chainages can differ by a few ulps, so the AABB below is
                // padded before it filters the authoritative exact predicate.
                let t_exit = t_max_x.min(t_max_y).min(1.0);
                let (cell_t_lo, cell_t_hi) = (t_enter.clamp(0.0, 1.0), t_exit.clamp(0.0, 1.0));
                match gate {
                    CellGate::All => {}
                    CellGate::Delta(p) => {
                        while win_lo + 1 < p.t.len() && p.t[win_lo + 1] <= cell_t_lo {
                            win_lo += 1;
                        }
                        let mut terr_win = p.elevation_m[win_lo] as f64;
                        let mut k = win_lo;
                        while k + 1 < p.t.len() && p.t[k] < cell_t_hi {
                            k += 1;
                            terr_win = terr_win.max(p.elevation_m[k] as f64);
                        }
                        let top_bound = terr_win + self.cell_max_h[cell] as f64;
                        if p.max_delta(top_bound, cell_t_lo, cell_t_hi) < p.floor_m {
                            lo = hi; // no edge here can reach the consumer's floor
                        }
                    }
                    CellGate::TallestBuilding => {
                        // The cell maximum counts walls too, so the skip is
                        // conservative: no building here can top the best.
                        if self.cell_max_h[cell] <= scratch.tallest_building_m {
                            lo = hi;
                        }
                    }
                }
                if lo < hi {
                    let (ray_x, ray_y) = ray_cell_aabb(sx, sy, dx, dy, cell_t_lo, cell_t_hi);
                    for &eref in &self.edge_refs[lo..hi] {
                        let slot = eref as usize & (scratch.recent.len() - 1);
                        let tag = (u64::from(generation) << 32) | u64::from(eref);
                        if scratch.recent[slot] == tag {
                            continue;
                        }
                        let e = &self.edges[eref as usize];
                        if !ray_cell_aabb_may_overlap(ray_x, ray_y, e) {
                            continue;
                        }
                        // A rejection in one cell must remain eligible in a later cell.
                        scratch.recent[slot] = tag;
                        if let Some(t) = segment_intersection_t(
                            sx,
                            sy,
                            dx,
                            dy,
                            e.x0 as f64,
                            e.y0 as f64,
                            e.x1 as f64,
                            e.y1 as f64,
                        ) {
                            if let CellGate::TallestBuilding = gate {
                                if e.kind() == ObstacleKind::Building {
                                    scratch.tallest_building_m =
                                        scratch.tallest_building_m.max(e.height_m);
                                }
                                continue;
                            }
                            out.push(CrossingCandidate {
                                t,
                                height_m: e.height_m,
                                kind: e.kind(),
                                id: e.id,
                                index: 0,
                            });
                        }
                    }
                }
            }
            if (cx == end_cx && cy == end_cy) || guard <= 0 {
                break;
            }
            guard -= 1;
            t_enter = t_max_x.min(t_max_y);
            if t_max_x < t_max_y {
                t_max_x += t_delta_x;
                cx += step_x;
            } else {
                t_max_y += t_delta_y;
                cy += step_y;
            }
            if cx < 0 || cy < 0 || cx >= self.cols as i64 || cy >= self.rows as i64 {
                break;
            }
        }
        out[start..].sort_unstable_by(|a, b| a.t.partial_cmp(&b.t).unwrap());
        // One candidate per (obstacle, chainage): kills ring-eviction repeats
        // (same edge ⇒ bit-identical t) and vertex double-counts (two edges of
        // one ring meeting at the hit point; tolerance covers their last-ulp
        // difference). A tangent ray thus yields ONE conservative candidate.
        // In-place tail dedup (slices have no `dedup_by`), keep-first.
        let mut w = start;
        for r in start..out.len() {
            if w > start && out[r].id == out[w - 1].id && (out[r].t - out[w - 1].t).abs() < 1e-9 {
                continue;
            }
            out[w] = out[r];
            w += 1;
        }
        out.truncate(w);
    }
}

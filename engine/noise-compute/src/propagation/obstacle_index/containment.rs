//! Test footprint parity, name footprints across squares and select the tallest enclosed one.

use super::{ObstacleIndex, ObstacleKind, ObstacleSet};
use crate::envelope::EnvelopeClass;

/// One footprint of the world: the z9 square whose `structures.arrow` owns it
/// and its dense index id there, which equals the row's `screening_ordinal`.
/// The structures producer assigns every footprint to exactly one square (its
/// centroid's), so this names a building once however many squares it spans.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FootprintKey {
    pub square_y: u16,
    pub square_x: u16,
    pub id: u32,
}

impl FootprintKey {
    pub fn square(self) -> grid::Square {
        grid::Square {
            x: self.square_x,
            y: self.square_y,
        }
    }
}

/// The enclosed footprint a point belongs to, by the one winner rule the popup,
/// the façade-exposure stage and the painter share.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnclosedFootprint {
    pub key: FootprintKey,
    pub class: EnvelopeClass,
    pub height_m: f32,
}

impl ObstacleSet {
    /// Tallest enclosed footprint containing the point; equal heights resolve
    /// to the smallest [`FootprintKey`], so every set holding both footprints
    /// picks the same one whatever its index order.
    pub fn enclosed_footprint_at(&self, lat: f64, lon: f64) -> Option<EnclosedFootprint> {
        let mut seen = Vec::new();
        self.indexes
            .iter()
            .filter_map(|index| {
                let square = index.square();
                index
                    .containing_enclosed(lat, lon, 0.0, &mut seen)
                    .map(|(class, height_m, id)| EnclosedFootprint {
                        key: FootprintKey {
                            square_y: square.y,
                            square_x: square.x,
                            id,
                        },
                        class,
                        height_m,
                    })
            })
            .max_by(|a, b| {
                a.height_m
                    .total_cmp(&b.height_m)
                    .then_with(|| b.key.cmp(&a.key))
            })
    }
}

impl ObstacleIndex {
    /// Point-in-footprint test via PER-FOOTPRINT crossing parity along the
    /// probe's eastward half-line: a point is inside footprint `id` iff that
    /// footprint's boundary crosses the half-line an odd number of times
    /// (holes share the outer ring's id, so courtyards read outside; a
    /// global parity bit would break on overlapping footprints). Only
    /// footprints with `height_m > min_height_m` count.
    ///
    /// Parity invariants:
    /// - Vertices use the classic half-open straddle rule
    ///   `(y0 > y) != (y1 > y)` — the same convention as `wkb.rs`
    ///   point-in-polygon: transit vertices count once, tangent vertices
    ///   twice or zero, horizontal edges never. No epsilon, no dedup.
    /// - An edge is listed in every row cell it passes; only the cell
    ///   CONTAINING the crossing point counts it (owner-cell rule), so
    ///   multi-cell edges cannot double-count.
    /// - The walk is bounded by `max_footprint_w`, the max footprint bbox
    ///   width IN THIS INDEX: any footprint whose bbox straddles the probe
    ///   ends within that distance east, and footprints starting east of the
    ///   probe (`footprint_xmin > x`) cannot contain it and are skipped —
    ///   both false-positive (ray "ending inside" a far footprint) and
    ///   false-negative (footprint wider than a fixed cast) failure modes of
    ///   a constant-length ray are structurally impossible.
    pub fn contains_built(
        &self,
        lat: f64,
        lon: f64,
        min_height_m: f32,
        seen: &mut Vec<(u32, u32, f32)>,
    ) -> bool {
        self.contains_built_other_than(lat, lon, min_height_m, None, seen)
    }

    /// [`Self::contains_built`] ignoring one footprint of this index (a façade
    /// receiver's own building).
    pub fn contains_built_other_than(
        &self,
        lat: f64,
        lon: f64,
        min_height_m: f32,
        ignored_id: Option<u32>,
        seen: &mut Vec<(u32, u32, f32)>,
    ) -> bool {
        self.collect_containing_footprints(lat, lon, min_height_m, seen);
        seen.iter()
            .any(|(id, crossings, _)| crossings % 2 == 1 && Some(*id) != ignored_id)
    }

    /// The z9 square this index belongs to: production indexes are built with
    /// their square's centre as the metric origin (`structure_store`).
    pub fn square(&self) -> grid::Square {
        grid::square_of(self.origin_lat, self.origin_lon)
    }

    /// Winning enclosed footprint at a point: tallest wins, then lower ordinal.
    /// Hole parity is evaluated per footprint, exactly as `contains_built`.
    pub fn containing_enclosed(
        &self,
        lat: f64,
        lon: f64,
        min_height_m: f32,
        seen: &mut Vec<(u32, u32, f32)>,
    ) -> Option<(EnvelopeClass, f32, u32)> {
        self.collect_containing_footprints(lat, lon, min_height_m, seen);
        seen.iter()
            .filter(|(_, crossings, _)| crossings % 2 == 1)
            .filter_map(|(id, _, height)| {
                let class = EnvelopeClass::from_u8(self.footprint_class[*id as usize]);
                class.is_enclosed().then_some((class, *height, *id))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| b.2.cmp(&a.2)))
    }

    /// Winning footprint at a point, including Outdoor-class structures: the
    /// building hover names visible carports and roof structures, which the
    /// building exposure ([`Self::containing_enclosed`]) treats as open ground.
    pub fn containing_footprint(
        &self,
        lat: f64,
        lon: f64,
        min_height_m: f32,
        seen: &mut Vec<(u32, u32, f32)>,
    ) -> Option<(EnvelopeClass, f32, u32)> {
        self.collect_containing_footprints(lat, lon, min_height_m, seen);
        seen.iter()
            .filter(|(_, crossings, _)| crossings % 2 == 1)
            .map(|(id, _, height)| {
                let class = EnvelopeClass::from_u8(self.footprint_class[*id as usize]);
                (class, *height, *id)
            })
            .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| b.2.cmp(&a.2)))
    }

    /// Walk the eastward containment ray once and retain each footprint's
    /// crossing parity and height at first sighting. Both boolean containment
    /// and envelope winner selection use this same CSR walk; the height is
    /// carried in `seen`, so winner lookup is O(1) instead of rescanning all
    /// edges for every indoor pixel.
    fn collect_containing_footprints(
        &self,
        lat: f64,
        lon: f64,
        min_height_m: f32,
        seen: &mut Vec<(u32, u32, f32)>,
    ) {
        seen.clear();
        if self.edges.is_empty() {
            return;
        }
        let (x, y) = self.to_local(lat, lon);
        // Bbox reject: a point outside this index's edge extent cannot be
        // inside any footprint it owns — kills the ~7x wasted walks when a
        // probe queries every ring cell's index.
        let max_x = self.min_x + self.cols as f64 * self.cell_m;
        let max_y = self.min_y + self.rows as f64 * self.cell_m;
        if x < self.min_x || x > max_x || y < self.min_y || y > max_y {
            return;
        }
        let inv_cell = 1.0 / self.cell_m;
        let cy = (((y - self.min_y) * inv_cell).floor() as i64).clamp(0, self.rows as i64 - 1);
        let mut cx = (((x - self.min_x) * inv_cell).floor() as i64).clamp(0, self.cols as i64 - 1);
        let end_cx = (((x + self.max_footprint_w - self.min_x) * inv_cell).floor() as i64)
            .clamp(0, self.cols as i64 - 1);
        // Horizontal half-line ⇒ the walk stays on one row.
        let row = cy as usize * self.cols;
        while cx <= end_cx {
            let cell_lo = self.min_x + cx as f64 * self.cell_m;
            let cell_hi = cell_lo + self.cell_m;
            let cell = row + cx as usize;
            let lo = self.cell_starts[cell] as usize;
            let hi = self.cell_starts[cell + 1] as usize;
            for &eref in &self.edge_refs[lo..hi] {
                let e = self.edges[eref as usize];
                if e.kind() != ObstacleKind::Building {
                    continue; // a wall is an open chain — crossing parity is
                              // meaningless on it; containment asks about FOOTPRINTS.
                }
                if e.height_m <= min_height_m {
                    continue;
                }
                let (y0, y1) = (e.y0 as f64, e.y1 as f64);
                if (y0 > y) == (y1 > y) {
                    continue; // no straddle (also skips horizontal edges)
                }
                if (self.footprint_xmin[e.id as usize] as f64) > x {
                    continue; // footprint entirely east of the probe
                }
                let (x0, x1) = (e.x0 as f64, e.x1 as f64);
                let xc = x0 + (y - y0) * (x1 - x0) / (y1 - y0);
                if xc > x && xc >= cell_lo && xc < cell_hi {
                    match seen.iter_mut().find(|(id, _, _)| *id == e.id) {
                        Some((_, crossings, _)) => *crossings += 1,
                        None => seen.push((e.id, 1, e.height_m)),
                    }
                }
            }
            cx += 1;
        }
    }
}

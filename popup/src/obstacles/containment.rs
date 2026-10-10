//! Which footprints contain a point: crossing parity per footprint along the eastward half-line
//! (dev4 `containment.rs`), the enclosed building a point stands in, and the receiver reflection
//! bonus of nine probes (dev4 `reflection.rs`).

use super::{Scene, cell_of};
use tiles::obstacles::{CELL_STEPS, EnvelopeClass, OutlineKind};

/// Spacing of the 3 x 3 reflection probes, a 150 m square (dev4 `ENCLOSURE_RADIUS_M`).
const REFLECTION_PROBE_SPACING_M: f64 = 75.0;
/// A probe is built inside a footprint strictly taller than this (dev4 `enclosure_db`).
const REFLECTION_BUILT_ABOVE_M: f64 = 5.0;
/// The largest receiver reflection bonus (dB).
pub const REFLECTION_MAX_DB: f64 = 3.0;

/// A building footprint in click metres.
#[derive(Clone, Debug, PartialEq)]
pub struct Footprint {
    pub id: u64,
    pub height_m: f64,
    pub envelope: EnvelopeClass,
    /// Every ring as stored: each part's exterior followed by its holes, closed (the first point
    /// repeated last).
    pub rings: Vec<FootprintRing>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FootprintRing {
    pub hole: bool,
    pub points: Vec<[f64; 2]>,
}

/// One footprint's crossings of the eastward half-line.
struct Parity {
    footprint_id: u64,
    crossings: u32,
    height_m: f64,
    envelope: EnvelopeClass,
}

impl Parity {
    fn inside(&self) -> bool {
        self.crossings % 2 == 1
    }
}

/// The tallest enclosed footprint of a parity walk; equal heights go to the smallest id, so every
/// scene holding both footprints picks the same one.
fn tallest_enclosed(parities: &[Parity]) -> Option<&Parity> {
    parities
        .iter()
        .filter(|parity| parity.inside() && parity.envelope.is_enclosed())
        .max_by(|a, b| {
            (a.height_m.total_cmp(&b.height_m)).then_with(|| b.footprint_id.cmp(&a.footprint_id))
        })
}

impl<'a> Scene<'a> {
    /// Crossing parity of every footprint taller than `min_height_m` along the eastward half-line
    /// from `point` (scene steps). Holes share their footprint's id, so a courtyard reads outside;
    /// a vertex on the half-line counts by the half-open rule `(y0 > y) != (y1 > y)` (transit
    /// once, tangent twice or never); an edge counts only in the cell holding its crossing (it is
    /// listed in every cell it passes); footprints starting east of the point are skipped, so the
    /// walk, as long as the widest footprint plus a cell, sees every crossing of the others.
    fn footprint_parities(
        &self,
        point: [f64; 2],
        min_height_m: f64,
        parities: &mut Vec<Parity>,
    ) -> Result<(), String> {
        parities.clear();
        let [x, y] = point;
        let cell_steps = CELL_STEPS as f64;
        let [first_column, row] = cell_of(point);
        let reach = (self.widest_footprint_steps + CELL_STEPS) as f64;
        let last_column = ((x + reach) / cell_steps).floor() as i64;
        for column in first_column..=last_column {
            let Some((tile, index)) = self.tile_at([column, row])? else {
                continue;
            };
            let (cell_west, cell_east) =
                (column as f64 * cell_steps, (column + 1) as f64 * cell_steps);
            for run in tile.obstacles.cell_runs(index) {
                let outline = run.outline as usize;
                let record = tile.obstacles.outline(outline);
                let west = (tile.offset[0] + i64::from(tile.footprint_west[outline])) as f64;
                if !record.kind.is_building() || record.height_m <= min_height_m || west > x {
                    continue;
                }
                let first = record.first_vertex + usize::from(run.first_edge);
                let mut a = tile.vertex(first);
                for next in first + 1..=first + usize::from(run.edge_count) {
                    let b = tile.vertex(next);
                    let crossing_x = ((a[1] > y) != (b[1] > y))
                        .then(|| a[0] + (y - a[1]) * (b[0] - a[0]) / (b[1] - a[1]));
                    a = b;
                    let Some(crossing_x) = crossing_x else {
                        continue;
                    };
                    if crossing_x <= x || crossing_x < cell_west || crossing_x >= cell_east {
                        continue;
                    }
                    match parities
                        .iter_mut()
                        .find(|p| p.footprint_id == record.footprint_id)
                    {
                        Some(parity) => parity.crossings += 1,
                        None => parities.push(Parity {
                            footprint_id: record.footprint_id,
                            crossings: 1,
                            height_m: record.height_m,
                            envelope: record.envelope,
                        }),
                    }
                }
            }
        }
        Ok(())
    }

    /// Whether a footprint taller than `min_height_m`, other than `ignored`, contains `point`
    /// (scene steps).
    pub(super) fn contains_built(
        &self,
        point: [f64; 2],
        min_height_m: f64,
        ignored: Option<u64>,
    ) -> Result<bool, String> {
        let mut parities = Vec::new();
        self.footprint_parities(point, min_height_m, &mut parities)?;
        Ok(parities
            .iter()
            .any(|p| p.inside() && Some(p.footprint_id) != ignored))
    }

    /// Whether `point` (click metres) stands inside an enclosed building.
    pub(super) fn in_enclosed_building(&self, point: [f64; 2]) -> Result<bool, String> {
        let mut parities = Vec::new();
        self.footprint_parities(self.lattice.steps(point), 0.0, &mut parities)?;
        Ok(tallest_enclosed(&parities).is_some())
    }

    /// The id of the enclosed building `point` (click metres) stands in, as
    /// [`Scene::enclosing_building`] finds it, without copying its outline.
    pub fn enclosing_building_id(&self, point: [f64; 2]) -> Result<Option<u64>, String> {
        let mut parities = Vec::new();
        self.footprint_parities(self.lattice.steps(point), 0.0, &mut parities)?;
        Ok(tallest_enclosed(&parities).map(|winner| winner.footprint_id))
    }

    /// The enclosed building `point` (click metres) stands in: the tallest enclosed footprint
    /// containing it, equal heights to the smallest id; `None` outdoors, in a courtyard or under
    /// an Outdoor-class roof.
    pub fn enclosing_building(&self, point: [f64; 2]) -> Result<Option<Footprint>, String> {
        let mut parities = Vec::new();
        self.footprint_parities(self.lattice.steps(point), 0.0, &mut parities)?;
        Ok(tallest_enclosed(&parities).map(|winner| self.footprint(winner)))
    }

    /// Every ring of a footprint from every read tile storing it, copies once, tiles in id order.
    fn footprint(&self, parity: &Parity) -> Footprint {
        let mut rings: Vec<FootprintRing> = Vec::new();
        for (_, tile) in self.read_tiles() {
            for index in tile.outlines_of(parity.footprint_id) {
                let record = tile.obstacles.outline(index);
                let vertices = record.first_vertex..record.first_vertex + record.vertex_count;
                let ring = FootprintRing {
                    hole: record.kind == OutlineKind::Hole,
                    points: vertices
                        .map(|v| self.lattice.metres(tile.vertex(v)))
                        .collect(),
                };
                if !rings.contains(&ring) {
                    rings.push(ring);
                }
            }
        }
        Footprint {
            id: parity.footprint_id,
            height_m: parity.height_m,
            envelope: parity.envelope,
            rings,
        }
    }

    /// The receiver reflection bonus (dev4 `enclosure_db`): 3 dB when more than half of nine
    /// probes, 75 m apart around the receiver, stand inside a footprint taller than 5 m, 1.5 dB
    /// above a fifth, else 0. Every envelope class counts, walls do not; the receiver's own
    /// building does not either (CNOSSOS 2.8 excludes the façade's own reflection), the nine-probe
    /// denominator staying.
    pub fn reflection_db(
        &self,
        receiver: [f64; 2],
        own_footprint: Option<u64>,
    ) -> Result<f64, String> {
        let mut built = 0u32;
        for north in [-1.0, 0.0, 1.0] {
            for east in [-1.0, 0.0, 1.0] {
                let probe = [
                    receiver[0] + east * REFLECTION_PROBE_SPACING_M,
                    receiver[1] + north * REFLECTION_PROBE_SPACING_M,
                ];
                let probe = self.lattice.steps(probe);
                if self.contains_built(probe, REFLECTION_BUILT_ABOVE_M, own_footprint)? {
                    built += 1;
                }
            }
        }
        let density = f64::from(built) / 9.0;
        Ok(if density > 0.5 {
            REFLECTION_MAX_DB
        } else if density > 0.2 {
            REFLECTION_MAX_DB / 2.0
        } else {
            0.0
        })
    }
}

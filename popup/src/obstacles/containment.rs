//! Footprint containment by crossing parity along the eastward half-line.

use super::{BuildingParts, Scene, cell_of, footprint_parts::eastward_crossing_x};
use tiles::obstacles::{CELL_STEPS, EnvelopeClass};

/// A building footprint in click metres.
#[derive(Clone, Debug, PartialEq)]
pub struct Footprint {
    pub id: u64,
    pub height_m: f64,
    pub envelope: EnvelopeClass,
    /// Whether this record has only one physical exterior building (so its own home-plant source
    /// matches by `id` even when a concave or courtyard home's centroid lies outside its walls).
    pub single_part: bool,
    /// This building's closed exterior followed by its courtyards.
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
    /// Whether the rectangle meets no stored obstacle edge; unread cells refuse the proof.
    /// With an outdoor sample and no edge, its interior cannot enter a building footprint.
    pub fn has_no_screening_edges(&self, low: [f64; 2], high: [f64; 2]) -> bool {
        let [a, b] = [low, high].map(|point| self.lattice.steps(point));
        let first = cell_of([0, 1].map(|axis| a[axis].min(b[axis])));
        let last = cell_of([0, 1].map(|axis| a[axis].max(b[axis])));
        for row in first[1]..=last[1] {
            for column in first[0]..=last[0] {
                let Ok(cell) = self.tile_at([column, row]) else {
                    return false;
                };
                let Some((tile, index)) = cell else { continue };
                let rectangle = super::crossings::piece_box([a, b], 0.0, tile.offset);
                for run_index in tile.obstacles.cell_run_range(index) {
                    let run = tile.obstacles.run(run_index);
                    let first_vertex = tile.run_first_vertex[run_index] as usize;
                    for edge in first_vertex..first_vertex + usize::from(run.edge_count) {
                        if super::crossings::edge_meets_box(
                            tile.obstacles.vertex(edge),
                            tile.obstacles.vertex(edge + 1),
                            rectangle,
                        ) {
                            return false;
                        }
                    }
                }
            }
        }
        true
    }

    /// Whether every receiver in the metre rectangle is outdoors without reflection: no edge
    /// in its reflection windows or containment walks. A footprint can cover the rectangle
    /// without an edge inside it, so the eastward walk still extends by the widest ring.
    pub fn is_open_rectangle(&self, low: [f64; 2], high: [f64; 2]) -> bool {
        let a = self.lattice.steps(low);
        let b = self.lattice.steps(high);
        let half_a = self.reflection_window_half_steps(a);
        let half_b = self.reflection_window_half_steps(b);
        let padding = [0, 1].map(|axis| {
            half_a[axis].max(half_b[axis])
                + super::reflection_coverage::REFLECTION_LATTICE_STEPS as f64
        });
        let first = cell_of([0, 1].map(|axis| a[axis].min(b[axis]) - padding[axis]));
        let mut last = cell_of([0, 1].map(|axis| a[axis].max(b[axis]) + padding[axis]));
        // The coverage's bilinear prefix can read just beyond a reflection window. Unknown
        // cells prevent the shortcut; the ordinary receiver still checks the cells it needs.
        last[0] += (self.widest_outline_steps + 2 * CELL_STEPS - 1) / CELL_STEPS;
        for row in first[1]..=last[1] {
            for column in first[0]..=last[0] {
                let Ok(cell) = self.tile_at([column, row]) else {
                    return false;
                };
                if let Some((tile, index)) = cell
                    && !tile.obstacles.cell_run_range(index).is_empty()
                {
                    return false;
                }
            }
        }
        true
    }

    /// Crossing parity of every footprint taller than `min_height_m` along the eastward half-line
    /// from `point` (scene steps). Holes share their footprint's id, so a courtyard reads outside;
    /// a vertex on the half-line counts by the half-open rule `(y0 > y) != (y1 > y)` (transit
    /// once, tangent twice or never); an edge counts only in the cell holding its crossing (it is
    /// listed in every cell it passes); rings starting east of the point are skipped, so the
    /// walk, as long as the widest ring plus a cell, sees every crossing of the others.
    fn footprint_parities(
        &self,
        point: [f64; 2],
        min_height_m: f64,
        parities: &mut Vec<Parity>,
    ) -> Result<(), String> {
        parities.clear();
        let x = point[0];
        let cell_steps = CELL_STEPS as f64;
        let [first_column, row] = cell_of(point);
        let reach = (self.widest_outline_steps + CELL_STEPS) as f64;
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
                let west = (tile.offset[0] + i64::from(tile.outline_west[outline])) as f64;
                if !record.kind.is_building() || record.height_m <= min_height_m || west > x {
                    continue;
                }
                let first = record.first_vertex + usize::from(run.first_edge);
                let mut a = tile.vertex(first);
                for next in first + 1..=first + usize::from(run.edge_count) {
                    let b = tile.vertex(next);
                    let crossing_x = eastward_crossing_x(a, b, point);
                    a = b;
                    let Some(crossing_x) = crossing_x else {
                        continue;
                    };
                    if crossing_x < cell_west || crossing_x >= cell_east {
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
    /// (scene steps): the direct definition the parity and reflection lattice tests check against.
    #[cfg(test)]
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
        let Some(winner) = tallest_enclosed(&parities) else {
            return Ok(None);
        };
        let footprint = match self.building_parts(winner.footprint_id) {
            BuildingParts::One(footprint) => footprint,
            BuildingParts::Locate(mut parts) => self
                .part_at_indoor_point(&parts, point)
                .map(|index| parts.swap_remove(index))
                // A stray courtyard can give odd parity outside every exterior. No façade is known.
                .unwrap_or_else(|| Footprint {
                    id: winner.footprint_id,
                    height_m: winner.height_m,
                    envelope: winner.envelope,
                    single_part: false,
                    rings: Vec::new(),
                }),
        };
        Ok(Some(footprint))
    }
}

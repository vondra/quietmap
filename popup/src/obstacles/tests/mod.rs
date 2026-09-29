//! Fixtures: outlines drawn in metres around a click, stored whole in every tile whose cells they
//! cross (as the builder stores them), and a scene over the tiles within two rings of the click.

use super::crossings::{segment_intersection_t, sort_and_deduplicate};
use super::*;
use physics::ray::Crossing;
use std::collections::BTreeMap;
use tiles::geo::{GlobalSteps, Mercator};
use tiles::obstacles::{EnvelopeClass, Outline, OutlineKind, encode, tiles_crossed};

mod containment;
mod crossings;
mod facades;
mod scene;
mod skyline;

/// dev4's obstacle test origin.
const LAT: f64 = 50.0;
const LON: f64 = 14.0;

struct StockOutline {
    footprint_id: u64,
    kind: OutlineKind,
    envelope: EnvelopeClass,
    height_m: f64,
    vertices: Vec<GlobalSteps>,
}

/// Outlines on the global step lattice around a click placed exactly on a step, so a length of
/// `L` metres is stored as `round(L / step)` steps.
struct Stock {
    frame: LocalFrame,
    outlines: Vec<StockOutline>,
}

impl Stock {
    fn at(lat: f64, lon: f64) -> Self {
        let origin = GlobalSteps::nearest(Mercator::from_degrees(lat, lon)).to_mercator();
        Stock {
            frame: LocalFrame::at(origin),
            outlines: Vec::new(),
        }
    }

    fn new() -> Self {
        Stock::at(LAT, LON)
    }

    /// The nearest step to a point, taken continuously across the antimeridian.
    fn steps(&self, metres: [f64; 2]) -> GlobalSteps {
        let [x, y] = self.frame.steps_of_metres(metres);
        GlobalSteps {
            x: x.round() as i64,
            y: y.round() as i64,
        }
    }

    /// Where a point is stored: the metres of its nearest step.
    fn snapped(&self, metres: [f64; 2]) -> [f64; 2] {
        let steps = self.steps(metres);
        self.frame.metres_of_steps([steps.x as f64, steps.y as f64])
    }

    fn push(
        &mut self,
        footprint_id: u64,
        kind: OutlineKind,
        envelope: EnvelopeClass,
        height_m: f64,
        mut vertices: Vec<GlobalSteps>,
    ) {
        if kind.is_building() {
            vertices.push(vertices[0]);
        }
        self.outlines.push(StockOutline {
            footprint_id,
            kind,
            envelope,
            height_m,
            vertices,
        });
    }

    /// A building of class `Residential`: its first ring the exterior, the others holes.
    fn building(&mut self, footprint_id: u64, height_m: f64, rings: &[Vec<[f64; 2]>]) {
        self.building_of_class(footprint_id, height_m, EnvelopeClass::Residential, rings);
    }

    fn building_of_class(
        &mut self,
        footprint_id: u64,
        height_m: f64,
        envelope: EnvelopeClass,
        rings: &[Vec<[f64; 2]>],
    ) {
        for (index, ring) in rings.iter().enumerate() {
            let kind = if index == 0 {
                OutlineKind::Exterior
            } else {
                OutlineKind::Hole
            };
            let vertices = ring.iter().map(|&point| self.steps(point)).collect();
            self.push(footprint_id, kind, envelope, height_m, vertices);
        }
    }

    /// A ring given directly on the lattice.
    fn building_in_steps(&mut self, footprint_id: u64, height_m: f64, ring: Vec<GlobalSteps>) {
        self.push(
            footprint_id,
            OutlineKind::Exterior,
            EnvelopeClass::Residential,
            height_m,
            ring,
        );
    }

    fn wall(&mut self, footprint_id: u64, height_m: f64, points: &[[f64; 2]]) {
        let vertices = points.iter().map(|&point| self.steps(point)).collect();
        self.push(
            footprint_id,
            OutlineKind::Wall,
            EnvelopeClass::Outdoor,
            height_m,
            vertices,
        );
    }

    /// Every tile within two rings of the click: the bytes of those holding outlines, `None` for
    /// the empty ones.
    fn files(&self) -> Vec<(TileId, Option<Vec<u8>>)> {
        let centre = TileId::containing(self.frame.origin);
        let mut tiles: BTreeMap<TileId, Vec<Outline>> = (0..=2)
            .flat_map(|ring| centre.ring(ring))
            .map(|tile| (tile, Vec::new()))
            .collect();
        let mut order: Vec<&StockOutline> = self.outlines.iter().collect();
        order.sort_by_key(|outline| outline.footprint_id);
        for outline in order {
            for tile in tiles_crossed(&outline.vertices) {
                let vertices = outline
                    .vertices
                    .iter()
                    .map(|&v| tile.local(v).expect("inside the margin"))
                    .collect();
                tiles.entry(tile).or_default().push(Outline {
                    footprint_id: outline.footprint_id,
                    kind: outline.kind,
                    envelope: outline.envelope,
                    height_m: outline.height_m,
                    vertices,
                });
            }
        }
        tiles
            .into_iter()
            .map(|(tile, outlines)| (tile, (!outlines.is_empty()).then(|| encode(&outlines))))
            .collect()
    }

    fn with_scene<R>(&self, check: impl FnOnce(&Scene) -> R) -> R {
        let files = self.files();
        let mut scene = Scene::new(self.frame);
        for (tile, bytes) in &files {
            match bytes {
                Some(bytes) => scene.insert(*tile, Obstacles::parse(bytes).unwrap()),
                None => scene.insert_empty(*tile),
            }
        }
        check(&scene)
    }
}

/// The corners of an axis-aligned square, counter-clockwise from the south-west.
fn square(east: f64, north: f64, half: f64) -> Vec<[f64; 2]> {
    rectangle(east - half, north - half, east + half, north + half)
}

fn rectangle(west: f64, south: f64, east: f64, north: f64) -> Vec<[f64; 2]> {
    vec![[west, south], [east, south], [east, north], [west, north]]
}

fn crossings_of(scene: &Scene, from: [f64; 2], to: [f64; 2]) -> Vec<Crossing> {
    let mut out = Vec::new();
    scene.crossings(from, to, &mut out).unwrap();
    out
}

/// The reference: the ray against every edge of every read tile, same deduplication.
fn every_crossing(scene: &Scene, from: [f64; 2], to: [f64; 2]) -> Vec<Crossing> {
    let (start, end) = (scene.lattice.steps(from), scene.lattice.steps(to));
    let delta = [end[0] - start[0], end[1] - start[1]];
    let mut out = Vec::new();
    for (_, tile) in scene.read_tiles() {
        for index in 0..tile.obstacles.outline_count() {
            let record = tile.obstacles.outline(index);
            for v in record.first_vertex..record.first_vertex + record.vertex_count - 1 {
                if let Some(t) =
                    segment_intersection_t(start, delta, tile.vertex(v), tile.vertex(v + 1))
                {
                    out.push(Crossing {
                        t,
                        height_m: record.height_m,
                        building: record.kind.is_building(),
                        footprint_id: record.footprint_id,
                    });
                }
            }
        }
    }
    sort_and_deduplicate(&mut out);
    out
}

/// Scene steps of a stored global position (for probes exactly on a vertex row).
fn scene_steps(scene: &Scene, global: GlobalSteps) -> [f64; 2] {
    let corner = scene
        .lattice
        .origin_cell
        .map(|cell| cell * tiles::obstacles::CELL_STEPS);
    [(global.x - corner[0]) as f64, (global.y - corner[1]) as f64]
}

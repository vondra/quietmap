//! The ring loop: read the clicked tile and ring 1 whole, answer, then ring after ring until the
//! reach is covered. Every ring's candidates join their layer's selection, which evaluates from the
//! loudest bound until the bounds of everything left out stay below (10^(0.1/10) - 1) times the
//! energy evaluated, per period and across all rings.

use crate::candidates::{
    Candidate, DisplayRef, GROUND_REACH_M, SourceAttribute, TileCandidates, collect, lden_weighted,
};
use crate::evaluate::{Receiver, Scratch, received_energy, trace};
use crate::obstacles::Scene;
use crate::release::{Release, RingFiles};
use crate::scene::Ground;
use physics::bands::{BANDS, PERIODS, energy};
use physics::bound::receiver_gain;
use rayon::prelude::*;
use std::cell::OnceCell;
use std::collections::HashMap;
use tiles::geo::{LocalFrame, Mercator, TileId};
use tiles::sources::{Layer, Sources, display_fields};
use tiles::terrain::Terrain;
use tiles::{Kind, obstacles::Obstacles};

/// Receiver height above the ground (END assessment height).
pub const RECEIVER_HEIGHT_M: f64 = 4.0;
/// The omitted energy may reach this fraction of the evaluated energy: 0.1 dB.
pub const OMITTED_ENERGY_FRACTION: f64 = 0.023_292_992_280_754_13;
/// Contributors listed per answer.
pub const CONTRIBUTORS_SHOWN: usize = 30;
/// The fewest candidates one round evaluates per layer, so the pool stays busy.
const MINIMUM_BATCH: usize = 16;

pub struct Options {
    /// Evaluate every candidate (the benchmark reference); the stop rule is off.
    pub exact: bool,
    /// List this many loudest evaluated pieces per layer with the final update (the benchmark's
    /// piece-by-piece comparison with dev4); 0 lists none.
    pub pieces: usize,
}

/// One evaluated piece, kept only when `Options::pieces` asks for them.
#[derive(Clone)]
pub struct EvaluatedPiece {
    pub layer: Layer,
    pub ends_m: [[f64; 2]; 2],
    pub distance_m: f64,
    pub energy: [f64; PERIODS],
    /// A-weighted emission per period (per metre for lines), linear.
    pub emission: [f64; PERIODS],
    pub group_key: u64,
    /// Buildings and walls crossed by the ray from the piece's closest point: distance from the
    /// receiver (m) and height (m), filled when listed.
    pub crossings: Vec<(f64, f64)>,
    /// Index into the click's attribute list.
    pub attribute: usize,
    /// The ray from the closest point, filled when listed: slant (m), favourable probability
    /// per period, and per state (homogeneous, favourable) the boundary, foliage and air
    /// attenuation as A-weighted over the day emission spectrum (dB), and the path difference.
    pub trace: Option<PieceTrace>,
}

#[derive(Clone)]
pub struct PieceTrace {
    pub slant_m: f64,
    pub favourable_probability: [f64; PERIODS],
    pub boundary_db: [f64; 2],
    pub without_ground_db: [f64; 2],
    pub foliage_db: [f64; 2],
    pub air_db: f64,
    pub path_difference_m: [f64; 2],
}

/// The point of the segment `a`-`b` closest to the origin.
fn closest_point(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length_sq = dx * dx + dy * dy;
    let t = if length_sq > 0.0 {
        (-(a[0] * dx + a[1] * dy) / length_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    [a[0] + t * dx, a[1] + t * dy]
}

/// A layer's state after a ring.
pub struct LayerAnswer {
    pub layer: Layer,
    pub energy: [f64; PERIODS],
    /// Sum of the bounds of the candidates left out.
    pub omitted_bound: [f64; PERIODS],
    pub evaluated: usize,
    pub candidates: usize,
}

/// One contributor group (sources sharing a display group key).
#[derive(Clone)]
pub struct Contributor {
    pub group_key: u64,
    pub layer: Layer,
    pub energy: [f64; PERIODS],
    pub distance_m: f64,
    pub display: DisplayRef,
}

pub struct Statistics {
    pub rings_read: u32,
    pub files: usize,
    pub bytes: u64,
    pub read_seconds: f64,
    /// Parsing tiles and bounding candidates.
    pub candidate_seconds: f64,
    /// The full physics of the selected candidates.
    pub evaluate_seconds: f64,
    pub elapsed_seconds: f64,
}

/// One streamed update.
pub struct Update<'u> {
    pub partial: bool,
    pub lat: f64,
    pub lon: f64,
    pub frame: LocalFrame,
    pub receiver_altitude_m: f64,
    /// The enclosed building the click stands in (footprint id, height), if any.
    pub building: Option<(u64, f64)>,
    pub layers: Vec<LayerAnswer>,
    pub contributors: Vec<Contributor>,
    /// The loudest evaluated pieces per layer (final update, when asked for).
    pub pieces: Vec<EvaluatedPiece>,
    pub statistics: Statistics,
    /// The display JSON of a contributor.
    pub display_json: &'u dyn Fn(DisplayRef, Layer) -> Result<String, String>,
}

struct LayerSelection {
    layer: Layer,
    pending: Vec<Candidate>,
    energy: [f64; PERIODS],
    evaluated: usize,
    contributors: HashMap<u64, Contributor>,
    pieces: Vec<EvaluatedPiece>,
}

impl LayerSelection {
    fn pending_bound(&self) -> [f64; PERIODS] {
        let mut sum = [0.0; PERIODS];
        for candidate in &self.pending {
            for (total, bound) in sum.iter_mut().zip(candidate.bound) {
                *total += bound;
            }
        }
        sum
    }

    /// The fewest loudest candidates whose evaluation could satisfy the rule: as if each
    /// delivered its whole bound, the most any can.
    fn fewest_to_satisfy(&self) -> usize {
        let mut remaining = self.pending_bound();
        let mut evaluated = self.energy;
        for (taken, candidate) in self.pending.iter().rev().enumerate() {
            if (0..PERIODS).all(|p| remaining[p] <= OMITTED_ENERGY_FRACTION * evaluated[p]) {
                return taken;
            }
            for period in 0..PERIODS {
                remaining[period] -= candidate.bound[period];
                evaluated[period] += candidate.bound[period];
            }
        }
        self.pending.len()
    }

    fn satisfied(&self, exact: bool) -> bool {
        if exact {
            return self.pending.is_empty();
        }
        let omitted = self.pending_bound();
        (0..PERIODS).all(|period| omitted[period] <= OMITTED_ENERGY_FRACTION * self.energy[period])
    }
}

/// Distance from the receiver to the edge of the block of rings up to `ring` (metres).
fn covered_radius_m(frame: &LocalFrame, centre: TileId, ring: u32) -> f64 {
    let (k, origin) = (f64::from(ring), frame.origin);
    let west = (origin.x - (f64::from(centre.x) - k)) * frame.east_m_per_unit;
    let east = (f64::from(centre.x) + k + 1.0 - origin.x) * frame.east_m_per_unit;
    let north = (origin.y - (f64::from(centre.y) - k)).max(0.0) * frame.north_m_per_unit;
    let south = (f64::from(centre.y) + k + 1.0 - origin.y) * frame.north_m_per_unit;
    let north = if f64::from(centre.y) - k <= 0.0 {
        f64::INFINITY
    } else {
        north
    };
    let south = if f64::from(centre.y) + k + 1.0 >= f64::from(tiles::geo::TILES_PER_AXIS) {
        f64::INFINITY
    } else {
        south
    };
    west.min(east).min(north).min(south)
}

/// Answers a click, calling `emit` after the first read (tile + ring 1) and after every further ring.
pub fn answer(
    release: &Release,
    lat: f64,
    lon: f64,
    options: &Options,
    emit: &mut dyn FnMut(&Update) -> Result<(), String>,
) -> Result<(), String> {
    let started = std::time::Instant::now();
    let origin = Mercator::from_degrees(lat, lon);
    let frame = LocalFrame::at(origin);
    let centre = TileId::containing(origin);
    let max_ring = (1..)
        .find(|&ring| covered_radius_m(&frame, centre, ring) >= GROUND_REACH_M)
        .unwrap_or(1);
    let rings: Vec<OnceCell<RingFiles>> = (0..=max_ring).map(|_| OnceCell::new()).collect();
    let kinds = vec![Kind::Terrain, Kind::Obstacles, Kind::Sources];
    let mut ground = Ground::new(frame, centre, max_ring);
    let mut obstacles = Scene::new(frame);
    let mut selections: Vec<LayerSelection> = Layer::ALL
        .iter()
        .map(|&layer| LayerSelection {
            layer,
            pending: Vec::new(),
            energy: [0.0; PERIODS],
            evaluated: 0,
            contributors: HashMap::new(),
            pieces: Vec::new(),
        })
        .collect();
    let (mut files, mut bytes, mut read_seconds) = (0usize, 0u64, 0.0f64);
    let (mut candidate_seconds, mut evaluate_seconds) = (0.0f64, 0.0f64);
    let mut receiver = None;
    let mut building = None;
    let mut attributes: Vec<SourceAttribute> = Vec::new();
    for ring in 1..=max_ring {
        let tiles = if ring == 1 {
            [centre.ring(0), centre.ring(1)].concat()
        } else {
            centre.ring(ring)
        };
        let cell = &rings[ring as usize];
        let _ = cell.set(RingFiles::read(release, tiles, kinds.clone())?);
        let read = cell.get().expect("the ring was just read");
        (files, bytes, read_seconds) = (
            files + read.file_count,
            bytes + read.bytes,
            read_seconds + read.read_seconds,
        );
        let candidates_started = std::time::Instant::now();
        let mut ring_sources = Vec::new();
        for (index, &tile) in read.tiles.iter().enumerate() {
            let terrain = read
                .file(index, Kind::Terrain)
                .map(Terrain::parse)
                .transpose()
                .map_err(|e| e.to_string())?;
            ground.insert(tile, terrain);
            if let Some(bytes) = read.file(index, Kind::Obstacles) {
                obstacles.insert(tile, Obstacles::parse(bytes).map_err(|e| e.to_string())?);
            } else {
                obstacles.insert_empty(tile);
            }
            if let Some(bytes) = read.file(index, Kind::Sources) {
                ring_sources.push((
                    index,
                    tile,
                    Sources::parse(bytes).map_err(|e| e.to_string())?,
                ));
            }
        }
        if receiver.is_none() {
            let altitude_m = ground.at([0.0, 0.0])?.height_m + RECEIVER_HEIGHT_M;
            let reflection_db = obstacles.reflection_db([0.0, 0.0], None)?;
            receiver = Some((altitude_m, release.weather.at(lat, lon), reflection_db));
            building = obstacles
                .enclosing_building([0.0, 0.0])?
                .map(|footprint| (footprint.id, footprint.height_m));
        }
        let (altitude_m, weather, reflection_db) = receiver.expect("set after the first read");
        let gain = receiver_gain(weather.maximum(), reflection_db);
        let collected: Vec<Result<TileCandidates, String>> = ring_sources
            .par_iter()
            .map(|(tile_index, tile, sources)| {
                collect(sources, *tile, (ring as usize, *tile_index), &ground, &gain)
            })
            .collect();
        for result in collected {
            let (tile_attributes, candidates) = result?;
            let first = attributes.len();
            attributes.extend(tile_attributes);
            for mut candidate in candidates {
                candidate.attribute += first;
                selections[candidate.layer as usize].pending.push(candidate);
            }
        }
        candidate_seconds += candidates_started.elapsed().as_secs_f64();
        let evaluate_started = std::time::Instant::now();
        let evaluation = Receiver {
            ground: &ground,
            obstacles: &obstacles,
            altitude_m,
            weather,
            reflection_db,
        };
        select(&mut selections, &evaluation, &attributes, options)?;
        evaluate_seconds += evaluate_started.elapsed().as_secs_f64();
        let display_json = |display: DisplayRef, layer: Layer| -> Result<String, String> {
            let read = rings[display.ring]
                .get()
                .ok_or("display of an unread ring")?;
            let bytes = read
                .file(display.tile, Kind::Sources)
                .ok_or("display of an absent sources file")?;
            let sources = Sources::parse(bytes).map_err(|e| e.to_string())?;
            let values: Vec<serde_json::Value> = serde_json::from_str(
                sources
                    .display(display.attribute)
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            let object: serde_json::Map<String, serde_json::Value> = display_fields(layer)
                .iter()
                .map(|field| field.to_string())
                .zip(values)
                .collect();
            Ok(serde_json::Value::Object(object).to_string())
        };
        let mut contributors: Vec<Contributor> = selections
            .iter()
            .flat_map(|selection| selection.contributors.values().cloned())
            .collect();
        contributors.sort_by(|a, b| {
            lden_weighted(&b.energy)
                .total_cmp(&lden_weighted(&a.energy))
                .then(a.group_key.cmp(&b.group_key))
        });
        contributors.truncate(CONTRIBUTORS_SHOWN);
        let mut pieces = Vec::new();
        if ring == max_ring {
            for selection in &mut selections {
                selection
                    .pieces
                    .sort_by(|a, b| lden_weighted(&b.energy).total_cmp(&lden_weighted(&a.energy)));
                for piece in selection.pieces.iter().take(options.pieces) {
                    let mut piece = piece.clone();
                    let from = closest_point(piece.ends_m[0], piece.ends_m[1]);
                    let mut crossings = Vec::new();
                    obstacles.crossings(from, [0.0, 0.0], &mut crossings)?;
                    let length = from[0].hypot(from[1]);
                    piece.crossings = crossings
                        .iter()
                        .map(|crossing| ((1.0 - crossing.t) * length, crossing.height_m))
                        .collect();
                    let source = &attributes[piece.attribute];
                    let terms = trace(&evaluation, from, source, &mut Scratch::default())?;
                    let spectrum = source.energy[0];
                    let weighted = |attenuation: &[f64; BANDS]| {
                        let total: f64 = spectrum.iter().sum();
                        let passed: f64 = (0..BANDS)
                            .map(|band| spectrum[band] * energy(-attenuation[band]))
                            .sum();
                        -10.0 * (passed / total).log10()
                    };
                    let azimuth = (-from[1]).atan2(-from[0]);
                    piece.trace = Some(PieceTrace {
                        slant_m: terms.transfer.slant_m,
                        favourable_probability: std::array::from_fn(|period| {
                            evaluation.weather.at(period, azimuth)
                        }),
                        boundary_db: [0, 1]
                            .map(|state| weighted(&terms.boundaries[state].attenuation_db)),
                        without_ground_db: [0, 1]
                            .map(|state| weighted(&terms.boundaries[state].without_ground_db)),
                        foliage_db: [0, 1].map(|state| weighted(&terms.foliage_db[state])),
                        air_db: weighted(&terms.air_db),
                        path_difference_m: [0, 1]
                            .map(|state| terms.boundaries[state].path_difference_m),
                    });
                    pieces.push(piece);
                }
            }
        }
        let update = Update {
            partial: ring < max_ring,
            lat,
            lon,
            frame,
            receiver_altitude_m: altitude_m,
            building,
            layers: selections
                .iter()
                .map(|selection| LayerAnswer {
                    layer: selection.layer,
                    energy: selection.energy,
                    omitted_bound: selection.pending_bound(),
                    evaluated: selection.evaluated,
                    candidates: selection.evaluated + selection.pending.len(),
                })
                .collect(),
            contributors,
            pieces,
            statistics: Statistics {
                rings_read: ring,
                files,
                bytes,
                read_seconds,
                candidate_seconds,
                evaluate_seconds,
                elapsed_seconds: started.elapsed().as_secs_f64(),
            },
            display_json: &display_json,
        };
        emit(&update)?;
    }
    Ok(())
}

/// Evaluates candidates from the loudest bound, in parallel batches over all unsatisfied layers,
/// until every layer's omitted-energy account allows it to stop.
fn select(
    selections: &mut [LayerSelection],
    receiver: &Receiver,
    attributes: &[SourceAttribute],
    options: &Options,
) -> Result<(), String> {
    let exact = options.exact;
    for selection in selections.iter_mut() {
        selection
            .pending
            .sort_by(|a, b| a.order.total_cmp(&b.order));
    }
    loop {
        let mut work: Vec<(usize, Candidate)> = Vec::new();
        for (layer, selection) in selections.iter_mut().enumerate() {
            if !selection.satisfied(exact) {
                let take = if exact {
                    selection.pending.len()
                } else {
                    selection.fewest_to_satisfy().max(MINIMUM_BATCH)
                };
                let start = selection.pending.len() - take.min(selection.pending.len());
                work.extend(
                    selection
                        .pending
                        .drain(start..)
                        .map(|candidate| (layer, candidate)),
                );
            }
        }
        if work.is_empty() {
            return Ok(());
        }
        let energies: Vec<Result<[f64; PERIODS], String>> = work
            .par_iter()
            .map_init(Scratch::default, |scratch, (_, candidate)| {
                received_energy(
                    receiver,
                    candidate,
                    &attributes[candidate.attribute],
                    scratch,
                )
            })
            .collect();
        for ((layer, candidate), energy) in work.into_iter().zip(energies) {
            let energy = energy?;
            let selection = &mut selections[layer];
            selection.evaluated += 1;
            for (total, value) in selection.energy.iter_mut().zip(energy) {
                *total += value;
            }
            let contributor = selection
                .contributors
                .entry(candidate.group_key)
                .or_insert_with(|| Contributor {
                    group_key: candidate.group_key,
                    layer: candidate.layer,
                    energy: [0.0; PERIODS],
                    distance_m: candidate.distance_m,
                    display: candidate.display,
                });
            for (total, value) in contributor.energy.iter_mut().zip(energy) {
                *total += value;
            }
            contributor.distance_m = contributor.distance_m.min(candidate.distance_m);
            if options.pieces > 0 {
                let emission = attributes[candidate.attribute]
                    .energy
                    .map(|bands| bands.iter().sum());
                selection.pieces.push(EvaluatedPiece {
                    layer: candidate.layer,
                    ends_m: candidate.ends_m,
                    distance_m: candidate.distance_m,
                    energy,
                    emission,
                    group_key: candidate.group_key,
                    crossings: Vec::new(),
                    attribute: candidate.attribute,
                    trace: None,
                });
            }
        }
    }
}

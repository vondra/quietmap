//! The ring loop: read the clicked tile and ring 1 whole, answer, then ring after ring until the
//! reach is covered. Every ring's candidates join their layer's selection ([`crate::selection`]).
//! A click inside a building is answered at its loudest façade ([`crate::building`]).

use crate::building::{BuildingClick, loudest_facade};
use crate::candidates::{
    Attributes, Candidate, DisplayRef, GROUND_REACH_M, TileCandidates, collect,
};
use crate::evaluate::Receiver;
use crate::listing::list_pieces;
use crate::obstacles::Scene;
use crate::release::{Release, RingFiles};
use crate::scene::Ground;
use crate::selection::{LayerSelection, select};
use crate::update::{Statistics, Update, empty_answer, layer_answers, loudest_contributors};
use physics::bound::receiver_gain;
use physics::weather::FavourableProbability;
use rayon::prelude::*;
use std::cell::OnceCell;
use tiles::geo::{LocalFrame, Mercator, TileId};
use tiles::sources::{Layer, Sources, display_fields};
use tiles::terrain::Terrain;
use tiles::{Kind, obstacles::Obstacles};

/// Receiver height above the ground (END assessment height).
pub const RECEIVER_HEIGHT_M: f64 = 4.0;

pub struct Options {
    /// Evaluate every candidate (the benchmark reference); the stop rule is off.
    pub exact: bool,
    /// List this many loudest evaluated pieces per layer with the final update (the benchmark's
    /// piece-by-piece comparison with dev4); 0 lists none.
    pub pieces: usize,
}

/// Where the click is answered: the click itself or a building's façade.
#[derive(Clone, Copy)]
struct Station {
    position: [f64; 2],
    altitude_m: f64,
    weather: FavourableProbability,
    reflection_db: f64,
}

/// Distance from `receiver` (click metres) to the edge of the block of rings up to `ring`.
fn covered_radius_m(frame: &LocalFrame, centre: TileId, ring: u32, receiver: [f64; 2]) -> f64 {
    let k = f64::from(ring);
    let origin = frame.to_mercator(receiver);
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

/// The rings to read for a receiver at `receiver`: until the reach is covered.
fn rings_needed(frame: &LocalFrame, centre: TileId, receiver: [f64; 2]) -> u32 {
    (1..)
        .find(|&ring| covered_radius_m(frame, centre, ring, receiver) >= GROUND_REACH_M)
        .unwrap_or(1)
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
    // A façade receiver stands at most a ring further out than the click needs.
    let most_rings = rings_needed(&frame, centre, [0.0, 0.0]) + 1;
    let mut max_ring = most_rings - 1;
    let rings: Vec<OnceCell<RingFiles>> = (0..=most_rings).map(|_| OnceCell::new()).collect();
    let kinds = vec![Kind::Terrain, Kind::Obstacles, Kind::Sources];
    let mut ground = Ground::new(frame, centre, most_rings);
    let mut obstacles = Scene::new(frame);
    let mut selections: Vec<LayerSelection> = Layer::ALL
        .iter()
        .map(|&layer| LayerSelection::new(layer))
        .collect();
    let (mut files, mut bytes, mut read_seconds) = (0usize, 0u64, 0.0f64);
    let (mut candidate_seconds, mut evaluate_seconds) = (0.0f64, 0.0f64);
    let mut station: Option<Station> = None;
    let mut building: Option<BuildingClick> = None;
    let mut attributes = Attributes::default();
    let weather = release.weather.at(lat, lon);
    let mut ring = 0;
    while ring < max_ring {
        ring += 1;
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
        let enclosing = if ring == 1 {
            obstacles.enclosing_building([0.0, 0.0])?
        } else {
            None
        };
        if ring == 1 && enclosing.is_none() {
            station = Some(Station {
                position: [0.0, 0.0],
                altitude_m: ground.at([0.0, 0.0])?.height_m + RECEIVER_HEIGHT_M,
                weather,
                reflection_db: obstacles.reflection_db([0.0, 0.0], None)?,
            });
        }
        let facade_receivers = match &enclosing {
            Some(footprint) => obstacles.facade_receivers(footprint)?,
            None => Vec::new(),
        };
        if let Some(footprint) = &enclosing
            && facade_receivers.is_empty()
        {
            // No exposed façade: the building is not assessed.
            let click = BuildingClick {
                footprint_id: footprint.id,
                height_m: footprint.height_m,
                receivers: 0,
                facade: None,
            };
            return empty_answer(
                lat,
                lon,
                frame,
                &ground,
                click,
                &selections,
                (files, bytes, read_seconds, started),
                emit,
            );
        }
        // Before a building's façade is chosen the candidates are collected around the click,
        // wide enough for any of its façades, and bounded again at the chosen one.
        let (collect_at, reach, reflection_db) = match station {
            Some(station) => (station.position, GROUND_REACH_M, station.reflection_db),
            None => (
                [0.0, 0.0],
                GROUND_REACH_M
                    + facade_receivers
                        .iter()
                        .map(|facade| facade.position[0].hypot(facade.position[1]))
                        .fold(0.0, f64::max),
                0.0,
            ),
        };
        let collect_gain = receiver_gain(weather.maximum(), reflection_db);
        let collected: Vec<Result<TileCandidates, String>> = ring_sources
            .par_iter()
            .map(|(tile_index, tile, sources)| {
                collect(
                    sources,
                    *tile,
                    (ring as u16, *tile_index as u16),
                    &ground,
                    collect_at,
                    reach,
                    &collect_gain,
                )
            })
            .collect();
        for result in collected {
            let (tile_attributes, candidates) = result?;
            let list = attributes.push(tile_attributes);
            for mut candidate in candidates {
                candidate.attribute.list = list;
                selections[candidate.layer as usize].pending.push(candidate);
            }
        }
        candidate_seconds += candidates_started.elapsed().as_secs_f64();
        let evaluate_started = std::time::Instant::now();
        if let Some(footprint) = &enclosing {
            let everything: Vec<&Candidate> = selections
                .iter()
                .flat_map(|selection| selection.pending.iter())
                .collect();
            let facade = loudest_facade(
                footprint,
                &facade_receivers,
                &everything,
                &attributes,
                &ground,
                &obstacles,
                weather,
            )?;
            building = Some(BuildingClick {
                footprint_id: footprint.id,
                height_m: footprint.height_m,
                receivers: facade_receivers.len(),
                facade: Some(facade),
            });
            let chosen = Station {
                position: facade.position,
                altitude_m: facade.altitude_m,
                weather,
                reflection_db: facade.reflection_db,
            };
            let gain = receiver_gain(weather.maximum(), chosen.reflection_db);
            for selection in &mut selections {
                selection.pending.retain_mut(|candidate| {
                    candidate.bound_at(chosen.position, &attributes[candidate.attribute], &gain)
                });
            }
            max_ring = rings_needed(&frame, centre, chosen.position).min(most_rings);
            station = Some(chosen);
        }
        let station = station.expect("chosen after the first read");
        let evaluation = Receiver {
            ground: &ground,
            obstacles: &obstacles,
            position: station.position,
            altitude_m: station.altitude_m,
            weather: station.weather,
            reflection_db: station.reflection_db,
        };
        select(
            &mut selections,
            &evaluation,
            &attributes,
            options.exact,
            options.pieces > 0,
        )?;
        evaluate_seconds += evaluate_started.elapsed().as_secs_f64();
        let pieces = if ring == max_ring && options.pieces > 0 {
            list_pieces(&mut selections, options.pieces, &evaluation, &attributes)?
        } else {
            Vec::new()
        };
        let display_json = |display: DisplayRef, layer: Layer| -> Result<String, String> {
            let read = rings[usize::from(display.ring)]
                .get()
                .ok_or("display of an unread ring")?;
            let bytes = read
                .file(usize::from(display.tile), Kind::Sources)
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
        let update = Update {
            partial: ring < max_ring,
            lat,
            lon,
            frame,
            receiver_altitude_m: station.altitude_m,
            reflection_db: station.reflection_db,
            building,
            layers: layer_answers(&selections),
            contributors: loudest_contributors(&selections),
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

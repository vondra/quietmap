//! The ring loop: read the clicked tile and ring 1 whole, answer, then ring after ring until the
//! reach is covered. Every ring's candidates join their layer's selection ([`crate::selection`]).
//! A click inside a building is answered at its loudest façade ([`crate::building`]).

use crate::aircraft::boxes::{AIRCRAFT_REACH_M, AircraftReceiver};
use crate::aircraft::flights::FlightTotals;
use crate::aircraft::horizons::Horizons;
use crate::aircraft::{reads_fine_boxes, ring_aircraft};
use crate::building::{BuildingClick, loudest_facade};
use crate::candidates::{
    Attributes, Candidate, DisplayRef, GROUND_REACH_M, TileCandidates, collect,
};
use crate::evaluate::Receiver;
use crate::lines::whole_lines;
use crate::listing::list_pieces;
use crate::obstacles::Scene;
use crate::release::{Release, RingFiles};
use crate::scene::Ground;
use crate::selection::{LayerSelection, select};
use crate::update::{Statistics, Update, empty_answer, layer_answers, loudest_contributors};
use physics::bands::PERIODS;
use physics::bound::receiver_bound;
use physics::doc29::atmosphere::{class_spectrum_at, place_rates_db_per_m};
use physics::doc29::profiles_generated::{noise_class_of, profile_idx};
use physics::weather::PlaceWeather;
use rayon::prelude::*;
use std::cell::OnceCell;
use tiles::aircraft::Aircraft;
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
    weather: PlaceWeather,
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

/// The rings to read for a receiver at `receiver`: until `reach_m` is covered.
fn rings_needed(frame: &LocalFrame, centre: TileId, receiver: [f64; 2], reach_m: f64) -> u32 {
    (1..)
        .find(|&ring| covered_radius_m(frame, centre, ring, receiver) >= reach_m)
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
    // A façade receiver stands at most a ring further out than the click needs. Ground kinds are
    // read to the ground reach, aircraft to theirs.
    let most_ground_rings = rings_needed(&frame, centre, [0.0, 0.0], GROUND_REACH_M) + 1;
    let most_aircraft_rings = rings_needed(&frame, centre, [0.0, 0.0], AIRCRAFT_REACH_M) + 1;
    let mut ground_rings = most_ground_rings - 1;
    let mut aircraft_rings = most_aircraft_rings - 1;
    let rings: Vec<OnceCell<RingFiles>> = (0..=most_ground_rings.max(most_aircraft_rings))
        .map(|_| OnceCell::new())
        .collect();
    let mut ground = Ground::new(frame, centre, most_ground_rings);
    let mut horizons: Option<Horizons> = None;
    let weather = release.weather.place(lat, lon);
    let mut flights = FlightTotals::default().in_atmosphere(&weather.alpha_db_per_km);
    // The flights' energy and energy times lambda per period, for the percentile levels.
    let (mut flight_energy, mut flight_energy_lambda) = ([0.0; PERIODS], [0.0; PERIODS]);
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
    let mut ring = 0;
    while ring < ground_rings.max(aircraft_rings) {
        ring += 1;
        let tiles = if ring == 1 {
            [centre.ring(0), centre.ring(1)].concat()
        } else {
            centre.ring(ring)
        };
        let reads_ground = ring <= ground_rings;
        let mut kinds = Vec::new();
        if reads_ground {
            kinds.extend([Kind::Terrain, Kind::Obstacles, Kind::Sources]);
        }
        // Fine boxes near the click, far boxes elsewhere.
        if ring <= aircraft_rings {
            kinds.extend([Kind::Aircraft, Kind::AircraftFar]);
        }
        let fine = |tile: TileId| reads_fine_boxes(&frame, tile);
        let cell = &rings[ring as usize];
        let _ = cell.set(RingFiles::read(
            release,
            tiles,
            kinds,
            |tile, kind| match kind {
                Kind::Aircraft => fine(tile),
                Kind::AircraftFar => !fine(tile),
                _ => true,
            },
        )?);
        let read = cell.get().expect("the ring was just read");
        (files, bytes, read_seconds) = (
            files + read.file_count,
            bytes + read.bytes,
            read_seconds + read.read_seconds,
        );
        let candidates_started = std::time::Instant::now();
        type Parsed<'a> = (
            Option<Terrain<'a>>,
            Option<Obstacles<'a>>,
            Option<Sources<'a>>,
            Option<Aircraft<'a>>,
        );
        let parsed: Vec<Result<Parsed, String>> = (0..read.tiles.len())
            .into_par_iter()
            .map(|index| {
                let terrain = read
                    .file(index, Kind::Terrain)
                    .map(Terrain::parse)
                    .transpose();
                let obstacles = read
                    .file(index, Kind::Obstacles)
                    .map(Obstacles::parse)
                    .transpose();
                let sources = read
                    .file(index, Kind::Sources)
                    .map(Sources::parse)
                    .transpose();
                let aircraft = read
                    .file(index, Kind::Aircraft)
                    .or_else(|| read.file(index, Kind::AircraftFar))
                    .map(Aircraft::parse)
                    .transpose();
                Ok((
                    terrain.map_err(|e| e.to_string())?,
                    obstacles.map_err(|e| e.to_string())?,
                    sources.map_err(|e| e.to_string())?,
                    aircraft.map_err(|e| e.to_string())?,
                ))
            })
            .collect();
        let mut ring_sources = Vec::new();
        let mut ring_obstacles = Vec::new();
        let mut ring_aircraft_tiles = Vec::new();
        for (index, (&tile, parsed)) in read.tiles.iter().zip(parsed).enumerate() {
            let (terrain, tile_obstacles, sources, aircraft) = parsed?;
            if reads_ground {
                ground.insert(tile, terrain);
                ring_obstacles.push((tile, tile_obstacles));
            }
            if let Some(sources) = sources {
                ring_sources.push((index, tile, sources));
            }
            if let Some(aircraft) = aircraft {
                ring_aircraft_tiles.push((tile, aircraft));
            }
        }
        obstacles.insert_all(ring_obstacles);
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
        let collect_gain = receiver_bound(
            weather.favourable.maximum(),
            reflection_db,
            weather.alpha_db_per_km,
        );
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
            let gain = receiver_bound(
                weather.favourable.maximum(),
                chosen.reflection_db,
                weather.alpha_db_per_km,
            );
            for selection in &mut selections {
                selection.pending.retain_mut(|candidate| {
                    candidate.bound_at(chosen.position, &attributes[candidate.attribute], &gain)
                });
            }
            ground_rings = rings_needed(&frame, centre, chosen.position, GROUND_REACH_M)
                .min(most_ground_rings);
            aircraft_rings = rings_needed(&frame, centre, chosen.position, AIRCRAFT_REACH_M)
                .min(most_aircraft_rings);
            station = Some(chosen);
        }
        let station = station.expect("chosen after the first read");
        if reads_ground {
            horizons = Some(Horizons::build(
                &ground,
                &obstacles,
                station.position,
                station.altitude_m,
            )?);
        }
        let receiver = AircraftReceiver {
            position: station.position,
            altitude_m: station.altitude_m,
        };
        let horizons = horizons.as_ref().expect("built at the first read");
        // The ring's boxes join the aircraft layer before its sources (airport ground operations)
        // are selected: their energy counts in the layer's omitted-energy account.
        let aircraft = &mut selections[Layer::Aircraft as usize];
        let (energy, energy_lambda, heard) = ring_aircraft(
            &ring_aircraft_tiles,
            &frame,
            receiver,
            horizons,
            &mut flights,
        );
        for period in 0..PERIODS {
            aircraft.energy[period] += energy[period];
            flight_energy[period] += energy[period];
            flight_energy_lambda[period] += energy_lambda[period];
        }
        aircraft.evaluated += heard;
        aircraft.covered += heard;
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
            (lat.to_bits() ^ lon.to_bits().rotate_left(32)) ^ u64::from(ring),
        )?;
        evaluate_seconds += evaluate_started.elapsed().as_secs_f64();
        let last_ring = ring == ground_rings.max(aircraft_rings);
        let pieces = if last_ring && options.pieces > 0 {
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
        let fields = |contributor: &crate::update::Contributor| {
            display_json(contributor.display, contributor.layer)
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok())
        };
        // The time levels come with the final answer only (the partial ones do not show them).
        let percentiles = last_ring.then(|| {
            crate::percentiles::percentiles(
                &selections,
                (flight_energy, flight_energy_lambda),
                &fields,
                lat.to_bits() ^ lon.to_bits().rotate_left(32),
            )
        });
        let loudness = percentiles.map(|levels| {
            let flight = flights.loudest().into_iter().next();
            let spectrum_db = flight.and_then(|flight| {
                let class = noise_class_of(profile_idx(&flight.type_designator));
                class_spectrum_at(
                    usize::from(class),
                    flight.closest_m.hypot(flight.altitude_m),
                    &place_rates_db_per_m(&weather.alpha_db_per_km),
                )
            });
            crate::loudness::loudness(
                &selections,
                &crate::loudness::FlightSound {
                    energy: flight_energy,
                    spectrum_db,
                },
                (levels.l5, levels.l50),
                levels.road_intermittency,
            )
        });
        let mut contributors = loudest_contributors(&selections);
        if last_ring {
            let read: Vec<&RingFiles> = rings.iter().filter_map(OnceCell::get).collect();
            let keys: Vec<u64> = contributors.iter().map(|c| c.group_key).collect();
            let lines = whole_lines(&read, &frame, &keys, station.position, GROUND_REACH_M)?;
            for (contributor, lines) in contributors.iter_mut().zip(lines) {
                contributor.lines = lines;
            }
        }
        let update = Update {
            partial: !last_ring,
            percentiles,
            loudness,
            lat,
            lon,
            frame,
            receiver_altitude_m: station.altitude_m,
            reflection_db: station.reflection_db,
            building,
            layers: layer_answers(&selections),
            contributors,
            flights: flights.loudest(),
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

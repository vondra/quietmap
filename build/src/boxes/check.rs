//! The boxes against the full computation (PLAN section 7): at each benchmark point, every
//! segment of the window through the kernel, weighted as the boxes weigh it, against the written
//! boxes through the click-time equation, both unscreened (the screening is one rule applied
//! alike); per period Leq of each and their difference, also apart for the first band (near the
//! ground, where the data's altitude error dominates) and the bands above, each segment cut into
//! its box pieces as the builder cuts it. The top-flights list as the popup makes it (the kept
//! pieces of each ring's loudest boxes, reading K of them) against the ten loudest flights of the
//! exact sum. One pass over the days serves every point.

use super::place::{Placement, cut_into_pieces};
use super::read::{FLAG_SECONDARY_ONLY, FlightSegment, read_segments};
use super::{Window, emission_of};
use physics::bands::{PERIOD_HOURS, PERIODS};
use physics::doc29::boxes::{AircraftBoxAtReceiver, box_sel_at_receiver};
use physics::doc29::screening::Unscreened;
use physics::doc29::segment::{SegmentEmission, SegmentGeometry, segment_sel_at_receiver};
use popup::aircraft::boxes::AircraftReceiver;
use popup::aircraft::flights::{
    BOXES_SEARCHED as BOXES_SEARCHED_BY_POPUP, FLIGHTS_SHOWN, FlightTotals,
};
use popup::aircraft::ring_aircraft;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tiles::aircraft::Aircraft;
use tiles::geo::{LocalFrame, Mercator, TileId};
use tiles::terrain::Terrain;

/// Aircraft within this horizontal distance of a point count (the popup's reach).
const REACH_M: f64 = 16_000.0;
/// The exact sum also reports what lies beyond the reach, out to this distance.
const BEYOND_REACH_M: f64 = 32_000.0;
/// The receiver height above the ground (m).
const RECEIVER_HEIGHT_M: f64 = 4.0;
/// Metres per degree of latitude (only the reading filter uses it, with a margin).
const METRES_PER_DEGREE: f64 = 111_000.0;
/// Rings of z12 tiles around a point that hold its reach (3 tiles are over 16 km anywhere the
/// slice lies), and one more for the terrain within one edge of their boxes.
const REACH_RINGS: u32 = 3;
/// The top-flights list is compared reading this many kept pieces per box (at most what the
/// boxes keep), of the popup's number of loudest boxes per ring or more.
const PIECES: [usize; 5] = [1, 2, 4, 8, 16];
const BOXES_SEARCHED: [usize; 3] = [
    BOXES_SEARCHED_BY_POPUP,
    4 * BOXES_SEARCHED_BY_POPUP,
    usize::MAX,
];

/// One point of the check: latitude, longitude (deg) and the ground under it (m).
#[derive(Debug, Clone, Copy)]
pub struct CheckPoint {
    pub lat: f64,
    pub lon: f64,
    pub ground_m: f64,
}

/// Leq (dB) per period.
pub type Levels = [f64; PERIODS];

/// What the check finds at one point.
#[derive(Debug, Clone)]
pub struct PointReport {
    /// Every segment within the reach, the boxes within it, and the segments beyond it.
    pub exact: Levels,
    pub boxed: Levels,
    pub beyond: Levels,
    /// The first band (exact pieces, boxes) and the bands above it.
    pub near_ground: [Levels; 2],
    pub aloft: [Levels; 2],
    /// The exact ten loudest flights (id, SEL dB) and the popup's lists.
    pub exact_top: Vec<(u64, f64)>,
    pub lists: Vec<FlightList>,
}

/// The popup's top-flights list reading `pieces` per box of `boxes_searched` per ring: the share
/// of the exact ten it holds, and per listed flight (id, SEL dB) its SEL minus the exact.
#[derive(Debug, Clone)]
pub struct FlightList {
    pub pieces: usize,
    pub boxes_searched: usize,
    pub recall: f64,
    pub listed: Vec<(u64, f64, f64)>,
}

/// Leq (dB) per period of the day SEL energies.
fn leq_db(energy: &[f64]) -> Levels {
    std::array::from_fn(|period| 10.0 * (energy[period] / (PERIOD_HOURS[period] * 3_600.0)).log10())
}

/// Energy slots of one point: exact within the reach, beyond it, and the pieces of the first
/// band and above (each per period); per flight its exact SEL energy within the reach.
#[derive(Clone)]
struct Sums {
    energy: [f64; 4 * PERIODS],
    flights: HashMap<u64, f64>,
}

const EXACT: usize = 0;
const BEYOND: usize = PERIODS;
const NEAR_GROUND: usize = 2 * PERIODS;
const ALOFT: usize = 3 * PERIODS;

impl Sums {
    fn new() -> Self {
        Sums {
            energy: [0.0; 4 * PERIODS],
            flights: HashMap::new(),
        }
    }

    fn merge(&mut self, other: Sums) {
        for (a, b) in self.energy.iter_mut().zip(other.energy) {
            *a += b;
        }
        for (flight, energy) in other.flights {
            *self.flights.entry(flight).or_default() += energy;
        }
    }
}

/// A receiver of the check.
struct Receiver {
    frame: LocalFrame,
    altitude_m: f64,
}

impl Receiver {
    /// A point (lat, lon, altitude above sea level) in metres east, north and above the receiver.
    fn local(&self, end: [f64; 3]) -> [f64; 3] {
        self.metres(Mercator::from_degrees(end[0], end[1]), end[2])
    }

    fn metres(&self, position: Mercator, altitude_m: f64) -> [f64; 3] {
        let [east, north] = self.frame.to_metres(position);
        [east, north, altitude_m - self.altitude_m]
    }
}

/// Adds one segment of weight `weight` to the sums of every point it reaches.
fn add_segment(
    sums: &mut [Sums],
    receivers: &[Receiver],
    placement: &Placement,
    segment: &FlightSegment,
    weight: f64,
) {
    let mut emission: Option<Option<(bool, SegmentEmission)>> = None;
    let mut pieces = None;
    for (sums, receiver) in sums.iter_mut().zip(receivers) {
        let (start, end) = (receiver.local(segment.start), receiver.local(segment.end));
        let nearest = start[0].hypot(start[1]).min(end[0].hypot(end[1]));
        if nearest > BEYOND_REACH_M {
            continue;
        }
        let Some((helicopter, emission)) = emission.get_or_insert_with(|| {
            emission_of(segment)
                .map(|(aircraft, emission)| (aircraft.helicopter.is_some(), emission))
        }) else {
            return;
        };
        let period = usize::from(segment.period).min(PERIODS - 1);
        let ground = |metres: f64| metres - receiver.altitude_m;
        let geometry = SegmentGeometry {
            start_m: start,
            end_m: end,
            ground_under_start_m: ground(segment.ground_m[0]),
            ground_under_end_m: ground(segment.ground_m[1]),
        };
        let Some(sel) = segment_sel_at_receiver(emission, &geometry, &Unscreened) else {
            continue;
        };
        let energy = 10f64.powf(sel.sel_db / 10.0);
        if nearest > REACH_M {
            sums.energy[BEYOND + period] += weight * energy;
            continue;
        }
        sums.energy[EXACT + period] += weight * energy;
        *sums.flights.entry(segment.flight_id).or_default() += energy;
        let point = |end: [f64; 3]| (Mercator::from_degrees(end[0], end[1]), end[2]);
        let (from, to) = (point(segment.start), point(segment.end));
        let pieces =
            pieces.get_or_insert_with(|| cut_into_pieces(placement, from, to, *helicopter));
        let length = (to.0.x - from.0.x).hypot(to.0.y - from.0.y);
        for piece in pieces.iter() {
            // The terrain under the piece's ends, along the segment's.
            let under = |at: Mercator| {
                let t = if length > 0.0 {
                    ((at.x - from.0.x).hypot(at.y - from.0.y) / length).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                ground(segment.ground_m[0] + t * (segment.ground_m[1] - segment.ground_m[0]))
            };
            let geometry = SegmentGeometry {
                start_m: receiver.metres(piece.start.0, piece.start.1),
                end_m: receiver.metres(piece.end.0, piece.end.1),
                ground_under_start_m: under(piece.start.0),
                ground_under_end_m: under(piece.end.0),
            };
            if let Some(sel) = segment_sel_at_receiver(emission, &geometry, &Unscreened) {
                let slot = if piece.key.band == 0 {
                    NEAR_GROUND
                } else {
                    ALOFT
                };
                sums.energy[slot + period] += weight * 10f64.powf(sel.sel_db / 10.0);
            }
        }
    }
}

/// The report of every point: one pass over the days of `window` under `segments_dir`, the
/// written boxes under `aircraft_root` (cut with a level step D of `level_step_db`), the box
/// placement from the terrain under `terrain_root`.
pub fn compare(
    segments_dir: &Path,
    window: &Window,
    (aircraft_root, level_step_db): (&Path, f64),
    terrain_root: &Path,
    points: &[CheckPoint],
) -> Result<Vec<PointReport>, String> {
    let receivers: Vec<Receiver> = points
        .iter()
        .map(|point| Receiver {
            frame: LocalFrame::at(Mercator::from_degrees(point.lat, point.lon)),
            altitude_m: point.ground_m + RECEIVER_HEIGHT_M,
        })
        .collect();
    let mut near: HashSet<TileId> = HashSet::new();
    for receiver in &receivers {
        for ring in 0..=REACH_RINGS + 1 {
            near.extend(TileId::containing(receiver.frame.origin).ring(ring));
        }
    }
    let bytes: Vec<(TileId, Vec<u8>)> = near
        .iter()
        .filter_map(|&tile| {
            let path = tiles::tile_path(terrain_root, tile, tiles::Kind::Terrain);
            std::fs::read(path).ok().map(|bytes| (tile, bytes))
        })
        .collect();
    let terrain: HashMap<TileId, Terrain<'_>> = bytes
        .iter()
        .map(|(tile, bytes)| Terrain::parse(bytes).map(|terrain| (*tile, terrain)))
        .collect::<Result<_, _>>()
        .map_err(|error| error.to_string())?;
    let placement = Placement::new(&near, &terrain, level_step_db);
    // Segments are read when an end lies in some point's latitude-longitude box of the
    // beyond-reach distance (with a quarter to spare).
    let reach_deg = 1.25 * BEYOND_REACH_M / METRES_PER_DEGREE;
    let areas: Vec<[f64; 4]> = points
        .iter()
        .map(|point| {
            let across = reach_deg / point.lat.to_radians().cos().max(0.05);
            [
                point.lat - reach_deg,
                point.lat + reach_deg,
                point.lon - across,
                point.lon + across,
            ]
        })
        .collect();
    let keep = |start: [f64; 2], end: [f64; 2]| {
        areas.iter().any(|area| {
            [start, end].iter().any(|at| {
                (area[0]..=area[1]).contains(&at[0]) && (area[2]..=area[3]).contains(&at[1])
            })
        })
    };
    let (baseline, increment) = (
        window.baseline_days.len().max(1) as f64,
        window.increment_days.len().max(1) as f64,
    );
    let mut days: Vec<&String> = window
        .baseline_days
        .iter()
        .chain(&window.increment_days)
        .collect();
    days.sort();
    days.dedup();
    let zero = || vec![Sums::new(); points.len()];
    let mut totals = zero();
    for day in days {
        let path = segments_dir.join("segments").join(format!("{day}.arrow"));
        let segments = read_segments(&path, &keep)?;
        let (weight, secondary) = (
            if window.baseline_days.contains(day) {
                1.0 / baseline
            } else {
                0.0
            },
            if window.increment_days.contains(day) {
                1.0 / increment
            } else {
                0.0
            },
        );
        let day_sums = segments
            .par_iter()
            .fold(zero, |mut sums, segment| {
                let segment_weight = if segment.flags & FLAG_SECONDARY_ONLY != 0 {
                    secondary
                } else {
                    weight
                };
                if segment_weight > 0.0 {
                    add_segment(&mut sums, &receivers, &placement, segment, segment_weight);
                }
                sums
            })
            .reduce(zero, |mut a, b| {
                for (a, b) in a.iter_mut().zip(b) {
                    a.merge(b);
                }
                a
            });
        for (total, day) in totals.iter_mut().zip(day_sums) {
            total.merge(day);
        }
        eprintln!("aircraft check: {day}: {} segments", segments.len());
    }
    receivers
        .iter()
        .zip(totals)
        .map(|(receiver, total)| report(aircraft_root, receiver, total))
        .collect()
}

/// One point's report from its sums and the written boxes around it.
fn report(aircraft_root: &Path, receiver: &Receiver, total: Sums) -> Result<PointReport, String> {
    let centre = TileId::containing(receiver.frame.origin);
    let rings: Vec<Vec<(TileId, Vec<u8>)>> = (0..=REACH_RINGS)
        .map(|ring| {
            centre
                .ring(ring)
                .into_iter()
                .filter_map(|tile| {
                    std::fs::read(tiles::tile_path(aircraft_root, tile, tiles::Kind::Aircraft))
                        .ok()
                        .map(|bytes| (tile, bytes))
                })
                .collect()
        })
        .collect();
    let parsed: Vec<Vec<(TileId, Aircraft<'_>)>> = rings
        .iter()
        .map(|ring| {
            ring.iter()
                .map(|(tile, bytes)| Aircraft::parse(bytes).map(|aircraft| (*tile, aircraft)))
                .collect::<Result<_, _>>()
        })
        .collect::<Result<_, _>>()
        .map_err(|error| error.to_string())?;
    // The boxes' energy in the first band and above.
    let mut near_ground = [0.0; PERIODS];
    let mut aloft = [0.0; PERIODS];
    for (tile, aircraft) in parsed.iter().flatten() {
        for index in 0..aircraft.box_count() {
            let record = aircraft.aircraft_box(index);
            let global = tile.global(record.centroid);
            let [east, north] = receiver
                .frame
                .metres_of_steps([global.x as f64, global.y as f64]);
            if east.hypot(north) > REACH_M {
                continue;
            }
            let at_receiver = AircraftBoxAtReceiver {
                centroid_m: [
                    east,
                    north,
                    record.centroid_altitude_m - receiver.altitude_m,
                ],
                axis_rad: record.axis_rad,
                gradient: record.gradient,
                piece_length_m: record.piece_length_m,
                levels_db: &record.energy_db,
                scaled_distance_m: &record.scaled_distance_m,
                installation_shares: record.installation_shares,
                ground_m: record.ground_m - receiver.altitude_m,
            };
            if let Some(sel) = box_sel_at_receiver(&at_receiver, &Unscreened) {
                let slots = if record.clearance_m == 0.0 {
                    &mut near_ground
                } else {
                    &mut aloft
                };
                for (total, level) in slots.iter_mut().zip(sel.sel_db) {
                    *total += 10f64.powf(level / 10.0);
                }
            }
        }
    }
    let boxed: [f64; PERIODS] = std::array::from_fn(|p| near_ground[p] + aloft[p]);
    // The ten loudest flights of the exact sum, and the popup's list reading K pieces per box.
    let mut exact_top: Vec<(u64, f64)> = total
        .flights
        .iter()
        .map(|(&flight, &energy)| (flight, 10.0 * energy.log10()))
        .collect();
    exact_top.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    exact_top.truncate(FLIGHTS_SHOWN);
    // The popup names a flight by its address and start (the id's bits 39-32 are dropped).
    let named = |flight: u64| (flight >> 40 & 0x00ff_ffff, flight & 0xffff_ffff);
    let exact_sel: HashMap<(u64, u64), f64> = total
        .flights
        .iter()
        .map(|(&flight, &energy)| (named(flight), 10.0 * energy.log10()))
        .collect();
    let aircraft_receiver = AircraftReceiver {
        position: [0.0, 0.0],
        altitude_m: receiver.altitude_m,
    };
    let mut lists = Vec::new();
    for boxes_searched in BOXES_SEARCHED {
        for pieces in PIECES {
            let mut flights = FlightTotals::reading(pieces, boxes_searched);
            for ring in &parsed {
                ring_aircraft(
                    ring,
                    &receiver.frame,
                    aircraft_receiver,
                    &Unscreened,
                    &mut flights,
                );
            }
            let listed: Vec<(u64, f64, f64)> = flights
                .loudest()
                .iter()
                .map(|flight| {
                    let id = (u64::from(flight.icao), u64::from(flight.start_unix));
                    let exact = exact_sel.get(&id).copied().unwrap_or(f64::NAN);
                    (id.0 << 40 | id.1, flight.sel_db, flight.sel_db - exact)
                })
                .collect();
            let found = exact_top
                .iter()
                .filter(|(flight, _)| {
                    let (icao, start) = named(*flight);
                    listed.iter().any(|(id, _, _)| *id == icao << 40 | start)
                })
                .count();
            lists.push(FlightList {
                pieces,
                boxes_searched,
                recall: found as f64 / exact_top.len().max(1) as f64,
                listed,
            });
        }
    }
    Ok(PointReport {
        exact: leq_db(&total.energy[EXACT..]),
        boxed: leq_db(&boxed),
        beyond: leq_db(&total.energy[BEYOND..]),
        near_ground: [leq_db(&total.energy[NEAR_GROUND..]), leq_db(&near_ground)],
        aloft: [leq_db(&total.energy[ALOFT..]), leq_db(&aloft)],
        exact_top,
        lists,
    })
}

/// The ground under a point from the prepared terrain (0 at sea or where none was read).
pub fn ground_at(terrain_root: &Path, point: (f64, f64)) -> f64 {
    let position = Mercator::from_degrees(point.0, point.1);
    let path = tiles::tile_path(
        terrain_root,
        TileId::containing(position),
        tiles::Kind::Terrain,
    );
    std::fs::read(path)
        .ok()
        .and_then(|bytes| {
            Terrain::parse(&bytes)
                .ok()
                .and_then(|terrain| terrain.sample(position))
                .map(|sample| sample.height_m)
        })
        .unwrap_or(0.0)
}

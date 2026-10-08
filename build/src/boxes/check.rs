//! The boxes against the full computation (PLAN section 7): at each benchmark point, every
//! segment of the window through the kernel, weighted as the boxes weigh it, against the written
//! boxes through the click-time equation, both unscreened (the screening is one rule applied
//! alike); per period Leq of each and their difference, also apart for the first band (near the
//! ground, where the data's altitude error dominates) and the bands above, each segment cut into
//! its box pieces as the builder cuts it. The top-flights list as the popup makes it (the kept
//! pieces of each ring's loudest boxes, reading K of them) against the ten loudest flights of the
//! exact sum. One pass over the days serves every point.

mod exact;
mod report;

use super::place::Placement;
use super::read::{FLAG_SECONDARY_ONLY, read_segments};
use super::{Window, place_atmosphere};
use crate::dev4::Square;
use exact::{DISTANCE_BANDS_M, Sums, add_segment};
use physics::bands::{PERIOD_HOURS, PERIODS};
use physics::doc29::atmosphere::PlaceAtmosphere;
use physics::weather::WeatherTable;
use rayon::prelude::*;
use report::report;
use std::collections::{HashMap, HashSet};
use std::path::Path;
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
/// boxes keep).
const PIECES: [usize; 5] = [1, 2, 4, 8, 16];
/// A listed flight whose exact Lmax is within this of the exact tenth's is as loud as the list
/// (under a busy approach dozens of flights are within a few tenths of a decibel).
const LIST_TOLERANCE_DB: f64 = 0.5;

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
    /// The boxes as the popup reads them (far boxes from the second ring) and the megabytes of
    /// aircraft files read: fine boxes in every ring, and as the popup reads them.
    pub boxed_as_read: Levels,
    pub megabytes: [f64; 2],
    /// The first band (exact pieces, boxes) and the bands above it; by horizontal distance
    /// (exact pieces by their middle, fine boxes by their centroid).
    pub near_ground: [Levels; 2],
    pub by_distance: [[Levels; 2]; DISTANCE_BANDS_M.len()],
    pub aloft: [Levels; 2],
    /// The exact ten loudest flights (id, LAmax dB) and the popup's lists.
    pub exact_top: Vec<(u64, f64)>,
    pub lists: Vec<FlightList>,
    /// For a diagnosed point, the fine boxes whose SEL sums miss their pieces' the most.
    pub diagnosis: Vec<BoxDiagnosis>,
}

/// One fine box at a diagnosed point: where it is (centroid latitude, longitude, altitude, the
/// horizontal distance), its cell and slab, its average piece, and per period the day SEL sum
/// (dB) of its pieces through the kernel and of the box through the click-time equation.
#[derive(Debug, Clone)]
pub struct BoxDiagnosis {
    pub centroid: [f64; 3],
    pub distance_m: f64,
    pub zoom: u8,
    pub clearance_m: f64,
    pub flights: u32,
    pub axis_deg: f64,
    pub gradient: f64,
    pub gradient_spread: f64,
    pub piece_length_m: Levels,
    pub exact_db: Levels,
    pub boxed_db: Levels,
    /// Per period, the altitude of the box's pieces weighted by their energy at the point, their
    /// energy-weighted harmonic mean d_lambda at the point, and the box's d_lambda there.
    pub exact_altitude_m: Levels,
    pub exact_scaled_distance_m: Levels,
    pub boxed_scaled_distance_m: f64,
    /// Per period, how far the pieces' energy-weighted middle lies from the box centroid (m), and
    /// the centroid in metres east and north of the point.
    pub exact_offset_m: Levels,
    pub centroid_m: [f64; 2],
    /// Per period, the pieces' horizontal length weighted by their energy at the point (m).
    pub exact_length_m: Levels,
}

/// The popup's top-flights list reading `pieces` per box: the share of the exact ten it holds,
/// the share of it that is as loud as the exact ten within [`LIST_TOLERANCE_DB`], per listed
/// flight (id, LAmax dB) its LAmax minus the exact, and whether the popup's search (boxes by
/// their LAmax bound) lists what computing every kept piece within reach lists.
#[derive(Debug, Clone)]
pub struct FlightList {
    pub pieces: usize,
    pub recall: f64,
    pub tolerant_recall: f64,
    pub listed: Vec<(u64, f64, f64)>,
    pub search_is_exhaustive: bool,
}

/// Leq (dB) per period of the day SEL energies.
fn leq_db(energy: &[f64]) -> Levels {
    std::array::from_fn(|period| 10.0 * (energy[period] / (PERIOD_HOURS[period] * 3_600.0)).log10())
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

/// The report of every point: one pass over the days of `window` under `segments_dir`, the
/// written boxes under `aircraft_root` (cut with a level step D of `level_step_db`), the box
/// placement from the terrain under `terrain_root`.
pub fn compare(
    (segments_dir, weather): (&Path, &WeatherTable),
    window: &Window,
    (aircraft_root, level_step_db): (&Path, f64),
    terrain_root: &Path,
    (points, diagnosed): (&[CheckPoint], Option<usize>),
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
    let zero = || {
        (0..points.len())
            .map(|index| Sums::new(diagnosed == Some(index)))
            .collect::<Vec<_>>()
    };
    let mut totals = zero();
    for day in days {
        let path = segments_dir.join("segments").join(format!("{day}.arrow"));
        let segments = read_segments(&path, &keep)?;
        // Each segment in the atmosphere of its start's square, as the boxes sum it.
        let square_of = |end: [f64; 3]| {
            let tile = TileId::containing(Mercator::from_degrees(end[0], end[1]));
            Square {
                x: tile.x >> 3,
                y: tile.y >> 3,
            }
        };
        let places: HashMap<Square, PlaceAtmosphere> = segments
            .iter()
            .map(|segment| square_of(segment.start))
            .collect::<HashSet<Square>>()
            .into_par_iter()
            .map(|square| (square, place_atmosphere(weather, square)))
            .collect();
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
                    let place = &places[&square_of(segment.start)];
                    add_segment(
                        &mut sums,
                        &receivers,
                        &placement,
                        (segment, place),
                        segment_weight,
                    );
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
        .map(|(receiver, total)| report((aircraft_root, level_step_db), receiver, total))
        .collect()
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

//! The boxes against the full computation (PLAN section 7): at each benchmark point, every
//! segment of the window through the kernel, weighted as the boxes weigh it, against the written
//! boxes through the click-time equation, both unscreened (the screening is one rule applied
//! alike); per period Leq of each and their difference. One pass over the days serves every point.

use super::read::{FLAG_SECONDARY_ONLY, read_segments};
use super::{Window, emission_of};
use physics::bands::{PERIOD_HOURS, PERIODS};
use physics::doc29::boxes::{AircraftBoxAtReceiver, box_sel_at_receiver};
use physics::doc29::screening::Unscreened;
use physics::doc29::segment::{SegmentGeometry, segment_sel_at_receiver};
use rayon::prelude::*;
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

/// One point of the check: latitude, longitude (deg) and the ground under it (m).
#[derive(Debug, Clone, Copy)]
pub struct CheckPoint {
    pub lat: f64,
    pub lon: f64,
    pub ground_m: f64,
}

/// Leq (dB) per period of the day SEL energies.
fn leq_db(energy: [f64; PERIODS]) -> [f64; PERIODS] {
    std::array::from_fn(|period| 10.0 * (energy[period] / (PERIOD_HOURS[period] * 3_600.0)).log10())
}

/// Per point the Leq per period of every segment within the reach (exact), of the boxes within
/// the reach, and of the segments beyond the reach, in that order.
pub fn compare(
    segments_dir: &Path,
    window: &Window,
    aircraft_root: &Path,
    points: &[CheckPoint],
) -> Result<Vec<[[f64; PERIODS]; 3]>, String> {
    let receivers: Vec<(LocalFrame, f64)> = points
        .iter()
        .map(|point| {
            let frame = LocalFrame::at(Mercator::from_degrees(point.lat, point.lon));
            (frame, point.ground_m + RECEIVER_HEIGHT_M)
        })
        .collect();
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
    let zero = || vec![[0.0; 2 * PERIODS]; points.len()];
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
        let day_energy = segments
            .par_iter()
            .fold(zero, |mut energy, segment| {
                let segment_weight = if segment.flags & FLAG_SECONDARY_ONLY != 0 {
                    secondary
                } else {
                    weight
                };
                if segment_weight <= 0.0 {
                    return energy;
                }
                let mut emission = None;
                for (slots, (frame, receiver_altitude)) in energy.iter_mut().zip(&receivers) {
                    let local = |end: [f64; 3]| {
                        let [east, north] = frame.to_metres(Mercator::from_degrees(end[0], end[1]));
                        [east, north, end[2] - receiver_altitude]
                    };
                    let (start, end) = (local(segment.start), local(segment.end));
                    let nearest = start[0].hypot(start[1]).min(end[0].hypot(end[1]));
                    if nearest > BEYOND_REACH_M {
                        continue;
                    }
                    let Some((_, emission)) = emission.get_or_insert_with(|| emission_of(segment))
                    else {
                        break;
                    };
                    let geometry = SegmentGeometry {
                        start_m: start,
                        end_m: end,
                        ground_under_start_m: segment.ground_m[0] - receiver_altitude,
                        ground_under_end_m: segment.ground_m[1] - receiver_altitude,
                    };
                    if let Some(sel) = segment_sel_at_receiver(emission, &geometry, &Unscreened) {
                        let slot = if nearest > REACH_M { PERIODS } else { 0 };
                        slots[slot + usize::from(segment.period).min(PERIODS - 1)] +=
                            segment_weight * 10f64.powf(sel.sel_db / 10.0);
                    }
                }
                energy
            })
            .reduce(zero, |mut a, b| {
                for (a, b) in a.iter_mut().zip(b) {
                    for (a, b) in a.iter_mut().zip(b) {
                        *a += b;
                    }
                }
                a
            });
        for (total, day) in totals.iter_mut().zip(day_energy) {
            for (total, day) in total.iter_mut().zip(day) {
                *total += day;
            }
        }
        eprintln!("aircraft check: {day}: {} segments", segments.len());
    }
    receivers
        .iter()
        .zip(&totals)
        .map(|((frame, receiver_altitude), total)| {
            let boxed = boxed_energy(aircraft_root, frame, *receiver_altitude)?;
            let (exact, beyond) = (
                std::array::from_fn(|period| total[period]),
                std::array::from_fn(|period| total[PERIODS + period]),
            );
            Ok([leq_db(exact), leq_db(boxed), leq_db(beyond)])
        })
        .collect()
}

/// The day SEL energy per period of the written boxes within the reach of a receiver.
fn boxed_energy(
    aircraft_root: &Path,
    frame: &LocalFrame,
    receiver_altitude: f64,
) -> Result<[f64; PERIODS], String> {
    let centre = TileId::containing(frame.origin);
    let mut boxed = [0.0; PERIODS];
    for ring in 0..=3 {
        for tile in centre.ring(ring) {
            let Ok(bytes) =
                std::fs::read(tiles::tile_path(aircraft_root, tile, tiles::Kind::Aircraft))
            else {
                continue;
            };
            let aircraft = Aircraft::parse(&bytes).map_err(|error| error.to_string())?;
            for index in 0..aircraft.box_count() {
                let record = aircraft.aircraft_box(index);
                let global = tile.global(record.centroid);
                let [east, north] = frame.metres_of_steps([global.x as f64, global.y as f64]);
                if east.hypot(north) > REACH_M {
                    continue;
                }
                let at_receiver = AircraftBoxAtReceiver {
                    centroid_m: [east, north, record.centroid_altitude_m - receiver_altitude],
                    axis_rad: record.axis_rad,
                    gradient: record.gradient,
                    piece_length_m: record.piece_length_m,
                    levels_db: &record.energy_db,
                    scaled_distance_m: &record.scaled_distance_m,
                    installation_shares: record.installation_shares,
                    ground_m: record.ground_m - receiver_altitude,
                };
                if let Some(sel) = box_sel_at_receiver(&at_receiver, &Unscreened) {
                    for (total, level) in boxed.iter_mut().zip(sel.sel_db) {
                        *total += 10f64.powf(level / 10.0);
                    }
                }
            }
        }
    }
    Ok(boxed)
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

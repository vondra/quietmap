//! The boxes against the full computation (PLAN section 7): at each benchmark point, every
//! segment of the window through the kernel, weighted as the boxes weigh it, against the written
//! boxes through the click-time equation, both unscreened (the screening is one rule applied
//! alike); per period Leq of each and their difference.

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

/// Leq (dB) per period of the day SEL energies.
fn leq_db(energy: [f64; PERIODS]) -> [f64; PERIODS] {
    std::array::from_fn(|period| 10.0 * (energy[period] / (PERIOD_HOURS[period] * 3_600.0)).log10())
}

/// The Leq per period at a point `(lat, lon)` whose ground is `ground_m`: of every segment within
/// the reach (exact), of the boxes within the reach, and of the segments beyond the reach, in
/// that order.
pub fn compare_at(
    segments_dir: &Path,
    window: &Window,
    aircraft_root: &Path,
    point: (f64, f64),
    ground_m: f64,
) -> Result<[[f64; PERIODS]; 3], String> {
    let origin = Mercator::from_degrees(point.0, point.1);
    let frame = LocalFrame::at(origin);
    let receiver_altitude = ground_m + RECEIVER_HEIGHT_M;
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
    let mut exact = [0.0; PERIODS];
    let mut beyond = [0.0; PERIODS];
    for day in days {
        let segments = read_segments(&segments_dir.join("segments").join(format!("{day}.arrow")))?;
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
        let local = |end: [f64; 3]| {
            let [east, north] = frame.to_metres(Mercator::from_degrees(end[0], end[1]));
            [east, north, end[2] - receiver_altitude]
        };
        let day_energy: [f64; 2 * PERIODS] = segments
            .par_iter()
            .filter_map(|segment| {
                let (start, end) = (local(segment.start), local(segment.end));
                let nearest = start[0].hypot(start[1]).min(end[0].hypot(end[1]));
                if nearest > BEYOND_REACH_M {
                    return None;
                }
                let slot = if nearest > REACH_M { PERIODS } else { 0 };
                let segment_weight = if segment.flags & FLAG_SECONDARY_ONLY != 0 {
                    secondary
                } else {
                    weight
                };
                let (_, emission) = emission_of(segment)?;
                let geometry = SegmentGeometry {
                    start_m: start,
                    end_m: end,
                    ground_under_start_m: segment.ground_m[0] - receiver_altitude,
                    ground_under_end_m: segment.ground_m[1] - receiver_altitude,
                };
                let sel = segment_sel_at_receiver(&emission, &geometry, &Unscreened)?;
                let mut energy = [0.0; 2 * PERIODS];
                energy[slot + usize::from(segment.period).min(PERIODS - 1)] =
                    segment_weight * 10f64.powf(sel.sel_db / 10.0);
                Some(energy)
            })
            .reduce(
                || [0.0; 2 * PERIODS],
                |a, b| std::array::from_fn(|p| a[p] + b[p]),
            );
        for period in 0..PERIODS {
            exact[period] += day_energy[period];
            beyond[period] += day_energy[PERIODS + period];
        }
    }
    let centre = TileId::containing(origin);
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
    Ok([leq_db(exact), leq_db(boxed), leq_db(beyond)])
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

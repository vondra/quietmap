//! Anonymous (`~`, TIS-B) echoes of an address track (dev4 `provider_merge.rs`): a `~` track
//! riding one address track for a minute is that aircraft seen twice, and its samples go.

use super::filters::point_is_sane;
use super::flat::{flat_distance_m, interpolate};
use super::merge::SAME_OBSERVATION_S;
use super::trace::{AircraftTrace, SECONDARY_PROVIDER, TracePoint, address_identity};
use std::collections::{HashMap, HashSet};

/// An anonymous track coincides with an address track on at least 6 ten-second bins (60 s) of
/// samples within 450 m and 400 ft (2026-05-01 pairing: 172 adsb.lol `~` tracks duplicated an
/// ADSBexchange address track).
const ANONYMOUS_MATCH_DISTANCE_M: f32 = 450.0;
const ANONYMOUS_MATCH_ALTITUDE_FT: f32 = 400.0;
const ANONYMOUS_MATCH_BRACKET_S: f64 = 30.0;
const ANONYMOUS_MATCH_BIN_S: f64 = 10.0;
const ANONYMOUS_MATCH_MIN_BINS: usize = 6;
/// Candidate cells (60 s x 0.05 degree) bound the pairwise comparison.
const CANDIDATE_CELL_S: f64 = 60.0;
const CANDIDATE_CELL_DEG: f32 = 0.05;

type Cell = (i64, i32, i32);

fn candidate_cell(point: &TracePoint) -> Cell {
    (
        (point.timestamp / CANDIDATE_CELL_S).floor() as i64,
        (point.lat / CANDIDATE_CELL_DEG).floor() as i32,
        (point.lon / CANDIDATE_CELL_DEG).floor() as i32,
    )
}

fn neighbouring_cells(cell: Cell) -> impl Iterator<Item = Cell> {
    (-1..=1).flat_map(move |dt| {
        (-1..=1).flat_map(move |dy| (-1..=1).map(move |dx| (cell.0 + dt, cell.1 + dy, cell.2 + dx)))
    })
}

/// Drop `~` samples coinciding with one address track for at least 6 ten-second bins. A dropped
/// primary sample hands its baseline provenance to the coincident address samples, so a duplicate
/// observation under another identity adds no increment energy (the +2.94 dB double count).
pub fn suppress_anonymous_echoes(traces: &mut [AircraftTrace]) -> u64 {
    let is_anonymous =
        |trace: &AircraftTrace| address_identity(&trace.address).is_some_and(|(tilde, _)| tilde);
    let anonymous: Vec<usize> = (0..traces.len())
        .filter(|&i| is_anonymous(&traces[i]))
        .collect();
    if anonymous.is_empty() {
        return 0;
    }
    let mut wanted: HashSet<Cell> = HashSet::new();
    for &index in &anonymous {
        for point in traces[index].points.iter().filter(|p| point_is_sane(p)) {
            wanted.extend(neighbouring_cells(candidate_cell(point)));
        }
    }
    let mut address_cells: HashMap<Cell, Vec<usize>> = HashMap::new();
    for (index, trace) in traces.iter().enumerate() {
        if address_identity(&trace.address).is_none_or(|(tilde, _)| tilde) {
            continue;
        }
        for point in trace.points.iter().filter(|p| point_is_sane(p)) {
            let cell = candidate_cell(point);
            if wanted.contains(&cell) {
                let list = address_cells.entry(cell).or_default();
                if list.last() != Some(&index) {
                    list.push(index);
                }
            }
        }
    }
    let (mut drops, mut transfers) = (Vec::with_capacity(anonymous.len()), Vec::new());
    for index in anonymous {
        let mut candidates: Vec<usize> = Vec::new();
        let mut seen = HashSet::new();
        for point in traces[index].points.iter().filter(|p| point_is_sane(p)) {
            if seen.insert(candidate_cell(point)) {
                for cell in neighbouring_cells(candidate_cell(point)) {
                    candidates.extend(address_cells.get(&cell).into_iter().flatten());
                }
            }
        }
        candidates.sort_unstable();
        candidates.dedup();
        let mut drop = vec![false; traces[index].points.len()];
        for candidate in candidates {
            let matched = coinciding_samples(&traces[index].points, &traces[candidate].points);
            let bins: HashSet<i64> = matched
                .iter()
                .map(|&(i, _, _)| {
                    (traces[index].points[i].timestamp / ANONYMOUS_MATCH_BIN_S).floor() as i64
                })
                .collect();
            if bins.len() >= ANONYMOUS_MATCH_MIN_BINS {
                for (i, first, second) in matched {
                    drop[i] = true;
                    if !traces[index].points[i].is_secondary() {
                        transfers.push((candidate, first));
                        transfers.extend(second.map(|second| (candidate, second)));
                    }
                }
            }
        }
        drops.push((index, drop));
    }
    let mut suppressed = 0;
    for (index, drop) in drops {
        let before = traces[index].points.len();
        traces[index].retain_points(|i, _| !drop[i]);
        suppressed += (before - traces[index].points.len()) as u64;
    }
    for (trace, point) in transfers {
        traces[trace].points[point].flags &= !SECONDARY_PROVIDER;
    }
    suppressed
}

/// `anonymous` samples on the `address` track: the address position interpolated between samples
/// at most 30 s apart (or a sample within 1 s) within 450 m, and within 400 ft when both are
/// airborne; each with the one or two address samples it lies between.
fn coinciding_samples(
    anonymous: &[TracePoint],
    address: &[TracePoint],
) -> Vec<(usize, usize, Option<usize>)> {
    let mut track: Vec<(usize, &TracePoint)> = address
        .iter()
        .enumerate()
        .filter(|(_, p)| point_is_sane(p))
        .collect();
    track.sort_by(|a, b| a.1.timestamp.total_cmp(&b.1.timestamp));
    let mut matched = Vec::new();
    for (index, point) in anonymous.iter().enumerate() {
        if !point_is_sane(point) {
            continue;
        }
        let t = point.timestamp;
        let after = track.partition_point(|(_, p)| p.timestamp < t);
        let before = after.checked_sub(1).map(|i| track[i]);
        let reference = match (before, track.get(after).copied()) {
            (Some(a), Some(b)) if b.1.timestamp - a.1.timestamp <= ANONYMOUS_MATCH_BRACKET_S => {
                (between(a.1, b.1, t), (a.0, Some(b.0)))
            }
            (a, b) => {
                let nearest = [a, b]
                    .into_iter()
                    .flatten()
                    .filter(|(_, p)| (p.timestamp - t).abs() <= SAME_OBSERVATION_S)
                    .min_by(|x, y| {
                        (x.1.timestamp - t)
                            .abs()
                            .total_cmp(&(y.1.timestamp - t).abs())
                    });
                match nearest {
                    Some((original, p)) => {
                        ((p.lat, p.lon, p.airborne_altitude_ft()), (original, None))
                    }
                    None => continue,
                }
            }
        };
        let ((lat, lon, altitude), (first, second)) = reference;
        if flat_distance_m(point.lat, point.lon, lat, lon) > ANONYMOUS_MATCH_DISTANCE_M {
            continue;
        }
        let same_level = match (point.airborne_altitude_ft(), altitude) {
            (Some(a), Some(b)) => (a - b).abs() <= ANONYMOUS_MATCH_ALTITUDE_FT,
            (None, None) => true,
            _ => false,
        };
        if same_level {
            matched.push((index, first, second));
        }
    }
    matched
}

fn between(a: &TracePoint, b: &TracePoint, t: f64) -> (f32, f32, Option<f32>) {
    let span = b.timestamp - a.timestamp;
    let w = if span > 0.0 {
        ((t - a.timestamp) / span) as f32
    } else {
        0.0
    };
    let (lat, lon) = interpolate(a.lat, a.lon, b.lat, b.lon, w);
    let altitude = match (a.airborne_altitude_ft(), b.airborne_altitude_ft()) {
        (Some(x), Some(y)) => Some(x + (y - x) * w),
        (x, y) => {
            if w < 0.5 {
                x
            } else {
                y
            }
        }
    };
    (lat, lon, altitude)
}

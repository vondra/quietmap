//! Builds `aircraft` tiles (PLAN section 5) from Stage 1's day files: every airborne or cruise
//! segment gets its Doc 29 emission (`physics::doc29`), is cut into the pieces of the boxes it
//! crosses, and each piece is added to its box, weighted as an average day of the window (the
//! P/B + S/I estimator: primary flights over the baseline days, flights only the secondary
//! provider saw over the increment days). A box keeps its loudest pieces for the top-flights list.

pub mod check;
mod place;
mod read;
mod write;

use physics::bands::PERIODS;
use physics::doc29::box_sums::BoxSums;
use physics::doc29::segment::{AircraftType, SegmentEmission};
use physics::doc29::thrust::SegmentFlight;
use place::{BoxKey, BoxPiece, Placement, cut_into_pieces};
use rayon::prelude::*;
use read::{
    FLAG_DEPARTURE, FLAG_HELICOPTER_DESCENT, FLAG_ON_GROUND, FLAG_SECONDARY_ONLY, FlightSegment,
    read_segments,
};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tiles::geo::{Mercator, TileId};
use tiles::terrain::Terrain;

/// Loudest pieces a box keeps for the top-flights list (PLAN section 5: K starts at 2).
pub const PIECES_PER_BOX: usize = 2;
/// The NPD distance (index) whose energy ranks a box's pieces (1,000 ft).
const RANKING_DISTANCE: usize = 3;

/// The days of the window: baseline days carry the primary provider; increment days add the
/// flights only the secondary provider saw.
pub struct Window {
    pub baseline_days: Vec<String>,
    pub increment_days: Vec<String>,
}

/// One kept piece of a box: what the popup needs to compute it exactly and name its flight.
#[derive(Debug, Clone, PartialEq)]
pub struct KeptPiece {
    pub rank: f64,
    pub segment: FlightSegment,
    pub emission: SegmentEmission,
    pub piece: BoxPiece,
}

/// What a box gathers over the window.
#[derive(Default)]
pub struct BoxEntry {
    pub sums: BoxSums,
    pub ground_m: f64,
    /// Flights per average day that crossed it, and the last flight seen (a flight's segments
    /// arrive together within a day).
    pub flights_per_day: f64,
    last_flight: u64,
    pub kept: Vec<KeptPiece>,
}

impl BoxEntry {
    fn keep(&mut self, candidate: KeptPiece) {
        if self.kept.len() < PIECES_PER_BOX {
            self.kept.push(candidate);
        } else if let Some(weakest) = self
            .kept
            .iter_mut()
            .min_by(|a, b| a.rank.total_cmp(&b.rank))
            .filter(|weakest| weakest.rank < candidate.rank)
        {
            *weakest = candidate;
        }
    }

    fn merge(&mut self, other: BoxEntry) {
        self.sums.merge(&other.sums);
        self.ground_m = other.ground_m;
        self.flights_per_day += other.flights_per_day;
        for kept in other.kept {
            self.keep(kept);
        }
    }
}

/// The Doc 29 emission of a segment, or `None` outside the thrust model's domain.
fn emission_of(segment: &FlightSegment) -> Option<(AircraftType, SegmentEmission)> {
    let designator = String::from_utf8_lossy(&segment.designator)
        .trim()
        .to_string();
    let aircraft = AircraftType::from_designator(&designator);
    let altitude = 0.5 * (segment.start[2] + segment.end[2]);
    let (dx, dy) = geo_metres(segment.start, segment.end);
    let length = dx
        .hypot(dy)
        .hypot(segment.end[2] - segment.start[2])
        .max(1.0);
    let flight = SegmentFlight {
        departure: segment.flags & FLAG_DEPARTURE != 0,
        on_ground: segment.flags & FLAG_ON_GROUND != 0,
        speed_kt: segment.speed_kt,
        pressure_altitude_m: 0.5
            * (segment.pressure_altitude_m[0] + segment.pressure_altitude_m[1]),
        climb_sine: (segment.end[2] - segment.start[2]) / length,
        height_above_field_m: if segment.departure_field_m.is_finite() {
            altitude - segment.departure_field_m
        } else {
            segment.above_ground_m
        },
    };
    let descent = segment.flags & FLAG_HELICOPTER_DESCENT != 0;
    SegmentEmission::new(&aircraft, &flight, descent).map(|emission| (aircraft, emission))
}

/// East and north metres from one (lat, lon) to another, on the local sphere (only a piece's
/// climb sine uses it).
fn geo_metres(from: [f64; 3], to: [f64; 3]) -> (f64, f64) {
    let metres_per_degree = 111_195.0;
    let east = (to[1] - from[1]) * metres_per_degree * from[0].to_radians().cos();
    (east, (to[0] - from[0]) * metres_per_degree)
}

/// Whether a segment's tile bounding box meets `scope` (segments far outside are skipped).
fn touches(scope: &HashSet<TileId>, start: Mercator, end: Mercator) -> bool {
    let (x0, x1) = (
        start.x.min(end.x).floor() as i64,
        start.x.max(end.x).floor() as i64,
    );
    let (y0, y1) = (
        start.y.min(end.y).floor() as i64,
        start.y.max(end.y).floor() as i64,
    );
    if (x1 - x0 + 1) * (y1 - y0 + 1) > 64 {
        return true;
    }
    (y0..=y1).any(|y| {
        (x0..=x1).any(|x| {
            scope.contains(&TileId {
                x: x.rem_euclid(i64::from(tiles::geo::TILES_PER_AXIS)) as u32,
                y: y.clamp(0, i64::from(tiles::geo::TILES_PER_AXIS) - 1) as u32,
            })
        })
    })
}

/// Adds one day's segments to `boxes`, weighted `weight` for primary flights and
/// `secondary_weight` for flights only the secondary provider saw (0 on baseline days).
fn add_day(
    boxes: &mut HashMap<BoxKey, BoxEntry>,
    segments: &[FlightSegment],
    placement: &Placement,
    scope: &HashSet<TileId>,
    weight: f64,
    secondary_weight: f64,
) {
    let partials: Vec<HashMap<BoxKey, BoxEntry>> = segments
        .par_chunks(4_096)
        .map(|chunk| {
            let mut local: HashMap<BoxKey, BoxEntry> = HashMap::new();
            for segment in chunk {
                let segment_weight = if segment.flags & FLAG_SECONDARY_ONLY != 0 {
                    secondary_weight
                } else {
                    weight
                };
                if segment_weight <= 0.0 || usize::from(segment.period) >= PERIODS {
                    continue;
                }
                let point = |end: [f64; 3]| (Mercator::from_degrees(end[0], end[1]), end[2]);
                if !touches(scope, point(segment.start).0, point(segment.end).0) {
                    continue;
                }
                let Some((aircraft, emission)) = emission_of(segment) else {
                    continue;
                };
                let mut period_weights = [0.0; PERIODS];
                period_weights[usize::from(segment.period)] = segment_weight;
                let levels = emission.npd_distance_levels();
                let helicopter = aircraft.helicopter.is_some();
                for piece in cut_into_pieces(
                    placement,
                    point(segment.start),
                    point(segment.end),
                    helicopter,
                ) {
                    if !scope.contains(&piece.key.tile) {
                        continue;
                    }
                    let entry = local.entry(piece.key).or_default();
                    let [start_m, end_m] = write::piece_metres(&piece);
                    entry.sums.add(
                        &levels,
                        emission.installation,
                        period_weights,
                        start_m,
                        end_m,
                    );
                    entry.ground_m = piece.ground_m;
                    if entry.last_flight != segment.flight_id {
                        entry.last_flight = segment.flight_id;
                        entry.flights_per_day += segment_weight;
                    }
                    let length = (end_m[0] - start_m[0]).hypot(end_m[1] - start_m[1]);
                    entry.keep(KeptPiece {
                        rank: segment_weight
                            * 10f64.powf(levels.sel_db[RANKING_DISTANCE] / 10.0)
                            * length,
                        segment: segment.clone(),
                        emission,
                        piece,
                    });
                }
            }
            local
        })
        .collect();
    for partial in partials {
        for (key, entry) in partial {
            boxes.entry(key).or_default().merge(entry);
        }
    }
}

/// Builds the aircraft tiles of `scope` from the day files under `segments_dir` and the terrain
/// of `terrain_root` (a prepared year root); writes them under `out`, returns the number written.
pub fn build(
    segments_dir: &Path,
    window: &Window,
    terrain_root: &Path,
    scope: &HashSet<TileId>,
    out: &Path,
) -> Result<usize, String> {
    let mut near: HashSet<TileId> = HashSet::new();
    for tile in scope {
        for ring in 0..=1 {
            near.extend(tile.ring(ring));
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
    let placement = Placement::new(&near, &terrain);
    eprintln!("aircraft boxes: terrain pyramid of {} tiles", near.len());
    let mut boxes: HashMap<BoxKey, BoxEntry> = HashMap::new();
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
    for day in days {
        let path = segments_dir.join("segments").join(format!("{day}.arrow"));
        let segments = read_segments(&path)?;
        // An increment day is usually a baseline day too: its primary flights count toward the
        // baseline, the flights only the secondary provider saw toward the increment.
        let weight = if window.baseline_days.contains(day) {
            1.0 / baseline
        } else {
            0.0
        };
        let secondary_weight = if window.increment_days.contains(day) {
            1.0 / increment
        } else {
            0.0
        };
        add_day(
            &mut boxes,
            &segments,
            &placement,
            scope,
            weight,
            secondary_weight,
        );
        eprintln!(
            "aircraft boxes: {day}: {} segments, {} boxes",
            segments.len(),
            boxes.len()
        );
    }
    write::write_tiles(&boxes, out)
}

#[cfg(test)]
mod tests;

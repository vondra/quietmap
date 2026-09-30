//! Builds `aircraft` tiles (PLAN section 5) from Stage 1's day files: the window's segments are
//! first sorted into z9 squares ([`shuffle`]), then square by square every airborne or cruise
//! segment gets its Doc 29 emission (`physics::doc29`), is cut into the pieces of the boxes it
//! crosses, and each piece is added to its box, weighted as an average day of the window. A box
//! keeps its loudest pieces for the top-flights list.

pub mod check;
mod place;
mod read;
pub mod shuffle;
mod write;

use crate::dev4::Square;
use physics::bands::PERIODS;
use physics::doc29::box_sums::BoxSums;
use physics::doc29::segment::{AircraftType, NpdDistanceLevels, SegmentEmission};
use physics::doc29::thrust::SegmentFlight;
use place::{BoxKey, BoxPiece, Placement, cut_into_pieces};
use rayon::prelude::*;
use read::{FLAG_DEPARTURE, FLAG_HELICOPTER_DESCENT, FLAG_ON_GROUND, FlightSegment};
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

/// One kept piece of a box: what the popup needs to compute it exactly and name its flight
/// (about 100 B; a box keeps a few for the whole year).
#[derive(Debug, Clone, PartialEq)]
pub struct KeptPiece {
    pub rank: f64,
    pub flight_id: u64,
    pub callsign: [u8; 8],
    pub designator: [u8; 4],
    /// Stage 1 flags and period, and the speed (kt).
    pub flags: u8,
    pub period: u8,
    pub speed_kt: f32,
    /// The kernel's emission class and power bracket code.
    pub class: u16,
    pub power_code: u16,
    /// The piece's ends: position and altitude above sea level.
    pub start: (Mercator, f64),
    pub end: (Mercator, f64),
}

impl KeptPiece {
    fn new(
        rank: f64,
        segment: &FlightSegment,
        emission: &SegmentEmission,
        piece: &BoxPiece,
    ) -> Self {
        KeptPiece {
            rank,
            flight_id: segment.flight_id,
            callsign: segment.callsign,
            designator: segment.designator,
            flags: segment.flags,
            period: segment.period,
            speed_kt: segment.speed_kt as f32,
            class: emission.class as u16,
            power_code: emission.power.code(),
            start: piece.start,
            end: piece.end,
        }
    }
}

/// Boxes are kept in this many shards by tile, so that each thread adds pieces to its own.
const SHARDS: usize = 512;

/// The shard of a tile's boxes (neighbouring tiles fall far apart).
fn shard_of(tile: TileId) -> usize {
    let mixed = (u64::from(tile.x) << 32 | u64::from(tile.y)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    (mixed >> (64 - SHARDS.trailing_zeros())) as usize
}

/// The boxes being built, in shards by tile.
pub struct Boxes {
    shards: Vec<HashMap<BoxKey, BoxEntry>>,
}

impl Default for Boxes {
    fn default() -> Self {
        Boxes {
            shards: (0..SHARDS).map(|_| HashMap::new()).collect(),
        }
    }
}

impl Boxes {
    fn len(&self) -> usize {
        self.shards.iter().map(HashMap::len).sum()
    }

    fn iter(&self) -> impl Iterator<Item = (&BoxKey, &BoxEntry)> {
        self.shards.iter().flatten()
    }
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
    /// Keeps `candidate` if it is among the `limit` loudest pieces so far.
    fn keep(&mut self, candidate: KeptPiece, limit: usize) {
        if self.kept.len() < limit {
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

/// Adds one day's segments with their window weights to `boxes`, each box keeping its `pieces`
/// loudest pieces. Each flight's segments must be together: a box counts a flight once while its
/// pieces arrive one after another.
fn add_day(
    boxes: &mut Boxes,
    segments: &[(FlightSegment, f64)],
    placement: &Placement,
    scope: &HashSet<TileId>,
    pieces: usize,
) {
    let point = |end: [f64; 3]| (Mercator::from_degrees(end[0], end[1]), end[2]);
    // Each segment's emission, its levels and whether it is a helicopter (none without weight,
    // outside the scope or outside the thrust model's domain).
    let emitted: Vec<Option<(SegmentEmission, NpdDistanceLevels, bool)>> = segments
        .par_iter()
        .map(|(segment, weight)| {
            if *weight <= 0.0 || usize::from(segment.period) >= PERIODS {
                return None;
            }
            if !touches(scope, point(segment.start).0, point(segment.end).0) {
                return None;
            }
            let (aircraft, emission) = emission_of(segment)?;
            let helicopter = aircraft.helicopter.is_some();
            Some((emission, emission.npd_distance_levels(), helicopter))
        })
        .collect();
    // Every piece in the scope with its shard, segment and order along the segment; then each
    // shard adds its pieces in that order.
    let mut cut: Vec<(u16, u32, u16, BoxPiece)> = segments
        .par_iter()
        .zip(&emitted)
        .enumerate()
        .flat_map_iter(|(index, ((segment, _), emitted))| {
            let pieces = match emitted {
                Some((_, _, helicopter)) => cut_into_pieces(
                    placement,
                    point(segment.start),
                    point(segment.end),
                    *helicopter,
                ),
                None => Vec::new(),
            };
            pieces
                .into_iter()
                .enumerate()
                .filter(|(_, piece)| scope.contains(&piece.key.tile))
                .map(move |(order, piece)| {
                    let shard = shard_of(piece.key.tile) as u16;
                    (shard, index as u32, order as u16, piece)
                })
        })
        .collect();
    cut.par_sort_unstable_by_key(|&(shard, index, order, _)| (shard, index, order));
    let bounds: Vec<usize> = (0..=SHARDS)
        .map(|shard| cut.partition_point(|entry| usize::from(entry.0) < shard))
        .collect();
    boxes
        .shards
        .par_iter_mut()
        .enumerate()
        .for_each(|(shard, map)| {
            for (_, index, _, piece) in &cut[bounds[shard]..bounds[shard + 1]] {
                let (segment, segment_weight) = &segments[*index as usize];
                let Some((emission, levels, _)) = &emitted[*index as usize] else {
                    continue;
                };
                let segment_weight = *segment_weight;
                let mut period_weights = [0.0; PERIODS];
                period_weights[usize::from(segment.period)] = segment_weight;
                let entry = map.entry(piece.key).or_default();
                let [start_m, end_m] = write::piece_metres(piece);
                entry.sums.add(
                    levels,
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
                let rank =
                    segment_weight * 10f64.powf(levels.sel_db[RANKING_DISTANCE] / 10.0) * length;
                entry.keep(KeptPiece::new(rank, segment, emission, piece), pieces);
            }
        });
}

/// How boxes are cut and what they keep: the level step D of their edges (dB) and the loudest
/// pieces kept per box.
#[derive(Debug, Clone, Copy)]
pub struct BoxRule {
    pub level_step_db: f64,
    pub pieces: usize,
}

impl Default for BoxRule {
    fn default() -> Self {
        BoxRule {
            level_step_db: physics::doc29::box_geometry::BOX_EDGE_LEVEL_STEP_DB,
            pieces: PIECES_PER_BOX,
        }
    }
}

/// Builds the aircraft tiles of one z9 square from its day files under `shuffled` (see
/// [`shuffle`]) and the terrain of `terrain_root` (a prepared year root) by `rule`; writes them
/// under `out`, returns the number of tiles and boxes written.
pub fn build_square(
    shuffled: &Path,
    square: Square,
    terrain_root: &Path,
    rule: BoxRule,
    out: &Path,
) -> Result<(usize, usize), String> {
    let scope: HashSet<TileId> = (0..64)
        .map(|index| TileId {
            x: square.x * 8 + index % 8,
            y: square.y * 8 + index / 8,
        })
        .collect();
    let mut near: HashSet<TileId> = HashSet::new();
    for tile in &scope {
        near.extend(tile.ring(0));
        near.extend(tile.ring(1));
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
    let placement = Placement::new(&near, &terrain, rule.level_step_db);
    let mut boxes = Boxes::default();
    for mut segments in shuffle::square_days(shuffled, square)? {
        // Each flight's segments together, in their order.
        segments.par_sort_by_key(|(segment, _)| segment.flight_id);
        add_day(&mut boxes, &segments, &placement, &scope, rule.pieces);
    }
    let written = write::write_tiles(&boxes, &placement, out)?;
    Ok((written, boxes.len()))
}

#[cfg(test)]
mod tests;

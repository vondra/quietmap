//! Builds `aircraft` tiles (PLAN section 5) from Stage 1's day files: the window's segments are
//! first sorted into z9 squares ([`shuffle`]), then square by square every airborne or cruise
//! segment gets its Doc 29 emission (`physics::doc29`), is cut into the pieces of the boxes it
//! crosses, and each piece is added to its box, weighted as an average day of the window. A box
//! keeps its loudest pieces for the top-flights list.

pub mod check;
pub mod events;
mod place;
mod read;
pub mod shuffle;
mod write;

use crate::dev4::Square;
use physics::bands::PERIODS;
use physics::doc29::atmosphere::PlaceAtmosphere;
use physics::doc29::box_sums::BoxSums;
use physics::doc29::npd::LAMAX_REFERENCE_SLANT_M;
use physics::doc29::segment::{AircraftType, NpdDistanceLevels, SegmentEmission};
use physics::doc29::thrust::SegmentFlight;
use physics::weather::WeatherTable;
use place::{BoxKey, BoxPiece, Placement, cut_into_pieces};
use rayon::prelude::*;
use read::{FLAG_DEPARTURE, FLAG_HELICOPTER_DESCENT, FLAG_ON_GROUND, FlightSegment};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tiles::geo::{Mercator, TileId};
use tiles::terrain::Terrain;

/// Loudest pieces a box keeps for the top-flights list (PLAN section 5: K starts at 2), one per
/// flight, by their LAmax at their height above the box's ground: the list ranks flights by
/// LAmax, and a receiver under a box hears its lowest pieces of the loudest types loudest.
pub const PIECES_PER_BOX: usize = 2;

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
    pub flight_id: u64,
    pub callsign: [u8; 8],
    pub designator: [u8; 4],
    /// Stage 1 flags and period, and the speed (kt).
    pub flags: u8,
    pub period: u8,
    pub speed_kt: f32,
    /// The kernel's emission class and power bracket code, and its LAmax at 1,000 ft (dB).
    pub class: u16,
    pub power_code: u16,
    pub lamax_reference_db: f64,
    /// Its LAmax at its height above the box's ground (dB): what orders a box's pieces.
    pub keep_level_db: f64,
    /// The piece's ends: position and altitude above sea level.
    pub start: (Mercator, f64),
    pub end: (Mercator, f64),
}

impl KeptPiece {
    fn new(segment: &FlightSegment, emission: &SegmentEmission, piece: &BoxPiece) -> Self {
        KeptPiece {
            flight_id: segment.flight_id,
            callsign: segment.callsign,
            designator: segment.designator,
            flags: segment.flags,
            period: segment.period,
            speed_kt: segment.speed_kt as f32,
            class: emission.class as u16,
            power_code: emission.power.code(),
            lamax_reference_db: emission.read_npd(LAMAX_REFERENCE_SLANT_M).lamax_db,
            keep_level_db: emission
                .read_npd(piece.start.1.min(piece.end.1) - piece.ground_m)
                .lamax_db,
            start: piece.start,
            end: piece.end,
        }
    }

    /// The order a box keeps its pieces in: by `keep_level_db`, equal levels by a hash of the
    /// flight, so that the flights kept of one type spread over the window.
    pub fn cmp_loudness(&self, other: &KeptPiece) -> Ordering {
        let mixed = |flight: u64| flight.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        self.keep_level_db
            .total_cmp(&other.keep_level_db)
            .then(mixed(self.flight_id).cmp(&mixed(other.flight_id)))
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
    /// Flights per average day that crossed it by the period of their first piece in it, and the
    /// last flight seen (a flight's segments arrive together within a day).
    pub flights_per_day: [f64; PERIODS],
    last_flight: u64,
    pub kept: Vec<KeptPiece>,
}

impl BoxEntry {
    /// Keeps `candidate` if it is among the `limit` loudest pieces so far, one per flight.
    fn keep(&mut self, candidate: KeptPiece, limit: usize) {
        let same_flight = self
            .kept
            .iter()
            .position(|kept| kept.flight_id == candidate.flight_id);
        let slot = match same_flight {
            Some(slot) => slot,
            None if self.kept.len() < limit => {
                self.kept.push(candidate);
                return;
            }
            None => match (0..self.kept.len())
                .min_by(|&a, &b| self.kept[a].cmp_loudness(&self.kept[b]))
            {
                Some(weakest) => weakest,
                None => return,
            },
        };
        if self.kept[slot].cmp_loudness(&candidate).is_lt() {
            self.kept[slot] = candidate;
        }
    }
}

/// The yearly atmosphere of a z9 square: the weather table's absorption at its centre (the table
/// has 0.5 degree nodes, a square spans 0.7 degrees of longitude).
pub(crate) fn place_atmosphere(weather: &WeatherTable, square: Square) -> PlaceAtmosphere {
    let centre = TileId {
        x: square.x * 8 + 4,
        y: square.y * 8 + 4,
    }
    .centre();
    let (lat, lon) = centre.to_degrees();
    PlaceAtmosphere::new(&weather.alpha_at(lat, lon))
}

/// The Doc 29 emission of a segment in the class of the profile Stage 1 decided for its flight,
/// or `None` outside the thrust model's domain.
fn emission_of(segment: &FlightSegment) -> Option<(AircraftType, SegmentEmission)> {
    let designator = String::from_utf8_lossy(&segment.designator);
    let aircraft = AircraftType::of(segment.profile, designator.trim());
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
        acceleration_ms2: segment.acceleration_ms2,
        height_above_field_m: if segment.flags & FLAG_DEPARTURE != 0
            && segment.departure_field_m.is_finite()
        {
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
    (placement, place): (&Placement, &PlaceAtmosphere),
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
            let emission = emission.in_atmosphere(place);
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
                    entry.flights_per_day[usize::from(segment.period)] += segment_weight;
                }
                entry.keep(KeptPiece::new(segment, emission, piece), pieces);
            }
        });
}

/// How boxes are cut and what they keep: the level step D of their edges (dB), the loudest
/// pieces kept per box, and the kind of file they go to.
#[derive(Debug, Clone, Copy)]
pub struct BoxRule {
    pub level_step_db: f64,
    pub pieces: usize,
    pub kind: tiles::Kind,
}

impl Default for BoxRule {
    fn default() -> Self {
        BoxRule {
            level_step_db: physics::doc29::box_geometry::BOX_EDGE_LEVEL_STEP_DB,
            pieces: PIECES_PER_BOX,
            kind: tiles::Kind::Aircraft,
        }
    }
}

impl BoxRule {
    /// The far boxes, read from the second ring on.
    pub fn far() -> Self {
        BoxRule {
            level_step_db: physics::doc29::box_geometry::FAR_BOX_EDGE_LEVEL_STEP_DB,
            kind: tiles::Kind::AircraftFar,
            ..BoxRule::default()
        }
    }
}

/// Builds the aircraft tiles of one z9 square from its day files under `shuffled` (see
/// [`shuffle`]; `days` are the days done there with their roles) and the terrain of
/// `terrain_root` (a prepared year root) by `rule`; writes them under `out`, returns the number
/// of tiles and boxes written.
pub fn build_square(
    (shuffled, days): (&Path, &[(String, shuffle::DayRoles)]),
    (square, weather): (Square, &WeatherTable),
    terrain_root: &Path,
    rule: BoxRule,
    out: &Path,
) -> Result<(usize, usize), String> {
    let place = place_atmosphere(weather, square);
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
    let counts = (
        days.iter().filter(|(_, roles)| roles.baseline).count(),
        days.iter().filter(|(_, roles)| roles.increment).count(),
    );
    for (day, roles) in days {
        let mut segments: Vec<(FlightSegment, f64)> = shuffle::square_day(shuffled, square, day)?
            .into_iter()
            .map(|segment| {
                let weight = roles.weight(&segment, counts);
                (segment, weight)
            })
            .collect();
        // Each flight's segments together, in their order.
        segments.par_sort_by_key(|(segment, _)| segment.flight_id);
        add_day(
            &mut boxes,
            &segments,
            (&placement, &place),
            &scope,
            rule.pieces,
        );
    }
    let written = write::write_tiles(&boxes, &placement, rule.kind, out)?;
    Ok((written, boxes.len()))
}

#[cfg(test)]
mod tests;

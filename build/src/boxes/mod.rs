//! Builds `aircraft` tiles (PLAN section 5) from Stage 1's day files: the window's segments are
//! first sorted into z9 squares ([`shuffle`]), then square by square every airborne or cruise
//! segment gets its Doc 29 emission (`physics::doc29`), is cut into the pieces of the boxes it
//! crosses, and each piece is added to its box, weighted as an average day of the window. A box
//! keeps its loudest pieces for the top-flights list.

pub mod check;
pub mod events;
mod place;
mod read;
pub mod run;
pub mod shuffle;
mod write;

use crate::dev4::Square;
use physics::bands::PERIODS;
use physics::doc29::atmosphere::PlaceAtmosphere;
use physics::doc29::box_sums::{BoxSums, SegmentEnergies};
use physics::doc29::npd::LAMAX_REFERENCE_SLANT_M;
use physics::doc29::segment::{AircraftType, SegmentEmission};
use physics::doc29::thrust::SegmentFlight;
use physics::weather::WeatherTable;
use place::{BoxKey, BoxPiece, Placement, TileHasher, cut_within_tile};
use rayon::prelude::*;
use read::{FLAG_DEPARTURE, FLAG_HELICOPTER_DESCENT, FLAG_ON_GROUND, FlightSegment};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tiles::geo::{LocalFrame, Mercator, TileId};
use tiles::square::{TILES_PER_SIDE, TILES_PER_SQUARE, read_held};
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
    fn new(
        segment: &FlightSegment,
        emitted: &Emitted,
        keep_level_db: f64,
        piece: &BoxPiece,
    ) -> Self {
        KeptPiece {
            flight_id: segment.flight_id,
            callsign: segment.callsign,
            designator: segment.designator,
            flags: segment.flags,
            period: segment.period,
            speed_kt: segment.speed_kt as f32,
            class: emitted.emission.class as u16,
            power_code: emitted.emission.power.code(),
            lamax_reference_db: emitted.lamax_reference_db,
            keep_level_db,
            start: piece.start,
            end: piece.end,
        }
    }

    /// The order a box keeps its pieces in: by `keep_level_db`, equal levels by a hash of the
    /// flight, so that the flights kept of one type spread over the window.
    pub fn cmp_loudness(&self, other: &KeptPiece) -> Ordering {
        loudness_order(
            (self.keep_level_db, self.flight_id),
            (other.keep_level_db, other.flight_id),
        )
    }
}

/// [`KeptPiece::cmp_loudness`] of two pieces' (keep level, flight).
fn loudness_order(a: (f64, u64), b: (f64, u64)) -> Ordering {
    let mixed = |flight: u64| flight.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    a.0.total_cmp(&b.0).then(mixed(a.1).cmp(&mixed(b.1)))
}

/// One tile's boxes.
type TileBoxes = HashMap<BoxKey, BoxEntry, std::hash::BuildHasherDefault<TileHasher>>;

/// The boxes of one square being built, a map per tile (by its index in the square), so that each
/// thread adds the pieces of its own tiles.
pub struct Boxes {
    tiles: Vec<TileBoxes>,
}

impl Default for Boxes {
    fn default() -> Self {
        Boxes {
            tiles: (0..TILES_PER_SQUARE)
                .map(|_| TileBoxes::default())
                .collect(),
        }
    }
}

impl Boxes {
    fn len(&self) -> usize {
        self.tiles.iter().map(HashMap::len).sum()
    }

    #[cfg(test)]
    fn iter(&self) -> impl Iterator<Item = (&BoxKey, &BoxEntry)> {
        self.tiles.iter().flatten()
    }
}

/// What a box gathers over the window.
#[derive(Default)]
pub struct BoxEntry {
    pub sums: BoxSums,
    pub ground_m: f64,
    /// Flights per average day that crossed it in each period (a flight passing at 23:00 counts in
    /// the evening and the night, as its energy does), and per period the last flight seen there
    /// (a flight's segments arrive together within a day).
    pub flights_per_day: [f64; PERIODS],
    last_flight: [u64; PERIODS],
    pub kept: Vec<KeptPiece>,
}

impl BoxEntry {
    /// Keeps the piece of `flight` at `keep_level_db` if it is among the `limit` loudest pieces so
    /// far, one per flight; `piece` builds it only then.
    fn keep(
        &mut self,
        (flight, keep_level_db): (u64, f64),
        limit: usize,
        piece: impl FnOnce() -> KeptPiece,
    ) {
        let same_flight = self.kept.iter().position(|kept| kept.flight_id == flight);
        let slot = match same_flight {
            Some(slot) => slot,
            None if self.kept.len() < limit => {
                self.kept.reserve_exact(limit - self.kept.len());
                self.kept.push(piece());
                return;
            }
            None => match (0..self.kept.len())
                .min_by(|&a, &b| self.kept[a].cmp_loudness(&self.kept[b]))
            {
                Some(weakest) => weakest,
                None => return,
            },
        };
        let kept = &self.kept[slot];
        if loudness_order(
            (kept.keep_level_db, kept.flight_id),
            (keep_level_db, flight),
        )
        .is_lt()
        {
            self.kept[slot] = piece();
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
    PlaceAtmosphere::new(&weather.place(lat, lon).alpha_db_per_km)
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

/// The square's tiles a segment's ends' tile box meets (columns and rows within the square), or
/// `None`: a piece's tile lies within its segment's ends' tiles.
fn tiles_met(
    square: Square,
    start: Mercator,
    end: Mercator,
) -> Option<[std::ops::RangeInclusive<u32>; 2]> {
    let last = f64::from(tiles::geo::TILES_PER_AXIS - 1);
    let span = |from: f64, to: f64, first: u32| {
        let low = (from.min(to).floor().clamp(0.0, last) as u32).max(first);
        let high = (from.max(to).floor().clamp(0.0, last) as u32).min(first + TILES_PER_SIDE - 1);
        (low <= high).then(|| low - first..=high - first)
    };
    Some([
        span(start.x, end.x, square.x * TILES_PER_SIDE)?,
        span(start.y, end.y, square.y * TILES_PER_SIDE)?,
    ])
}

/// What a segment adds to the boxes, worked out once for all its pieces and rules.
struct Emitted {
    emission: SegmentEmission,
    energies: SegmentEnergies,
    /// The LAmax its kept pieces name at 1,000 ft (dB).
    lamax_reference_db: f64,
    helicopter: bool,
}

/// Adds one day's segments of `square` with their window weights to the boxes of each rule (its
/// placement and the pieces a box keeps), tile by tile. Each flight's segments must be together: a
/// box counts a flight once while its pieces arrive one after another.
fn add_day(
    boxes: &mut [Boxes],
    rules: &[(&Placement, usize)],
    segments: &[(FlightSegment, f64)],
    (square, place): (Square, &PlaceAtmosphere),
) {
    let point = |end: [f64; 3]| (Mercator::from_degrees(end[0], end[1]), end[2]);
    // Each segment's emission and the square's tiles it can reach (none without weight, outside
    // the square or outside the thrust model's domain).
    let emitted: Vec<Option<(Emitted, [std::ops::RangeInclusive<u32>; 2])>> = segments
        .par_iter()
        .map(|(segment, weight)| {
            if *weight <= 0.0 || usize::from(segment.period) >= PERIODS {
                return None;
            }
            let tiles = tiles_met(square, point(segment.start).0, point(segment.end).0)?;
            let (aircraft, emission) = emission_of(segment)?;
            let emission = emission.in_atmosphere(place);
            let emitted = Emitted {
                energies: SegmentEnergies::of(&emission.npd_distance_levels()),
                lamax_reference_db: emission.lamax_db(LAMAX_REFERENCE_SLANT_M),
                helicopter: aircraft.helicopter.is_some(),
                emission,
            };
            Some((emitted, tiles))
        })
        .collect();
    // Per tile the segments that can reach it, in their order.
    let mut reaching: Vec<Vec<u32>> = vec![Vec::new(); TILES_PER_SQUARE];
    for (index, emitted) in emitted.iter().enumerate() {
        if let Some((_, [columns, rows])) = emitted {
            for row in rows.clone() {
                for column in columns.clone() {
                    reaching[(row * TILES_PER_SIDE + column) as usize].push(index as u32);
                }
            }
        }
    }
    // Each rule's tiles add their pieces in segment order, a tile at a time.
    let mut work: Vec<(&mut TileBoxes, (&Placement, usize), usize)> = boxes
        .iter_mut()
        .zip(rules)
        .flat_map(|(boxes, &rule)| {
            boxes
                .tiles
                .iter_mut()
                .enumerate()
                .map(move |(index, map)| (map, rule, index))
        })
        .collect();
    work.par_iter_mut()
        .for_each(|(map, (placement, pieces), index)| {
            let tile = square.tile(*index);
            let frame = LocalFrame::at(tile.centre());
            let metres = |(position, altitude): (Mercator, f64)| {
                let [east, north] = frame.to_metres(position);
                [east, north, altitude]
            };
            for &segment_index in &reaching[*index] {
                let (segment, weight) = &segments[segment_index as usize];
                let Some((emitted, _)) = &emitted[segment_index as usize] else {
                    continue;
                };
                let period = usize::from(segment.period);
                let mut period_weights = [0.0; PERIODS];
                period_weights[period] = *weight;
                let cut = cut_within_tile(
                    placement,
                    point(segment.start),
                    point(segment.end),
                    emitted.helicopter,
                    tile,
                );
                for piece in &cut {
                    let entry = map.entry(piece.key).or_default();
                    entry.sums.add(
                        &emitted.energies,
                        emitted.emission.installation,
                        period_weights,
                        metres(piece.start),
                        metres(piece.end),
                    );
                    entry.ground_m = piece.ground_m;
                    if entry.last_flight[period] != segment.flight_id {
                        entry.last_flight[period] = segment.flight_id;
                        entry.flights_per_day[period] += *weight;
                    }
                    let keep_level_db = emitted
                        .emission
                        .lamax_db(piece.start.1.min(piece.end.1) - piece.ground_m);
                    entry.keep((segment.flight_id, keep_level_db), *pieces, || {
                        KeptPiece::new(segment, emitted, keep_level_db, piece)
                    });
                }
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
    /// The far boxes, read for tiles beyond 3 km of the click.
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
/// `terrain_root` (a prepared year root) by each of `rules`, the day files read once for all of
/// them; writes them under `out`, returns per rule the number of tiles and boxes written.
pub fn build_square(
    (shuffled, days): (&Path, &[(String, shuffle::DayRoles)]),
    (square, weather): (Square, &WeatherTable),
    terrain_root: &Path,
    rules: &[BoxRule],
    out: &Path,
) -> Result<Vec<(usize, usize)>, String> {
    let place = place_atmosphere(weather, square);
    let mut near: HashSet<TileId> = HashSet::new();
    for tile in square.tiles() {
        near.extend(tile.ring(0));
        near.extend(tile.ring(1));
    }
    let bytes = read_held(terrain_root, tiles::Kind::Terrain, near.iter().copied())?;
    let terrain: HashMap<TileId, Terrain> = bytes
        .iter()
        .map(|(tile, bytes)| Terrain::parse(bytes).map(|terrain| (*tile, terrain)))
        .collect::<Result<_, _>>()
        .map_err(|error| error.to_string())?;
    let placements: Vec<Placement> = rules
        .iter()
        .map(|rule| Placement::new(&near, &terrain, rule.level_step_db))
        .collect();
    let placed: Vec<(&Placement, usize)> = placements
        .iter()
        .zip(rules)
        .map(|(placement, rule)| (placement, rule.pieces))
        .collect();
    let mut boxes: Vec<Boxes> = rules.iter().map(|_| Boxes::default()).collect();
    let counts = (
        days.iter().filter(|(_, roles)| roles.baseline).count(),
        days.iter().filter(|(_, roles)| roles.increment).count(),
    );
    // A day's segments with their weights, each flight's segments together in their order.
    let read_day = |(day, roles): &(String, shuffle::DayRoles)| {
        shuffle::square_day(shuffled, square, day).map(|segments| {
            let mut weighted: Vec<(FlightSegment, f64)> = segments
                .into_iter()
                .map(|segment| {
                    let weight = roles.weight(&segment, counts);
                    (segment, weight)
                })
                .collect();
            weighted.par_sort_by_key(|(segment, _)| segment.flight_id);
            weighted
        })
    };
    // The next day is read while this one is added.
    let mut next = days.first().map(read_day);
    for index in 0..days.len() {
        let segments = next.take().expect("the day read ahead")?;
        let ((), following) = rayon::join(
            || add_day(&mut boxes, &placed, &segments, (square, &place)),
            || days.get(index + 1).map(read_day),
        );
        next = following;
    }
    boxes
        .iter()
        .zip(&placements)
        .zip(rules)
        .map(|((boxes, placement), rule)| {
            let written = write::write_square_boxes(boxes, placement, (rule.kind, square), out)?;
            Ok((written, boxes.len()))
        })
        .collect()
}

#[cfg(test)]
mod tests;

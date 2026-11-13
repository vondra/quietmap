//! The aircraft layer of a click: the receiver's horizons ([`horizons`]), every box of the read
//! tiles through the click-time equation ([`boxes`]), the runways' rolls ([`rolls`]), the loudest
//! flights ([`flights`]) and what flies over the receiver ([`events`]).

pub mod boxes;
pub mod events;
pub mod flights;
pub mod horizons;
pub mod rolls;
pub mod tracks;

use boxes::{AircraftReceiver, FLIGHT_KINDS, tile_energy};
use flights::FlightTotals;
use physics::bands::PERIODS;
use physics::doc29::screening::ReceiverHorizons;
use rayon::prelude::*;
use tiles::aircraft::Aircraft;
use tiles::geo::{LocalFrame, TileId};

/// Boxes computed between two looks at the list's entry level.
const SEARCH_CHUNK: usize = 16;
/// Fine boxes are read for the tiles whose nearest point lies within this of the click, far boxes
/// (four times the edges) for the others: a few kilometres off, the steepest slope is a fraction
/// of the one at a low box's clearance, and far boxes are a seventh of the fine ones.
pub const FINE_BOXES_WITHIN_M: f64 = 3_000.0;

/// Whether `tile` is read with its fine boxes for a click at the origin of `frame`.
pub fn reads_fine_boxes(frame: &LocalFrame, tile: TileId) -> bool {
    frame.tile_gap_m(tile, [0.0, 0.0]) <= FINE_BOXES_WITHIN_M
}

/// Whether `tile` can hold the start of a roll within the rolls' reach of a receiver at `position`
/// (click metres), a metre for the start's rounding to tile steps.
pub fn holds_rolls_in_reach(frame: &LocalFrame, tile: TileId, position: [f64; 2]) -> bool {
    frame.tile_gap_m(tile, position) <= rolls::ROLL_REACH_M + 1.0
}

/// Whether `tile` can hold a box within the aircraft reach of a receiver at `position` (click
/// metres). The builder cuts every flight segment at the z12 tile edges before it cuts it into
/// box pieces (build/src/boxes/place.rs `cut_into_pieces`), so a box's centroid, its pieces'
/// weighted middle, lies in its own tile: a tile further away than the reach holds none in it
/// (a metre for the centroid's rounding to tile steps).
pub fn holds_boxes_in_reach(frame: &LocalFrame, tile: TileId, position: [f64; 2]) -> bool {
    frame.tile_gap_m(tile, position) <= boxes::AIRCRAFT_REACH_M + 1.0
}

/// The classes of the flights' passes: [`PASS_CLASS_DB`] wide from [`PASS_LOWEST_DB`] (dB). The
/// real flights of 34 days of the year past 14 places, each moment a class of its own level, read
/// their time series' loudness within 4 % in classes of 1 and of 3 dB, 4 to 8 % louder in classes
/// of 10 dB.
pub const PASS_CLASSES: usize = 48;
pub const PASS_CLASS_DB: f64 = 3.0;
pub const PASS_LOWEST_DB: f64 = -30.0;

/// The flights heard as passes. While a flight is in a box it is heard at the box's level: the
/// box's received energy over the share of the period its flights are in it (`boxes::box_passing`:
/// their number, the cell's mean chord along their axis, their speed). Per period and class of
/// that level, the boxes' energy (Leq, linear) and their shares of the period: a class is one
/// stream of passes at its mean level, the flights at that level at a moment Poisson with the
/// class's summed share as their mean, the classes independent (`percentiles::Line::flight_passes`).
/// At those 14 places the click reads the real flights' loudness 9 % quieter to 30 % louder, the
/// median 9 % louder: a box takes all its flights at one level, though their levels spread by 2
/// to 5 dB in it, and its chord and kept speeds stand for their times in it (the flights' own
/// times in the boxes read no nearer; with the spread of their levels 0 to 19 % louder, the
/// median 7 %). One Kurze line at the boxes' energy-weighted lambda (the slant
/// over 70 m/s) read it 2 to 157 % louder, the median 73 %: Doc 29's pass is half as long as a
/// point's at the same distance and shorter still far away, cruise flies three times as fast,
/// and one line gives every pass one peak.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlightPasses {
    pub energy: [[f64; PASS_CLASSES]; PERIODS],
    pub passing: [[f64; PASS_CLASSES]; PERIODS],
}

impl Default for FlightPasses {
    fn default() -> Self {
        FlightPasses {
            energy: [[0.0; PASS_CLASSES]; PERIODS],
            passing: [[0.0; PASS_CLASSES]; PERIODS],
        }
    }
}

impl FlightPasses {
    /// Adds passes heard with `energy` (Leq, linear) in `period`, in progress `passing` of it.
    pub fn add_pass(&mut self, period: usize, energy: f64, passing: f64) {
        if energy <= 0.0 || passing <= 0.0 {
            return;
        }
        let level_db = 10.0 * (energy / passing).log10();
        let class =
            (((level_db - PASS_LOWEST_DB) / PASS_CLASS_DB).max(0.0) as usize).min(PASS_CLASSES - 1);
        self.energy[period][class] += energy;
        self.passing[period][class] += passing;
    }

    pub fn add(&mut self, other: &FlightPasses) {
        for period in 0..PERIODS {
            for class in 0..PASS_CLASSES {
                self.energy[period][class] += other.energy[period][class];
                self.passing[period][class] += other.passing[period][class];
            }
        }
    }
}

/// One tile of a ring and the boxes the receiver hears of it: its fine ones within
/// [`FINE_BOXES_WITHIN_M`], else its far ones.
#[derive(Clone, Copy)]
pub struct RingTile<'a> {
    pub tile: TileId,
    pub heard: Aircraft<'a>,
}

/// One ring's boxes at the receiver.
pub struct RingAircraft {
    /// Period energies (Leq, linear).
    pub energy: [f64; PERIODS],
    /// The flights' passes.
    pub passes: FlightPasses,
    /// The Lden energies per flight kind ([`boxes::FLIGHT_KINDS`]).
    pub kinds: [f64; FLIGHT_KINDS],
    /// Boxes within reach.
    pub heard: usize,
}

/// One ring's aircraft at the receiver: every box of its tiles through the click-time equation,
/// and into `flights` the kept pieces
/// of every box whose bound on their LAmax is above the list's entry level, loudest bound first
/// (so the list holds the loudest flights of all kept pieces, rings read so far).
pub fn ring_aircraft(
    tiles: &[RingTile<'_>],
    frame: &LocalFrame,
    receiver: AircraftReceiver,
    horizons: &(impl ReceiverHorizons + Sync),
    flights: &mut FlightTotals,
) -> RingAircraft {
    let mut ring = RingAircraft {
        energy: [0.0; PERIODS],
        passes: FlightPasses::default(),
        kinds: [0.0; FLIGHT_KINDS],
        heard: 0,
    };
    let mut bounds: Vec<(f64, usize, usize)> = Vec::new();
    for (tile_index, ring_tile) in tiles.iter().enumerate() {
        let answer = tile_energy(&ring_tile.heard, ring_tile.tile, frame, receiver, horizons);
        for period in 0..PERIODS {
            ring.energy[period] += answer.energy[period];
        }
        ring.passes.add(&answer.passes);
        for (kind, tile_kind) in ring.kinds.iter_mut().zip(answer.kinds) {
            *kind += tile_kind;
        }
        ring.heard += answer.boxes;
        bounds.extend(
            answer
                .lamax_bounds
                .iter()
                .map(|&(index, bound)| (bound, tile_index, index)),
        );
    }
    let heard: Vec<(TileId, Aircraft)> = (tiles.iter())
        .map(|ring_tile| (ring_tile.tile, ring_tile.heard))
        .collect();
    read_loudest(bounds, &heard, (frame, receiver, horizons), flights);
    ring
}

/// The kept pieces of every box whose bound on their LAmax is above the list's entry level into
/// `flights`, loudest bound first.
fn read_loudest(
    mut bounds: Vec<(f64, usize, usize)>,
    tiles: &[(TileId, Aircraft<'_>)],
    at: (
        &LocalFrame,
        AircraftReceiver,
        &(impl ReceiverHorizons + Sync),
    ),
    flights: &mut FlightTotals,
) {
    bounds.par_sort_unstable_by(loudest_first);
    read_loudest_in_order(bounds.into_iter(), tiles, at, flights);
}

/// The order the loudest flights' search reads boxes in: by their bound, loudest first, then by
/// tile and index.
pub fn loudest_first(a: &(f64, usize, usize), b: &(f64, usize, usize)) -> std::cmp::Ordering {
    b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2))
}

/// [`read_loudest`] over the boxes' bounds already in [`loudest_first`] order (bound, index into
/// `tiles`, box index), read a chunk at a time while the chunk's loudest bound passes the list's
/// entry level.
pub fn read_loudest_in_order(
    mut ordered: impl Iterator<Item = (f64, usize, usize)>,
    tiles: &[(TileId, Aircraft<'_>)],
    (frame, receiver, horizons): (
        &LocalFrame,
        AircraftReceiver,
        &(impl ReceiverHorizons + Sync),
    ),
    flights: &mut FlightTotals,
) {
    loop {
        let chunk: Vec<(f64, usize, usize)> = ordered.by_ref().take(SEARCH_CHUNK).collect();
        let entry_lmax_db = flights.entry_lmax_db();
        if chunk.is_empty() || chunk[0].0 <= entry_lmax_db {
            break;
        }
        for &(_, tile_index, index) in &chunk {
            let (tile, boxes) = &tiles[tile_index];
            flights.add_boxes(boxes, *tile, &[index], frame, receiver, entry_lmax_db);
        }
    }
    flights.sum_listed(receiver.altitude_m, horizons);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiles::geo::Mercator;

    /// Around Prague a z12 tile is 6.28 km wide; from the centre of the click's tile the third tile
    /// east begins 15.7 km away, within the 16 km reach, the fifth 28.3 km away: no box of it can
    /// be heard, unless the receiver stands 13 km nearer it (the rule follows the receiver).
    #[test]
    fn a_tile_beyond_the_reach_holds_no_box_in_reach() {
        let centre = TileId::containing(Mercator::from_degrees(50.08, 14.42));
        let frame = LocalFrame::at(centre.centre());
        let east = |tiles: u32| TileId {
            x: centre.x + tiles,
            ..centre
        };
        assert!(holds_boxes_in_reach(&frame, east(3), [0.0, 0.0]));
        assert!(!holds_boxes_in_reach(&frame, east(5), [0.0, 0.0]));
        assert!(holds_boxes_in_reach(&frame, east(5), [13_000.0, 0.0]));
        assert!(!reads_fine_boxes(&frame, east(2)));
    }
}

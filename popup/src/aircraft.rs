//! The aircraft layer of a click: the receiver's horizons ([`horizons`]), every box of the read
//! tiles through the click-time equation ([`boxes`]), the runways' rolls ([`rolls`]), the loudest
//! flights ([`flights`]) and what flies over the receiver ([`events`]).

pub mod boxes;
pub mod events;
pub mod flights;
pub mod horizons;
pub mod rolls;
pub mod tracks;

use boxes::{AIRCRAFT_REACH_M, AircraftReceiver, FLIGHT_KINDS, tile_energy, tile_flight_sums};
use flights::FlightTotals;
use physics::bands::PERIODS;
use physics::doc29::screening::ReceiverHorizons;
use rayon::prelude::*;
use tiles::aircraft::Aircraft;
use tiles::geo::{LocalFrame, TileId};

/// Boxes computed between two looks at the list's entry level.
const SEARCH_CHUNK: usize = 16;
/// Far boxes a core sums at a time for the lambda (`tile_energy`'s run).
const FAR_RUN: usize = 1_024;
/// Fine boxes are read for the tiles whose nearest point lies within this of the click, far boxes
/// (four times the edges) for the others: a few kilometres off, the steepest slope is a fraction
/// of the one at a low box's clearance, and far boxes are a seventh of the fine ones.
pub const FINE_BOXES_WITHIN_M: f64 = 3_000.0;

/// Whether `tile` is read with its fine boxes for a click at the origin of `frame`.
pub fn reads_fine_boxes(frame: &LocalFrame, tile: TileId) -> bool {
    tile_gap_m(frame, tile, [0.0, 0.0]) <= FINE_BOXES_WITHIN_M
}

/// Whether `tile` can hold the start of a roll within the rolls' reach of a receiver at `position`
/// (click metres), a metre for the start's rounding to tile steps.
pub fn holds_rolls_in_reach(frame: &LocalFrame, tile: TileId, position: [f64; 2]) -> bool {
    tile_gap_m(frame, tile, position) <= rolls::ROLL_REACH_M + 1.0
}

/// Whether `tile` can hold a box within the aircraft reach of a receiver at `position` (click
/// metres). The builder cuts every flight segment at the z12 tile edges before it cuts it into
/// box pieces (build/src/boxes/place.rs `cut_into_pieces`), so a box's centroid, its pieces'
/// weighted middle, lies in its own tile: a tile further away than the reach holds none in it
/// (a metre for the centroid's rounding to tile steps).
pub fn holds_boxes_in_reach(frame: &LocalFrame, tile: TileId, position: [f64; 2]) -> bool {
    tile_gap_m(frame, tile, position) <= boxes::AIRCRAFT_REACH_M + 1.0
}

/// The horizontal distance (m) from `position` (click metres) to the nearest point of `tile`.
fn tile_gap_m(frame: &LocalFrame, tile: TileId, position: [f64; 2]) -> f64 {
    let corner = |dx: f64, dy: f64| {
        let m = frame.to_metres(tiles::geo::Mercator {
            x: f64::from(tile.x) + dx,
            y: f64::from(tile.y) + dy,
        });
        [m[0] - position[0], m[1] - position[1]]
    };
    let (a, b) = (corner(0.0, 0.0), corner(1.0, 1.0));
    let gap = |low: f64, high: f64| (low.min(high)).max(-high.max(low)).max(0.0);
    gap(a[0], b[0]).hypot(gap(a[1], b[1]))
}

/// The flights' Kurze lambda, their energy-weighted mean over the far boxes of the tiles heard
/// (also where their fine boxes give the energy) and the runways' rolls: per period those boxes'
/// and rolls' own energy and energy times lambda. A box
/// counts the flights crossing it, so its lambda depends on how the boxes group the flights; read
/// from one grouping everywhere, the lambda does not change where a tile's fine boxes give way to
/// its far ones. Of
/// the two, the far boxes' gave the loudness nearest to the boxes' crossings simulated as streams
/// one by one (measured 2026-11-08 at 23 places: the far boxes' within 2.5 phon, the fine boxes' 5
/// to 8 phon quieter).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LambdaSums {
    pub energy: [f64; PERIODS],
    pub energy_lambda: [f64; PERIODS],
}

impl From<([f64; PERIODS], [f64; PERIODS])> for LambdaSums {
    /// Energies and energies times lambda, as `boxes::tile_flight_sums` sums them.
    fn from((energy, energy_lambda): ([f64; PERIODS], [f64; PERIODS])) -> Self {
        LambdaSums {
            energy,
            energy_lambda,
        }
    }
}

impl LambdaSums {
    pub fn add(&mut self, other: &LambdaSums) {
        for period in 0..PERIODS {
            self.energy[period] += other.energy[period];
            self.energy_lambda[period] += other.energy_lambda[period];
        }
    }

    /// The flights' lambda per period (infinite with no energy: a steady line).
    pub fn lambda(&self) -> [f64; PERIODS] {
        std::array::from_fn(|period| {
            if self.energy[period] > 0.0 {
                self.energy_lambda[period] / self.energy[period]
            } else {
                f64::INFINITY
            }
        })
    }
}

/// One tile of a ring: the boxes the receiver hears (its fine ones within
/// [`FINE_BOXES_WITHIN_M`], else its far ones) and, where those are its fine ones, its far ones,
/// which the flights' lambda is read from ([`LambdaSums`]; `None`: the heard ones).
#[derive(Clone, Copy)]
pub struct RingTile<'a> {
    pub tile: TileId,
    pub heard: Aircraft<'a>,
    pub far: Option<Aircraft<'a>>,
}

/// One ring's boxes at the receiver.
pub struct RingAircraft {
    /// Period energies (Leq, linear).
    pub energy: [f64; PERIODS],
    /// What the flights' lambda is weighted from.
    pub lambda: LambdaSums,
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
        lambda: LambdaSums::default(),
        kinds: [0.0; FLIGHT_KINDS],
        heard: 0,
    };
    // The far boxes of the tiles heard in their fine ones, for the lambda only: in runs of boxes
    // over every core (the click's own tile alone may hold them), added in order, so that a click
    // always sums them alike.
    let runs: Vec<(usize, std::ops::Range<usize>)> = (tiles.iter().enumerate())
        .filter_map(|(at, ring_tile)| ring_tile.far.map(|far| (at, far.box_count())))
        .flat_map(|(at, count)| {
            (0..count)
                .step_by(FAR_RUN)
                .map(move |start| (at, start..(start + FAR_RUN).min(count)))
        })
        .collect();
    let run_lambdas: Vec<LambdaSums> = runs
        .par_iter()
        .map(|(at, boxes)| {
            let ring_tile = &tiles[*at];
            let far = ring_tile.far.expect("a run of far boxes");
            let whole: &dyn Fn(f64) -> f64 = &|_| 1.0;
            let reach = (boxes.clone(), whole, AIRCRAFT_REACH_M);
            tile_flight_sums(&far, ring_tile.tile, frame, receiver, horizons, reach).into()
        })
        .collect();
    let mut far_lambdas: Vec<Option<LambdaSums>> = (tiles.iter())
        .map(|ring_tile| ring_tile.far.map(|_| LambdaSums::default()))
        .collect();
    for ((at, _), run) in runs.iter().zip(&run_lambdas) {
        if let Some(sums) = &mut far_lambdas[*at] {
            sums.add(run);
        }
    }
    let mut bounds: Vec<(f64, usize, usize)> = Vec::new();
    for (tile_index, (ring_tile, far_lambda)) in tiles.iter().zip(far_lambdas).enumerate() {
        let answer = tile_energy(&ring_tile.heard, ring_tile.tile, frame, receiver, horizons);
        for period in 0..PERIODS {
            ring.energy[period] += answer.energy[period];
        }
        ring.lambda
            .add(&far_lambda.unwrap_or((answer.energy, answer.energy_lambda).into()));
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

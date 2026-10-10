//! The aircraft layer of a click: the receiver's horizons ([`horizons`]), every box of the read
//! tiles through the click-time equation ([`boxes`]), the loudest flights ([`flights`]) and what
//! flies over the receiver ([`events`]).

pub mod boxes;
pub mod events;
pub mod flights;
pub mod horizons;
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
    tile_gap_m(frame, tile, [0.0, 0.0]) <= FINE_BOXES_WITHIN_M
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

/// One ring's boxes at the receiver.
pub struct RingAircraft {
    /// Period energies (Leq, linear).
    pub energy: [f64; PERIODS],
    /// The period energies times their flights' Kurze lambda.
    pub energy_lambda: [f64; PERIODS],
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
    tiles: &[(TileId, Aircraft<'_>)],
    frame: &LocalFrame,
    receiver: AircraftReceiver,
    horizons: &(impl ReceiverHorizons + Sync),
    flights: &mut FlightTotals,
) -> RingAircraft {
    let mut ring = RingAircraft {
        energy: [0.0; PERIODS],
        energy_lambda: [0.0; PERIODS],
        kinds: [0.0; FLIGHT_KINDS],
        heard: 0,
    };
    let mut bounds: Vec<(f64, usize, usize)> = Vec::new();
    for (tile_index, (tile, boxes)) in tiles.iter().enumerate() {
        let answer = tile_energy(boxes, *tile, frame, receiver, horizons);
        for period in 0..PERIODS {
            ring.energy[period] += answer.energy[period];
            ring.energy_lambda[period] += answer.energy_lambda[period];
        }
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
    bounds.par_sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    for chunk in bounds.chunks(SEARCH_CHUNK) {
        if chunk[0].0 <= flights.entry_lmax_db() {
            break;
        }
        for &(_, tile_index, index) in chunk {
            let (tile, boxes) = &tiles[tile_index];
            flights.add_boxes(boxes, *tile, &[index], frame, receiver, horizons);
        }
    }
    ring
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

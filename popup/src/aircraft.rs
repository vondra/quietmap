//! The aircraft layer of a click: the receiver's horizons ([`horizons`]), every box of the read
//! tiles through the click-time equation ([`boxes`]), and the loudest flights ([`flights`]).

pub mod boxes;
pub mod flights;
pub mod horizons;

use boxes::{AircraftReceiver, tile_energy};
use flights::FlightTotals;
use physics::bands::PERIODS;
use physics::doc29::screening::ReceiverHorizons;
use rayon::prelude::*;
use tiles::aircraft::Aircraft;
use tiles::geo::{LocalFrame, TileId};

/// Boxes computed between two looks at the list's entry level.
const SEARCH_CHUNK: usize = 16;

/// One ring's aircraft at the receiver: every box of its tiles through the click-time equation
/// (period energies, linear, and the number of boxes heard), and into `flights` the kept pieces
/// of every box whose bound on their LAmax is above the list's entry level, loudest bound first
/// (so the list holds the loudest flights of all kept pieces, rings read so far).
pub fn ring_aircraft(
    tiles: &[(TileId, Aircraft<'_>)],
    frame: &LocalFrame,
    receiver: AircraftReceiver,
    horizons: &(impl ReceiverHorizons + Sync),
    flights: &mut FlightTotals,
) -> ([f64; PERIODS], usize) {
    let (mut energy, mut heard) = ([0.0; PERIODS], 0);
    let mut bounds: Vec<(f64, usize, usize)> = Vec::new();
    for (tile_index, (tile, boxes)) in tiles.iter().enumerate() {
        let answer = tile_energy(boxes, *tile, frame, receiver, horizons);
        for (total, value) in energy.iter_mut().zip(answer.energy) {
            *total += value;
        }
        heard += answer.boxes;
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
    (energy, heard)
}

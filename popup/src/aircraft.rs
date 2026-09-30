//! The aircraft layer of a click: the receiver's horizons ([`horizons`]), every box of the read
//! tiles through the click-time equation ([`boxes`]), and the loudest flights ([`flights`]).

pub mod boxes;
pub mod flights;
pub mod horizons;

use boxes::{AircraftReceiver, tile_energy};
#[cfg(doc)]
use flights::BOXES_SEARCHED;
use flights::FlightTotals;
use physics::bands::PERIODS;
use physics::doc29::screening::ReceiverHorizons;
use tiles::aircraft::Aircraft;
use tiles::geo::{LocalFrame, TileId};

/// One ring's aircraft at the receiver: every box of its tiles through the click-time equation
/// (period energies, linear, and the number of boxes heard), and the kept pieces of the ring's
/// loudest boxes ([`BOXES_SEARCHED`] in the popup) into `flights`.
pub fn ring_aircraft(
    tiles: &[(TileId, Aircraft<'_>)],
    frame: &LocalFrame,
    receiver: AircraftReceiver,
    horizons: &(impl ReceiverHorizons + Sync),
    flights: &mut FlightTotals,
) -> ([f64; PERIODS], usize) {
    let (mut energy, mut heard) = ([0.0; PERIODS], 0);
    let mut loudest_boxes: Vec<(f64, usize, usize)> = Vec::new();
    for (tile_index, (tile, boxes)) in tiles.iter().enumerate() {
        let answer = tile_energy(
            boxes,
            *tile,
            frame,
            receiver,
            horizons,
            flights.boxes_searched,
        );
        for (total, value) in energy.iter_mut().zip(answer.energy) {
            *total += value;
        }
        heard += answer.boxes;
        loudest_boxes.extend(
            answer
                .loudest
                .iter()
                .map(|&(index, energy)| (energy, tile_index, index)),
        );
    }
    loudest_boxes.sort_by(|a, b| b.0.total_cmp(&a.0));
    loudest_boxes.truncate(flights.boxes_searched);
    for (tile_index, (tile, boxes)) in tiles.iter().enumerate() {
        let chosen: Vec<usize> = loudest_boxes
            .iter()
            .filter(|&&(_, owner, _)| owner == tile_index)
            .map(|&(_, _, index)| index)
            .collect();
        flights.add_boxes(boxes, *tile, &chosen, frame, receiver, horizons);
    }
    (energy, heard)
}

//! The aircraft file of each tile: its boxes in (band, row, column, group) order with their sums'
//! values, the kept pieces behind them, and the flights those pieces belong to.

use super::place::Placement;
use super::read::{FLAG_DEPARTURE, FLAG_HELICOPTER_DESCENT};
use super::{BoxEntry, Boxes, TileBoxes, place::BoxKey};
use crate::dev4::Square;
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::Path;
use tiles::Kind;
use tiles::aircraft::{AircraftBox, BoxPlace, Flight, FlightPiece, Group, encode};
use tiles::geo::{LocalFrame, Mercator, TileId};
use tiles::square::SquareWriter;

/// Two ends (position, altitude above sea level) in `tile`'s frame: metres east and north of the
/// tile centre (the frame every box of the tile sums in).
fn ends_metres(tile: TileId, start: (Mercator, f64), end: (Mercator, f64)) -> [[f64; 3]; 2] {
    let frame = LocalFrame::at(tile.centre());
    let at = |(position, altitude): (Mercator, f64)| {
        let [east, north] = frame.to_metres(position);
        [east, north, altitude]
    };
    [at(start), at(end)]
}

/// Tile-local int16 steps of a point in its tile's frame (clamped into the frame's margin).
fn local_steps(tile: TileId, frame: &LocalFrame, metres: [f64; 2]) -> [i16; 2] {
    let global = frame.steps_of_metres(metres);
    let centre = tile.centre_steps();
    [
        (global[0] - centre.x as f64)
            .round()
            .clamp(-32_768.0, 32_767.0) as i16,
        (global[1] - centre.y as f64)
            .round()
            .clamp(-32_768.0, 32_767.0) as i16,
    ]
}

fn flight_of(flight_id: u64, callsign: [u8; 8], designator: [u8; 4]) -> Flight {
    Flight {
        icao: (flight_id >> 40) as u32 & 0x00ff_ffff,
        callsign,
        type_designator: designator,
        start_unix: flight_id as u32,
    }
}

/// Writes the boxes of one square (their bands from `placement`) as its file of `kind`, its tiles
/// encoded at once; returns how many tiles hold boxes.
pub fn write_square_boxes(
    boxes: &Boxes,
    placement: &Placement,
    (kind, square): (Kind, Square),
    out: &Path,
) -> Result<usize, String> {
    let encoded: Vec<Option<Vec<u8>>> = boxes
        .tiles
        .par_iter()
        .enumerate()
        .map(|(index, map)| encode_tile(square.tile(index), map, placement))
        .collect();
    let mut writer = SquareWriter::create(out, kind, square)?;
    for (index, bytes) in encoded.iter().enumerate() {
        if let Some(bytes) = bytes {
            writer.add(square.tile(index), bytes)?;
        }
    }
    writer.finish()
}

/// One tile's file: its boxes in (band, row, column, group) order, `None` without any.
fn encode_tile(tile: TileId, boxes: &TileBoxes, placement: &Placement) -> Option<Vec<u8>> {
    let mut tile_boxes: Vec<(&BoxKey, &BoxEntry)> = boxes.iter().collect();
    tile_boxes.sort_by_key(|(key, _)| (key.band, key.cell[1], key.cell[0], key.helicopter));
    let frame = LocalFrame::at(tile.centre());
    let bands = placement.bands(tile);
    let (mut records, mut flights, mut pieces) = (Vec::new(), Vec::new(), Vec::new());
    let mut flight_index: HashMap<u64, u32> = HashMap::new();
    for (key, entry) in tile_boxes {
        let Some(values) = entry.sums.values() else {
            continue;
        };
        let band = bands[usize::from(key.band)];
        let first_piece = pieces.len() as u32;
        let mut kept = entry.kept.clone();
        kept.sort_by(|a, b| b.cmp_loudness(a));
        for kept in &kept {
            let flight = *flight_index.entry(kept.flight_id).or_insert_with(|| {
                flights.push(flight_of(kept.flight_id, kept.callsign, kept.designator));
                (flights.len() - 1) as u32
            });
            let [start, end] = ends_metres(tile, kept.start, kept.end);
            let flags = (kept.flags & FLAG_DEPARTURE != 0) as u8
                | ((kept.flags & FLAG_HELICOPTER_DESCENT != 0) as u8) << 1;
            pieces.push(FlightPiece {
                flight,
                ends: [
                    local_steps(tile, &frame, [start[0], start[1]]),
                    local_steps(tile, &frame, [end[0], end[1]]),
                ],
                altitudes_m: [start[2], end[2]],
                speed_kt: f64::from(kept.speed_kt),
                class: kept.class,
                power_code: kept.power_code,
                flags,
                period: kept.period,
            });
        }
        let cells_per_side = 1u32 << (band.zoom - 12);
        records.push(AircraftBox {
            place: BoxPlace {
                zoom: band.zoom,
                cell: [
                    (key.cell[0] - tile.x * cells_per_side) as u16,
                    (key.cell[1] - tile.y * cells_per_side) as u16,
                ],
                group: if key.helicopter {
                    Group::Helicopter
                } else {
                    Group::FixedWing
                },
                ground_m: entry.ground_m,
                clearance_m: band.clearance_m,
                height_m: band.edge_m,
                centroid: local_steps(tile, &frame, [values.centroid_m[0], values.centroid_m[1]]),
                first_piece,
                piece_count: kept.len() as u8,
                loudest_lamax_db: kept
                    .iter()
                    .map(|kept| kept.lamax_reference_db)
                    .fold(f64::NEG_INFINITY, f64::max),
            },
            centroid_altitude_m: values.centroid_m[2],
            axis_rad: values.axis_rad,
            gradient: values.gradient.clamp(-3.0, 3.0),
            gradient_spread: values.gradient_spread.min(6.0),
            piece_length_m: values.piece_length_m,
            flights_per_day: entry.flights_per_day,
            energy_db: values.levels_db,
            tail_energy_db: values.tail_levels_db,
            lg_scaled_distance: values.scaled_distance_m.map(f64::log10),
            installation_shares: values.installation_shares,
        });
    }
    (!records.is_empty()).then(|| encode(&records, &flights, &pieces))
}

//! The aircraft file of each tile: its boxes in (band, row, column, group) order with their sums'
//! values, the kept pieces behind them, and the flights those pieces belong to.

use super::place::{BoxPiece, Placement};
use super::read::{FLAG_DEPARTURE, FLAG_HELICOPTER_DESCENT};
use super::{BoxEntry, Boxes, place::BoxKey};
use crate::output::write_tile;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use tiles::Kind;
use tiles::aircraft::{AircraftBox, Flight, FlightPiece, Group, encode};
use tiles::geo::{LocalFrame, Mercator, TileId};

/// Days in the year the flight counts are stated for.
const DAYS_PER_YEAR: f64 = 365.25;

/// A piece's ends in its tile's frame: metres east and north of the tile centre, altitude above
/// sea level (the frame every box of the tile sums in).
pub fn piece_metres(piece: &BoxPiece) -> [[f64; 3]; 2] {
    ends_metres(piece.key.tile, piece.start, piece.end)
}

/// Two ends (position, altitude above sea level) in `tile`'s frame.
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

/// Writes one aircraft file of `kind` per tile holding boxes (their bands from `placement`);
/// returns how many were written.
pub fn write_tiles(
    boxes: &Boxes,
    placement: &Placement,
    kind: Kind,
    out: &Path,
) -> Result<usize, String> {
    let mut by_tile: BTreeMap<TileId, Vec<(&BoxKey, &BoxEntry)>> = BTreeMap::new();
    for (key, entry) in boxes.iter() {
        by_tile.entry(key.tile).or_default().push((key, entry));
    }
    let mut written = 0;
    for (tile, mut tile_boxes) in by_tile {
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
            kept.sort_by(|a, b| b.rank.total_cmp(&a.rank));
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
                centroid_altitude_m: values.centroid_m[2],
                axis_rad: values.axis_rad,
                gradient: values.gradient.clamp(-3.0, 3.0),
                piece_length_m: values.piece_length_m,
                flights: (entry.flights_per_day * DAYS_PER_YEAR).round() as u32,
                energy_db: values.levels_db,
                tail_energy_db: values.tail_levels_db,
                scaled_distance_m: values.scaled_distance_m,
                installation_shares: values.installation_shares,
                first_piece,
                piece_count: kept.len() as u8,
                loudest_lamax_db: kept
                    .iter()
                    .map(|kept| kept.lamax_reference_db)
                    .fold(f64::NEG_INFINITY, f64::max),
            });
        }
        if records.is_empty() {
            continue;
        }
        write_tile(out, tile, kind, &encode(&records, &flights, &pieces))?;
        written += 1;
    }
    Ok(written)
}

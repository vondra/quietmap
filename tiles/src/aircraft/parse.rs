//! Reading an aircraft file in place: the records decode on demand.

use super::{
    AircraftBox, BOX_BYTES, FLIGHT_BYTES, Flight, FlightPiece, Group, HEADER_BYTES, INSTALLATIONS,
    MAGIC, NPD_DISTANCES, PERIODS, PIECE_BYTES, level_db,
};
use crate::FormatError;

/// A parsed aircraft file borrowing its bytes.
#[derive(Clone, Copy)]
pub struct Aircraft<'a> {
    boxes: &'a [u8],
    flights: &'a [u8],
    pieces: &'a [u8],
}

fn u16_at(record: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([record[at], record[at + 1]])
}

fn i16_at(record: &[u8], at: usize) -> i16 {
    i16::from_le_bytes([record[at], record[at + 1]])
}

fn u32_at(record: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(record[at..at + 4].try_into().unwrap())
}

impl<'a> Aircraft<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, FormatError> {
        if bytes.len() < HEADER_BYTES || &bytes[..8] != MAGIC {
            return Err(FormatError("aircraft: bad magic"));
        }
        let count = |at: usize| u32_at(bytes, at) as usize;
        let (boxes, flights, pieces) = (count(8), count(12), count(16));
        let boxes_end = HEADER_BYTES + BOX_BYTES * boxes;
        let flights_end = boxes_end + FLIGHT_BYTES * flights;
        if bytes.len() != flights_end + PIECE_BYTES * pieces {
            return Err(FormatError("aircraft: length does not match the counts"));
        }
        let parsed = Aircraft {
            boxes: &bytes[HEADER_BYTES..boxes_end],
            flights: &bytes[boxes_end..flights_end],
            pieces: &bytes[flights_end..],
        };
        for index in 0..boxes {
            let record = parsed.box_record(index);
            let (zoom, first, count) = (record[0], u32_at(record, 98), usize::from(record[102]));
            if !(12..=super::MAXIMUM_ZOOM).contains(&zoom) || record[1] > 1 {
                return Err(FormatError("aircraft: bad box zoom or group"));
            }
            let cells = 1u32 << (zoom - 12);
            if u32::from(u16_at(record, 2)) >= cells || u32::from(u16_at(record, 4)) >= cells {
                return Err(FormatError("aircraft: box cell outside the tile"));
            }
            if first as usize + count > pieces {
                return Err(FormatError("aircraft: a box names a missing piece"));
            }
        }
        for index in 0..pieces {
            if u32_at(parsed.piece_record(index), 0) as usize >= flights {
                return Err(FormatError("aircraft: a piece names a missing flight"));
            }
        }
        Ok(parsed)
    }

    pub fn box_count(&self) -> usize {
        self.boxes.len() / BOX_BYTES
    }

    pub fn flight_count(&self) -> usize {
        self.flights.len() / FLIGHT_BYTES
    }

    pub fn piece_count(&self) -> usize {
        self.pieces.len() / PIECE_BYTES
    }

    fn box_record(&self, index: usize) -> &'a [u8] {
        &self.boxes[index * BOX_BYTES..(index + 1) * BOX_BYTES]
    }

    fn piece_record(&self, index: usize) -> &'a [u8] {
        &self.pieces[index * PIECE_BYTES..(index + 1) * PIECE_BYTES]
    }

    pub fn aircraft_box(&self, index: usize) -> AircraftBox {
        let record = self.box_record(index);
        let energy = |at: usize| level_db(u16_at(record, at));
        let (wing, fuselage) = (f64::from(record[96]) / 255.0, f64::from(record[97]) / 255.0);
        let mut installation_shares = [wing, fuselage, 0.0];
        installation_shares[INSTALLATIONS - 1] = (1.0 - wing - fuselage).max(0.0);
        AircraftBox {
            zoom: record[0],
            cell: [u16_at(record, 2), u16_at(record, 4)],
            group: if record[1] == 0 {
                Group::FixedWing
            } else {
                Group::Helicopter
            },
            floor_m: f64::from(i16_at(record, 6)),
            height_m: f64::from(u16_at(record, 8)),
            centroid: [i16_at(record, 10), i16_at(record, 12)],
            centroid_altitude_m: f64::from(i16_at(record, 14)),
            axis_rad: f64::from(u16_at(record, 16)) / 65_536.0 * std::f64::consts::PI,
            gradient: f64::from(i16_at(record, 18)) / 10_000.0,
            piece_length_m: f64::from(u16_at(record, 20)),
            flights: u32_at(record, 22),
            energy_db: std::array::from_fn(|period| {
                std::array::from_fn(|distance| energy(26 + 2 * (period * NPD_DISTANCES + distance)))
            }),
            sel_minus_lamax_db: std::array::from_fn(|distance| {
                f64::from(record[86 + distance]) / 10.0
            }),
            installation_shares,
            first_piece: u32_at(record, 98),
            piece_count: record[102],
        }
    }

    pub fn flight(&self, index: usize) -> Flight {
        let record = &self.flights[index * FLIGHT_BYTES..(index + 1) * FLIGHT_BYTES];
        Flight {
            icao: u32::from_le_bytes([record[0], record[1], record[2], 0]),
            callsign: record[3..11].try_into().unwrap(),
            type_designator: record[11..15].try_into().unwrap(),
            start_unix: u32_at(record, 15),
        }
    }

    pub fn piece(&self, index: usize) -> FlightPiece {
        let record = self.piece_record(index);
        FlightPiece {
            flight: u32_at(record, 0),
            ends: [
                [i16_at(record, 4), i16_at(record, 6)],
                [i16_at(record, 8), i16_at(record, 10)],
            ],
            altitudes_m: [f64::from(i16_at(record, 12)), f64::from(i16_at(record, 14))],
            speed_kt: f64::from(u16_at(record, 16)) / 10.0,
            profile: u16_at(record, 18),
            power_code: u16_at(record, 20),
            flags: record[22],
            period: record[23],
        }
    }
}

const _: () = assert!(26 + 2 * PERIODS * NPD_DISTANCES == 86);

//! Reading an aircraft file in place: the records decode on demand.

use super::codes::{
    cell_steps, curve_levels, flights, in_cell, lamax_db, lg_distances, offset, tail_db,
};
use super::{
    AircraftBox, BAND_BYTES, BLOCK_BYTES, BOX_BYTES, BOXES_PER_BLOCK, FLIGHT_BYTES, Flight,
    FlightPiece, Group, HEAD_HELICOPTER, HEAD_PIECES_SHIFT, HEADER_BYTES, INSTALLATIONS, MAGIC,
    MAXIMUM_ZOOM, MIX_BYTES, NPD_DISTANCES, PERIOD_BYTES, PERIODS, PIECE_BYTES,
};
use crate::FormatError;

/// A parsed aircraft file borrowing its bytes.
#[derive(Clone, Copy)]
pub struct Aircraft<'a> {
    bands: &'a [u8],
    mixes: &'a [u8],
    blocks: &'a [u8],
    heads: &'a [u8],
    boxes: &'a [u8],
    periods: &'a [u8],
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

fn u24_at(record: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([record[at], record[at + 1], record[at + 2], 0])
}

fn heard_periods(head: u8) -> u32 {
    (head & 7).count_ones()
}

fn piece_count(head: u8) -> u32 {
    u32::from(head >> HEAD_PIECES_SHIFT & 3)
}

impl<'a> Aircraft<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, FormatError> {
        if bytes.len() < HEADER_BYTES || &bytes[..8] != MAGIC {
            return Err(FormatError("aircraft: bad magic"));
        }
        let count = |at: usize| u32_at(bytes, at) as usize;
        let (boxes, periods, flights, pieces, bands, mixes) = (
            count(8),
            count(12),
            count(16),
            count(20),
            count(24),
            count(28),
        );
        let lengths = [
            BAND_BYTES * bands,
            MIX_BYTES * mixes,
            BLOCK_BYTES * boxes.div_ceil(BOXES_PER_BLOCK),
            boxes,
            BOX_BYTES * boxes,
            PERIOD_BYTES * periods,
            FLIGHT_BYTES * flights,
            PIECE_BYTES * pieces,
        ];
        let mut end = HEADER_BYTES;
        let sections = lengths.map(|length| {
            let start = end;
            end = end.saturating_add(length);
            start..end
        });
        if bands > 256 || bytes.len() != end {
            return Err(FormatError("aircraft: length does not match the counts"));
        }
        let [
            bands_at,
            mixes_at,
            blocks_at,
            heads_at,
            boxes_at,
            periods_at,
            flights_at,
            pieces_at,
        ] = sections;
        let parsed = Aircraft {
            bands: &bytes[bands_at],
            mixes: &bytes[mixes_at],
            blocks: &bytes[blocks_at],
            heads: &bytes[heads_at],
            boxes: &bytes[boxes_at],
            periods: &bytes[periods_at],
            flights: &bytes[flights_at],
            pieces: &bytes[pieces_at],
        };
        for band in 0..bands {
            if !(12..=MAXIMUM_ZOOM).contains(&parsed.bands[BAND_BYTES * band]) {
                return Err(FormatError("aircraft: bad band zoom"));
            }
        }
        let (mut periods_before, mut pieces_before) = (0u32, 0u32);
        for index in 0..boxes {
            if index % BOXES_PER_BLOCK == 0 {
                let block = &parsed.blocks[BLOCK_BYTES * (index / BOXES_PER_BLOCK)..];
                if (u32_at(block, 0), u32_at(block, 4)) != (periods_before, pieces_before) {
                    return Err(FormatError("aircraft: an index block miscounts"));
                }
            }
            let (head, record) = (parsed.heads[index], parsed.box_record(index));
            let band = usize::from(record[0]);
            if band >= bands || head & 7 == 0 || head >> 6 != 0 {
                return Err(FormatError(
                    "aircraft: a box of no band, no period or a bad head",
                ));
            }
            let edge = cell_steps(parsed.bands[BAND_BYTES * band]);
            if i32::from(record[1].max(record[2])) * edge >= 32_768 {
                return Err(FormatError("aircraft: box cell outside the tile"));
            }
            periods_before += heard_periods(head);
            pieces_before += piece_count(head);
        }
        if periods_before as usize != periods || pieces_before as usize != pieces {
            return Err(FormatError(
                "aircraft: the boxes do not name their periods and pieces",
            ));
        }
        for index in 0..periods {
            if usize::from(u16_at(parsed.period_record(index), 16)) >= mixes {
                return Err(FormatError("aircraft: a period names a missing mix"));
            }
        }
        for index in 0..pieces {
            if u24_at(parsed.piece_record(index), 0) as usize >= flights {
                return Err(FormatError("aircraft: a piece names a missing flight"));
            }
        }
        Ok(parsed)
    }

    pub fn box_count(&self) -> usize {
        self.heads.len()
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

    fn period_record(&self, index: usize) -> &'a [u8] {
        &self.periods[index * PERIOD_BYTES..(index + 1) * PERIOD_BYTES]
    }

    fn piece_record(&self, index: usize) -> &'a [u8] {
        &self.pieces[index * PIECE_BYTES..(index + 1) * PIECE_BYTES]
    }

    /// The first heard period and the first piece of box `index`: its block's counts and the
    /// heads of the boxes before it in the block.
    fn firsts(&self, index: usize) -> (usize, u32) {
        let block = index / BOXES_PER_BLOCK;
        let counts = &self.blocks[BLOCK_BYTES * block..];
        self.heads[block * BOXES_PER_BLOCK..index].iter().fold(
            (u32_at(counts, 0) as usize, u32_at(counts, 4)),
            |(periods, pieces), &head| {
                (
                    periods + heard_periods(head) as usize,
                    pieces + piece_count(head),
                )
            },
        )
    }

    /// The zoom of a box record's band.
    fn zoom(&self, record: &[u8]) -> u8 {
        self.bands[BAND_BYTES * usize::from(record[0])]
    }

    /// A box's emission-weighted centroid alone (tile-local steps): where it is, without the rest
    /// of its record.
    pub fn box_centroid(&self, index: usize) -> [i16; 2] {
        let record = self.box_record(index);
        let edge = cell_steps(self.zoom(record));
        [
            in_cell(record[5], record[1], edge),
            in_cell(record[6], record[2], edge),
        ]
    }

    pub fn aircraft_box(&self, index: usize) -> AircraftBox {
        let (head, record) = (self.heads[index], self.box_record(index));
        let band = &self.bands[BAND_BYTES * usize::from(record[0])..];
        let (mut next_period, first_piece) = self.firsts(index);
        let mut energy_db = [[f64::NEG_INFINITY; NPD_DISTANCES]; PERIODS];
        let mut tail_energy_db = [f64::NEG_INFINITY; PERIODS];
        let mut flights_per_period = [0; PERIODS];
        let mut piece_length_m = [0.0; PERIODS];
        let mut installation_shares = [[[0.0, 0.0, 1.0]; 2]; PERIODS];
        for period in (0..PERIODS).filter(|period| head & 1 << period != 0) {
            let record = self.period_record(next_period);
            next_period += 1;
            let (levels, end) = curve_levels(u16_at(record, 0), &record[2..11]);
            energy_db[period] = levels;
            tail_energy_db[period] = tail_db(end, record[11]);
            flights_per_period[period] = flights(u16_at(record, 12));
            piece_length_m[period] = f64::from(u16_at(record, 14));
            let mix = &self.mixes[MIX_BYTES * usize::from(u16_at(record, 16))..];
            installation_shares[period] = [0, 2].map(|at| {
                let (wing, fuselage) = (f64::from(mix[at]) / 255.0, f64::from(mix[at + 1]) / 255.0);
                let mut shares = [wing, fuselage, 0.0];
                shares[INSTALLATIONS - 1] = (1.0 - wing - fuselage).max(0.0);
                shares
            });
        }
        AircraftBox {
            zoom: band[0],
            cell: [u16::from(record[1]), u16::from(record[2])],
            group: if head & HEAD_HELICOPTER == 0 {
                Group::FixedWing
            } else {
                Group::Helicopter
            },
            ground_m: f64::from(i16_at(record, 3)),
            clearance_m: f64::from(u16_at(band, 1)),
            height_m: f64::from(u16_at(band, 3)),
            centroid: self.box_centroid(index),
            centroid_altitude_m: f64::from(i16_at(record, 7)),
            axis_rad: f64::from(u16_at(record, 9)) / 65_536.0 * std::f64::consts::PI,
            gradient: f64::from(i16_at(record, 11)) / 10_000.0,
            gradient_spread: f64::from(u16_at(record, 13)) / 10_000.0,
            piece_length_m,
            flights: flights_per_period,
            energy_db,
            lg_scaled_distance: lg_distances(u16_at(record, 15), &record[17..26]),
            tail_energy_db,
            installation_shares,
            first_piece,
            piece_count: (head >> HEAD_PIECES_SHIFT) & 3,
            loudest_lamax_db: lamax_db(record[26]),
        }
    }

    pub fn flight(&self, index: usize) -> Flight {
        let record = &self.flights[index * FLIGHT_BYTES..(index + 1) * FLIGHT_BYTES];
        Flight {
            icao: u24_at(record, 0),
            callsign: record[3..11].try_into().unwrap(),
            type_designator: record[11..15].try_into().unwrap(),
            start_unix: u32_at(record, 15),
        }
    }

    /// The `k`th of `owner`'s pieces (`owner` as [`Self::aircraft_box`] read it): its ends are
    /// stored from the box's centroid.
    pub fn piece(&self, owner: &AircraftBox, k: usize) -> FlightPiece {
        assert!(k < usize::from(owner.piece_count), "box piece {k}");
        let record = self.piece_record(owner.first_piece as usize + k);
        let edge = cell_steps(owner.zoom);
        let end =
            |at: usize| [0, 1].map(|axis| offset(record[at + axis], owner.centroid[axis], edge));
        let kind = u16_at(record, 13);
        FlightPiece {
            flight: u24_at(record, 0),
            ends: [end(3), end(5)],
            altitudes_m: [f64::from(i16_at(record, 7)), f64::from(i16_at(record, 9))],
            speed_kt: f64::from(u16_at(record, 11)) / 10.0,
            class: kind & 0x0fff,
            power_code: u16_at(record, 15),
            flags: (kind >> 12 & 3) as u8,
            period: (kind >> 14) as u8,
        }
    }
}

const _: () = assert!(17 + NPD_DISTANCES - 1 + 1 == BOX_BYTES);
const _: () = assert!(2 + NPD_DISTANCES - 1 + 1 + 2 + 2 + 2 == PERIOD_BYTES && PERIODS == 3);

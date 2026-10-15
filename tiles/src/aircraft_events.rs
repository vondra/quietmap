//! `aircraft-events` tiles: what flies over each z16 cell of the tile, every flight of the year
//! counted once at its loudest moment there (Doc 29 Eq. 4-8a, outdoors in the open): per band of
//! that maximum level (50, 60, 70 dB) the flights of an average day and those at night, their
//! mean height above the ground at that moment and the type flying most of them; and the
//! helicopters of the lowest band. A tile without any flight above 50 dB has no file.
//!
//! ```text
//! magic "qmevt2\n\0", u16 cells per side (16), u16 designators
//! per cell, row-major from the north-west, 23 B:
//!   per band: flights a day, of them at night (23-07) as u16 rates, mean height i16 (m, negative
//!   below the ground there), designator index u8 (255 none)
//!   helicopters a day in the lowest band, a u16 rate
//! designators: 4 B each, ICAO, space padded
//! ```
//!
//! A rate is a u16 float (`rate`: within 0.025 %); a file names at most 29 types in r064
//! (`events-tracks-ranges-r064.txt`).

use crate::FormatError;

const MAGIC: &[u8; 8] = b"qmevt2\n\0";
const HEADER_BYTES: usize = 12;
const BAND_BYTES: usize = 7;
const CELL_BYTES: usize = BANDS * BAND_BYTES + 2;
const NO_DESIGNATOR: u8 = u8::MAX;
/// The bands of the table (dB, maximum level at or above).
pub const EVENT_BANDS_DB: [f64; 3] = [50.0, 60.0, 70.0];
pub const BANDS: usize = EVENT_BANDS_DB.len();
/// z16 cells per side of a z12 tile (about 400 m in Europe): read with bilinear counts, as good
/// as z17's nearest cell at the pilot's ten points and a fifth of its work (evidence 2026-10-08,
/// r055 pilot).
pub const CELLS_PER_SIDE: usize = 16;
/// One band of one cell.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BandCell {
    pub per_day: f32,
    pub night_per_day: f32,
    pub height_m: i16,
    pub designator: Option<[u8; 4]>,
}

/// One cell.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EventCell {
    pub bands: [BandCell; BANDS],
    pub helicopters_per_day: f32,
}

/// A rate a day as its u16 float.
fn rate_code(rate: f32) -> u16 {
    crate::rate::rate_code(f64::from(rate))
}

fn rate(code: u16) -> f32 {
    crate::rate::rate(code) as f32
}

/// The bytes of a tile's cells (row-major from the north-west, [`CELLS_PER_SIDE`] squared).
pub fn encode(cells: &[EventCell]) -> Vec<u8> {
    assert_eq!(
        cells.len(),
        CELLS_PER_SIDE * CELLS_PER_SIDE,
        "a tile's cells"
    );
    let mut designators: Vec<[u8; 4]> = cells
        .iter()
        .flat_map(|cell| cell.bands.iter().filter_map(|band| band.designator))
        .collect();
    designators.sort_unstable();
    designators.dedup();
    assert!(
        designators.len() <= usize::from(NO_DESIGNATOR),
        "{} designators in a tile",
        designators.len()
    );
    let mut bytes =
        Vec::with_capacity(HEADER_BYTES + CELL_BYTES * cells.len() + 4 * designators.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&(CELLS_PER_SIDE as u16).to_le_bytes());
    bytes.extend_from_slice(&(designators.len() as u16).to_le_bytes());
    for cell in cells {
        for band in &cell.bands {
            bytes.extend_from_slice(&rate_code(band.per_day).to_le_bytes());
            bytes.extend_from_slice(&rate_code(band.night_per_day).to_le_bytes());
            bytes.extend_from_slice(&band.height_m.to_le_bytes());
            bytes.push(band.designator.map_or(NO_DESIGNATOR, |designator| {
                designators.binary_search(&designator).expect("listed") as u8
            }));
        }
        bytes.extend_from_slice(&rate_code(cell.helicopters_per_day).to_le_bytes());
    }
    for designator in &designators {
        bytes.extend_from_slice(designator);
    }
    bytes
}

/// A parsed events file borrowing its bytes.
#[derive(Clone, Copy)]
pub struct AircraftEvents<'a> {
    cells: &'a [u8],
    designators: &'a [u8],
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

impl<'a> AircraftEvents<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, FormatError> {
        if bytes.len() < HEADER_BYTES || &bytes[..8] != MAGIC {
            return Err(FormatError("aircraft-events: bad magic"));
        }
        if usize::from(u16_at(bytes, 8)) != CELLS_PER_SIDE {
            return Err(FormatError("aircraft-events: cells per side"));
        }
        let designators = usize::from(u16_at(bytes, 10));
        let cells_end = HEADER_BYTES + CELL_BYTES * CELLS_PER_SIDE * CELLS_PER_SIDE;
        if bytes.len() != cells_end + 4 * designators {
            return Err(FormatError("aircraft-events: length does not match"));
        }
        let parsed = AircraftEvents {
            cells: &bytes[HEADER_BYTES..cells_end],
            designators: &bytes[cells_end..],
        };
        for record in parsed.cells.chunks_exact(CELL_BYTES) {
            for band in 0..BANDS {
                let index = record[BAND_BYTES * band + 6];
                if index != NO_DESIGNATOR && usize::from(index) >= designators {
                    return Err(FormatError("aircraft-events: a cell names a missing type"));
                }
            }
        }
        Ok(parsed)
    }

    /// The cell `column` from the west and `row` from the north.
    pub fn cell(&self, column: usize, row: usize) -> EventCell {
        let at = CELL_BYTES * (row * CELLS_PER_SIDE + column);
        let record = &self.cells[at..at + CELL_BYTES];
        EventCell {
            bands: std::array::from_fn(|band| {
                let at = BAND_BYTES * band;
                let index = record[at + 6];
                BandCell {
                    per_day: rate(u16_at(record, at)),
                    night_per_day: rate(u16_at(record, at + 2)),
                    height_m: i16::from_le_bytes([record[at + 4], record[at + 5]]),
                    designator: (index != NO_DESIGNATOR).then(|| {
                        let start = 4 * usize::from(index);
                        self.designators[start..start + 4].try_into().unwrap()
                    }),
                }
            }),
            helicopters_per_day: rate(u16_at(record, BAND_BYTES * BANDS)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cells round trip, their rates within 0.025 % and their types listed once; a cell naming a
    /// type beyond the list, or a short file, is refused.
    #[test]
    fn cells_round_trip_and_bad_files_are_refused() {
        let mut cells = vec![EventCell::default(); CELLS_PER_SIDE * CELLS_PER_SIDE];
        cells[17] = EventCell {
            bands: [
                BandCell {
                    per_day: 412.5,
                    night_per_day: 21.25,
                    height_m: 640,
                    designator: Some(*b"A320"),
                },
                BandCell {
                    per_day: 0.004,
                    night_per_day: 0.0,
                    height_m: -35,
                    designator: Some(*b"EC35"),
                },
                BandCell::default(),
            ],
            helicopters_per_day: 0.00282,
        };
        cells[255].bands[0].designator = Some(*b"A320");
        let bytes = encode(&cells);
        assert_eq!(bytes.len(), HEADER_BYTES + 23 * 256 + 2 * 4);
        let parsed = AircraftEvents::parse(&bytes).unwrap();
        let read = parsed.cell(1, 1);
        let close = |read: f32, written: f32| (read - written).abs() <= written * 2.5e-4;
        for (r, w) in read.bands.iter().zip(&cells[17].bands) {
            assert!(close(r.per_day, w.per_day) && close(r.night_per_day, w.night_per_day));
            assert_eq!((r.height_m, r.designator), (w.height_m, w.designator));
        }
        assert!(close(read.helicopters_per_day, 0.00282));
        assert_eq!(
            parsed.cell(1, 1).bands[0].per_day,
            412.5,
            "exact in 11 bits"
        );
        assert_eq!(parsed.cell(15, 15), cells[255]);
        assert_eq!(parsed.cell(0, 0), EventCell::default());
        assert!(AircraftEvents::parse(&bytes[..bytes.len() - 1]).is_err());
        let mut missing = bytes.clone();
        missing[HEADER_BYTES + 23 * 17 + 6] = 7;
        assert!(AircraftEvents::parse(&missing).is_err());
    }
}

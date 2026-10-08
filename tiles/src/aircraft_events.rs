//! `aircraft-events` tiles: what flies over each z17 cell of the tile, every flight of the year
//! counted once at its loudest moment there (Doc 29 Eq. 4-8a, outdoors in the open): per band of
//! that maximum level (50, 60, 70 dB) the flights of an average day and those at night, their
//! mean height above the ground at that moment and the type flying most of them; and the
//! helicopters of the lowest band. A tile without any flight above 50 dB has no file.
//!
//! ```text
//! magic "qmevt1\n\0", u16 cells per side (32), u16 designators
//! per cell, row-major from the north-west, 40 B:
//!   per band: f32 flights a day, f32 of them at night (23-07), i16 mean height (m, negative
//!   below the ground there), u16 designator index (0xFFFF none)
//!   f32 helicopters a day in the lowest band
//! designators: 4 B each, ICAO, space padded
//! ```

use crate::FormatError;

const MAGIC: &[u8; 8] = b"qmevt1\n\0";
const HEADER_BYTES: usize = 12;
const CELL_BYTES: usize = 40;
const NO_DESIGNATOR: u16 = u16::MAX;
/// The bands of the table (dB, maximum level at or above).
pub const EVENT_BANDS_DB: [f64; 3] = [50.0, 60.0, 70.0];
pub const BANDS: usize = EVENT_BANDS_DB.len();
/// z17 cells per side of a z12 tile.
pub const CELLS_PER_SIDE: usize = 32;

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
        designators.len() < usize::from(NO_DESIGNATOR),
        "designators"
    );
    let mut bytes =
        Vec::with_capacity(HEADER_BYTES + CELL_BYTES * cells.len() + 4 * designators.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&(CELLS_PER_SIDE as u16).to_le_bytes());
    bytes.extend_from_slice(&(designators.len() as u16).to_le_bytes());
    for cell in cells {
        for band in &cell.bands {
            bytes.extend_from_slice(&band.per_day.to_le_bytes());
            bytes.extend_from_slice(&band.night_per_day.to_le_bytes());
            bytes.extend_from_slice(&band.height_m.to_le_bytes());
            let index = band.designator.map_or(NO_DESIGNATOR, |designator| {
                designators.binary_search(&designator).expect("listed") as u16
            });
            bytes.extend_from_slice(&index.to_le_bytes());
        }
        bytes.extend_from_slice(&cell.helicopters_per_day.to_le_bytes());
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

fn f32_at(bytes: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
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
                let index = u16_at(record, 12 * band + 10);
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
                let at = 12 * band;
                let index = u16_at(record, at + 10);
                BandCell {
                    per_day: f32_at(record, at),
                    night_per_day: f32_at(record, at + 4),
                    height_m: i16::from_le_bytes([record[at + 8], record[at + 9]]),
                    designator: (index != NO_DESIGNATOR).then(|| {
                        let start = 4 * usize::from(index);
                        self.designators[start..start + 4].try_into().unwrap()
                    }),
                }
            }),
            helicopters_per_day: f32_at(record, 12 * BANDS),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cells round trip with their types listed once; a cell naming a type beyond the list, or a
    /// short file, is refused.
    #[test]
    fn cells_round_trip_and_bad_files_are_refused() {
        let mut cells = vec![EventCell::default(); CELLS_PER_SIDE * CELLS_PER_SIDE];
        cells[33] = EventCell {
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
            helicopters_per_day: 0.004,
        };
        cells[1023].bands[0].designator = Some(*b"A320");
        let bytes = encode(&cells);
        assert_eq!(bytes.len(), HEADER_BYTES + 40 * 1024 + 2 * 4);
        let parsed = AircraftEvents::parse(&bytes).unwrap();
        assert_eq!(parsed.cell(1, 1), cells[33]);
        assert_eq!(parsed.cell(31, 31), cells[1023]);
        assert_eq!(parsed.cell(0, 0), EventCell::default());
        assert!(AircraftEvents::parse(&bytes[..bytes.len() - 1]).is_err());
        let mut missing = bytes.clone();
        missing[HEADER_BYTES + 40 * 33 + 10] = 7;
        assert!(AircraftEvents::parse(&missing).is_err());
    }
}

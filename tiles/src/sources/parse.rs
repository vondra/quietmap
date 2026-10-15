//! Reading a sources file in place: pieces, attributes and decks decode on demand, a display only
//! for the contributors the panel shows.

use super::display::read_record;
use super::{
    ATTRIBUTE_BYTES, Attribute, BANDS, DECK_BYTES, EMISSION_BYTES, Emission, HEADER_BYTES,
    LEVEL_OFFSET_DB, LINE_BYTES, Layer, MAGIC, POINT_BYTES, Piece,
};
use crate::FormatError;
use crate::terrain::height_m_of_code;

fn level_db(code: u16) -> f64 {
    if code == 0 {
        f64::NEG_INFINITY
    } else {
        f64::from(code) / 100.0 - LEVEL_OFFSET_DB
    }
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

/// A parsed sources file borrowing its bytes.
pub struct Sources<'a> {
    lines: &'a [u8],
    points: &'a [u8],
    attributes: &'a [u8],
    emissions: &'a [u8],
    decks: &'a [u8],
    displays: &'a [u8],
    strings: &'a [u8],
}

impl<'a> Sources<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, FormatError> {
        if bytes.len() < HEADER_BYTES || &bytes[..8] != MAGIC {
            return Err(FormatError("sources: bad magic"));
        }
        let count = |at: usize| u32_at(bytes, at) as usize;
        let lengths = [
            LINE_BYTES.saturating_mul(count(8)),
            POINT_BYTES.saturating_mul(count(12)),
            ATTRIBUTE_BYTES.saturating_mul(count(16)),
            EMISSION_BYTES.saturating_mul(count(20)),
            DECK_BYTES.saturating_mul(count(24)),
            count(28),
            count(32),
        ];
        let mut end = HEADER_BYTES;
        let sections = lengths.map(|length| {
            let start = end;
            end = end.saturating_add(length);
            start..end
        });
        if bytes.len() != end {
            return Err(FormatError("sources: length does not match the counts"));
        }
        let [
            lines,
            points,
            attributes,
            emissions,
            decks,
            displays,
            strings,
        ] = sections.map(|section| &bytes[section]);
        Ok(Sources {
            lines,
            points,
            attributes,
            emissions,
            decks,
            displays,
            strings,
        })
    }

    /// The deck's altitude (m) at the two ends of piece `piece` on a bridge (lines come first, so
    /// a line's piece index is its index among the lines).
    pub fn deck(&self, piece: usize) -> Option<[f64; 2]> {
        let entry = |k: usize| &self.decks[k * DECK_BYTES..(k + 1) * DECK_BYTES];
        let (mut low, mut high) = (0, self.decks.len() / DECK_BYTES);
        while low < high {
            let middle = (low + high) / 2;
            match (u32_at(entry(middle), 0) as usize).cmp(&piece) {
                std::cmp::Ordering::Less => low = middle + 1,
                std::cmp::Ordering::Greater => high = middle,
                std::cmp::Ordering::Equal => {
                    let code =
                        |at: usize| u16::from_le_bytes([entry(middle)[at], entry(middle)[at + 1]]);
                    return Some([height_m_of_code(code(4)), height_m_of_code(code(6))]);
                }
            }
        }
        None
    }

    fn line_count(&self) -> usize {
        self.lines.len() / LINE_BYTES
    }

    /// Lines, then points.
    pub fn piece_count(&self) -> usize {
        self.line_count() + self.points.len() / POINT_BYTES
    }

    pub fn attribute_count(&self) -> usize {
        self.attributes.len() / ATTRIBUTE_BYTES
    }

    /// Piece `index`: the lines first, then the points (a point's two ends are its place).
    pub fn piece(&self, index: usize) -> Result<Piece, FormatError> {
        let i16_at = |record: &[u8], at: usize| i16::from_le_bytes([record[at], record[at + 1]]);
        let (ends, attribute) = if index < self.line_count() {
            let record = &self.lines[index * LINE_BYTES..(index + 1) * LINE_BYTES];
            (
                [
                    [i16_at(record, 0), i16_at(record, 2)],
                    [i16_at(record, 4), i16_at(record, 6)],
                ],
                u32_at(record, 8),
            )
        } else {
            let at = (index - self.line_count()) * POINT_BYTES;
            let record = &self.points[at..at + POINT_BYTES];
            let place = [i16_at(record, 0), i16_at(record, 2)];
            ([place, place], u32_at(record, 4))
        };
        if attribute as usize >= self.attribute_count() {
            return Err(FormatError("sources: attribute index out of range"));
        }
        Ok(Piece { ends, attribute })
    }

    fn attribute_record(&self, index: u32) -> Result<&'a [u8], FormatError> {
        self.attributes
            .get(index as usize * ATTRIBUTE_BYTES..(index as usize + 1) * ATTRIBUTE_BYTES)
            .ok_or(FormatError("sources: attribute index out of range"))
    }

    /// An attribute without its display.
    pub fn attribute(&self, index: u32) -> Result<Attribute, FormatError> {
        let record = self.attribute_record(index)?;
        let u16_at = |at: usize| u16::from_le_bytes([record[at], record[at + 1]]);
        let u64_at = |at: usize| u64::from_le_bytes(record[at..at + 8].try_into().unwrap());
        let row = self.emission_row_of(index)?;
        Ok(Attribute {
            layer: Layer::from_code(record[0])?,
            ground_percent: record[1],
            platform_half_width_m: f64::from(u16_at(2)) / 10.0,
            height_m: f64::from(u16_at(4)) / 100.0,
            exclusion_radius_m: f64::from(u16_at(6)) / 10.0,
            footprint_id: u64_at(8),
            group_key: u64_at(16),
            emission: self.emission_row(row),
            display: Vec::new(),
        })
    }

    /// The distinct emission rows the attributes share.
    pub fn emission_row_count(&self) -> usize {
        self.emissions.len() / EMISSION_BYTES
    }

    /// Emission row `row`: a reader computing from an emission does it once a row.
    pub fn emission_row(&self, row: usize) -> Emission {
        let row = &self.emissions[row * EMISSION_BYTES..(row + 1) * EMISSION_BYTES];
        std::array::from_fn(|period| {
            std::array::from_fn(|band| {
                let at = 2 * (period * BANDS + band);
                level_db(u16::from_le_bytes([row[at], row[at + 1]]))
            })
        })
    }

    /// The emission row of an attribute.
    pub fn emission_row_of(&self, index: u32) -> Result<usize, FormatError> {
        let row = u32_at(self.attribute_record(index)?, 24) as usize;
        if row >= self.emission_row_count() {
            return Err(FormatError("sources: emission row out of range"));
        }
        Ok(row)
    }

    /// An attribute's layer alone, without decoding its emission.
    pub fn layer(&self, index: u32) -> Result<Layer, FormatError> {
        Layer::from_code(self.attribute_record(index)?[0])
    }

    /// An attribute's group key alone, without decoding its emission.
    pub fn group_key(&self, index: u32) -> Result<u64, FormatError> {
        let record = self.attribute_record(index)?;
        Ok(u64::from_le_bytes(record[16..24].try_into().unwrap()))
    }

    /// The display fields of an attribute as the positional JSON array of its layer.
    pub fn display(&self, index: u32) -> Result<Vec<serde_json::Value>, FormatError> {
        let record = self.attribute_record(index)?;
        read_record(
            Layer::from_code(record[0])?,
            self.displays,
            self.strings,
            u32_at(record, 28) as usize,
        )
    }
}

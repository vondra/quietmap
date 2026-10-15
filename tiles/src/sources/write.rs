//! Writing a sources file: the lines, then the points, the attribute records, the distinct
//! emission rows, the decks of the lines on bridges, the display records and their strings.

use super::display::DisplayTable;
use super::{
    ATTRIBUTE_BYTES, Attribute, BANDS, DECK_BYTES, Deck, EMISSION_BYTES, Emission, HEADER_BYTES,
    LEVEL_OFFSET_DB, LINE_BYTES, MAGIC, PERIODS, POINT_BYTES, Piece,
};
use crate::terrain::code_of_height_m;
use std::collections::HashMap;

fn level_code(level_db: f64) -> u16 {
    if level_db == f64::NEG_INFINITY {
        return 0;
    }
    let code = ((level_db + LEVEL_OFFSET_DB) * 100.0).round();
    assert!(
        (1.0..=65_535.0).contains(&code),
        "emission {level_db} dB outside the u16 range"
    );
    code as u16
}

fn emission_row(emission: &Emission) -> [u8; EMISSION_BYTES] {
    let mut row = [0; EMISSION_BYTES];
    for (code, level) in row.chunks_exact_mut(2).zip(emission.iter().flatten()) {
        code.copy_from_slice(&level_code(*level).to_le_bytes());
    }
    row
}

/// A length in steps of `1 / steps_per_m` metres as u16, refused beyond its range.
fn steps_u16(value_m: f64, steps_per_m: f64, what: &str) -> [u8; 2] {
    let code = (value_m * steps_per_m).round();
    assert!(
        (0.0..=f64::from(u16::MAX)).contains(&code),
        "{what} {value_m} m outside the u16 range"
    );
    (code as u16).to_le_bytes()
}

fn write_attribute_record(bytes: &mut Vec<u8>, attribute: &Attribute, row: u32, display: u32) {
    bytes.push(attribute.layer as u8);
    bytes.push(attribute.ground_percent);
    bytes.extend_from_slice(&steps_u16(
        attribute.platform_half_width_m,
        10.0,
        "platform",
    ));
    bytes.extend_from_slice(&steps_u16(attribute.height_m, 100.0, "height"));
    bytes.extend_from_slice(&steps_u16(attribute.exclusion_radius_m, 10.0, "exclusion"));
    bytes.extend_from_slice(&attribute.footprint_id.to_le_bytes());
    bytes.extend_from_slice(&attribute.group_key.to_le_bytes());
    bytes.extend_from_slice(&row.to_le_bytes());
    bytes.extend_from_slice(&display.to_le_bytes());
}

/// Two attributes with equal keys are stored once: the record as written, its emission and its
/// display.
pub fn attribute_key(attribute: &Attribute) -> Vec<u8> {
    let mut key = Vec::with_capacity(ATTRIBUTE_BYTES + EMISSION_BYTES + 64);
    write_attribute_record(&mut key, attribute, 0, 0);
    key.extend_from_slice(&emission_row(&attribute.emission));
    key.extend_from_slice(&serde_json::to_vec(&attribute.display).expect("a display serialises"));
    key
}

/// The bytes of a sources file. The lines keep their order, then the points theirs; a deck names
/// a line (a piece on a bridge has two ends) by its index in `pieces`, in piece order.
pub fn encode(pieces: &[Piece], attributes: &[Attribute], decks: &[Deck]) -> Vec<u8> {
    let (lines, points): (Vec<&Piece>, Vec<&Piece>) =
        pieces.iter().partition(|piece| piece.is_line());
    // The index among the lines of each line piece.
    let mut line_of = vec![u32::MAX; pieces.len()];
    let mut next = 0;
    for (index, piece) in pieces.iter().enumerate() {
        if piece.is_line() {
            line_of[index] = next;
            next += 1;
        }
    }
    let mut displays = DisplayTable::default();
    let display_at: Vec<u32> = attributes
        .iter()
        .map(|attribute| displays.record(attribute.layer, &attribute.display))
        .collect();
    let mut rows: Vec<[u8; EMISSION_BYTES]> = Vec::new();
    let mut row_of: HashMap<[u8; EMISSION_BYTES], u32> = HashMap::new();
    let row_indices: Vec<u32> = attributes
        .iter()
        .map(|attribute| {
            let row = emission_row(&attribute.emission);
            *row_of.entry(row).or_insert_with(|| {
                rows.push(row);
                u32::try_from(rows.len() - 1).expect("tile too large")
            })
        })
        .collect();
    let mut bytes = Vec::with_capacity(
        HEADER_BYTES
            + LINE_BYTES * lines.len()
            + POINT_BYTES * points.len()
            + ATTRIBUTE_BYTES * attributes.len()
            + EMISSION_BYTES * rows.len()
            + DECK_BYTES * decks.len()
            + displays.records.len()
            + displays.strings.len(),
    );
    bytes.extend_from_slice(MAGIC);
    for count in [
        lines.len(),
        points.len(),
        attributes.len(),
        rows.len(),
        decks.len(),
        displays.records.len(),
        displays.strings.len(),
    ] {
        bytes.extend_from_slice(&u32::try_from(count).expect("tile too large").to_le_bytes());
    }
    for piece in &lines {
        assert!((piece.attribute as usize) < attributes.len());
        for coordinate in piece.ends.iter().flatten() {
            bytes.extend_from_slice(&coordinate.to_le_bytes());
        }
        bytes.extend_from_slice(&piece.attribute.to_le_bytes());
    }
    for piece in &points {
        assert!((piece.attribute as usize) < attributes.len());
        for coordinate in piece.ends[0] {
            bytes.extend_from_slice(&coordinate.to_le_bytes());
        }
        bytes.extend_from_slice(&piece.attribute.to_le_bytes());
    }
    for ((attribute, &row), &display) in attributes.iter().zip(&row_indices).zip(&display_at) {
        write_attribute_record(&mut bytes, attribute, row, display);
    }
    for row in &rows {
        bytes.extend_from_slice(row);
    }
    for (k, deck) in decks.iter().enumerate() {
        let line = line_of[deck.piece as usize];
        assert!(line != u32::MAX, "a deck under a point");
        assert!(
            k == 0 || decks[k - 1].piece < deck.piece,
            "decks in piece order"
        );
        bytes.extend_from_slice(&line.to_le_bytes());
        for altitude_m in deck.altitude_m {
            bytes.extend_from_slice(&code_of_height_m(altitude_m).to_le_bytes());
        }
    }
    bytes.extend_from_slice(&displays.records);
    bytes.extend_from_slice(&displays.strings);
    bytes
}

const _: () = assert!(EMISSION_BYTES == 2 * BANDS * PERIODS && LINE_BYTES == 12);

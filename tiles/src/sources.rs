//! `sources` tiles: every ground source owned by the tile (each piece lies inside it) as a point or
//! a straight line piece. Pieces sharing emission and display share one attribute record: its
//! layer, the geometry of emission (height, ground, platform), emission per octave band and period
//! computed at build time, and the display fields the popup shows for its contributors. Records
//! sharing their display text point at one copy of it: the rows of a road differ in emission by
//! their gradient and junction corrections, not in what the popup shows of them. Records sharing
//! their emission point at one row of it: a city's buildings, each its own record by its footprint
//! and group, share a few spectra. A piece on a bridge has an entry in the deck table: the deck's
//! altitude at its two ends, which its source stands on instead of the terrain under it.
//!
//! ```text
//! magic "qmsrc4\n\0", u32 pieces, attributes, emission rows, decks, text bytes
//! per piece, 12 B: ends 4 x i16, attribute u32
//! per attribute, 36 B: layer u8, ground u8, platform half-width u16 (0.1 m), height u16 (0.01 m),
//!   exclusion radius u16 (0.1 m), footprint u64, group u64, emission row u32, text start u32,
//!   text length u32
//! per emission row, 48 B: per period and octave band a level code u16
//! per deck, in piece order, 8 B: piece u32, the deck's altitude at its ends 2 x u16 (terrain codes)
//! display texts, each distinct text once
//! ```

use crate::FormatError;
use crate::terrain::{code_of_height_m, height_m_of_code};
use std::collections::HashMap;

const MAGIC: &[u8; 8] = b"qmsrc4\n\0";
const HEADER_BYTES: usize = 28;
const PIECE_BYTES: usize = 12;
const ATTRIBUTE_BYTES: usize = 36;
/// An emission row: a level code per period and octave band.
const EMISSION_BYTES: usize = 2 * BANDS * PERIODS;
/// A deck entry: the piece's index (u32) and the deck's altitude at its two ends (terrain codes).
const DECK_BYTES: usize = 8;
/// Octave bands 63 Hz .. 8 kHz.
pub const BANDS: usize = 8;
/// Day, evening, night.
pub const PERIODS: usize = 3;
/// Stored level = code / 100 - 100 dB; code 0 is silence.
const LEVEL_OFFSET_DB: f64 = 100.0;
/// `ground_percent` meaning "the ground under the source, read from the terrain".
pub const GROUND_FROM_TERRAIN: u8 = u8::MAX;

/// The layer a source is reported in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Layer {
    Road = 0,
    Railway = 1,
    Industry = 2,
    Building = 3,
    Ship = 4,
    Aircraft = 5,
}

impl Layer {
    pub const ALL: [Layer; 6] = [
        Layer::Road,
        Layer::Railway,
        Layer::Industry,
        Layer::Building,
        Layer::Ship,
        Layer::Aircraft,
    ];

    pub fn name(self) -> &'static str {
        [
            "road",
            "railway",
            "industrial",
            "building",
            "ship",
            "aircraft",
        ][self as usize]
    }

    fn from_code(code: u8) -> Result<Self, FormatError> {
        Layer::ALL
            .get(usize::from(code))
            .copied()
            .ok_or(FormatError("sources: unknown layer"))
    }
}

/// The display fields of a layer's records, in the order of their positional JSON arrays.
pub fn display_fields(layer: Layer) -> &'static [&'static str] {
    match layer {
        Layer::Road => &[
            "name",
            "ref",
            "road_class",
            "aadt_light",
            "aadt_medium",
            "aadt_heavy",
            "aadt_moto",
            "traffic_estimated",
            "cross_section_aadt",
            "speed_posted_kmh",
            "speed_kmh",
            "speed_source",
            "surface",
            "surface_corr_db",
            "lanes",
            "oneway",
            "bridge",
            "source_id",
            "period_shares",
        ],
        Layer::Railway => &[
            "name",
            "ref",
            "rail_type",
            "usage",
            "trains_passenger_day",
            "trains_passenger_evening",
            "trains_passenger_night",
            "trains_freight_day",
            "trains_freight_evening",
            "trains_freight_night",
            "passenger_status",
            "freight_status",
            "speed_kmh",
            "speed_source",
            "bridge",
            "passenger_source_id",
            "freight_source_id",
        ],
        Layer::Industry => &[
            "name",
            "source_type",
            "area_m2",
            "nace",
            "grid_points",
            "hub_height_m",
            "rated_power_kw",
            "sound_power_dba",
            "source_id",
        ],
        Layer::Building => &[
            "name",
            "building_type",
            "height_m",
            "floors",
            "area_m2",
            "address",
            "sound_power_dba",
            "movements_per_day",
            "events_per_day",
            "duty",
            "object",
        ],
        Layer::Ship => &[
            "source_type",
            "area_m2",
            "hours_per_month",
            "sound_power_dba",
            "source_id",
        ],
        Layer::Aircraft => &[
            "name",
            "subtype",
            "airport",
            "arrivals_per_day",
            "departures_per_day",
            "ground_vehicles_per_day",
        ],
    }
}

// Where a road's traffic and a railway's trains come from, as the display fields' source ids say:
// the builder sets them, the data layers read them.

/// dev4's source id of a class prior (no dataset).
pub const PRIOR_SOURCE_ID: u16 = 0;
/// dev4's service-tree heuristic of local streets (a background plus routed trips).
pub const SERVICE_TREE_SOURCE_ID: u16 = 11;
/// The source id this converter gives a row whose traffic the buildings model.
pub const BUILDING_TRAFFIC_SOURCE_ID: u16 = 30;
/// The source id of a Thai national highway's row carrying the department's counts.
pub const THAI_HIGHWAYS_SOURCE_ID: u16 = 31;
/// dev4's sources whose category split is a guess, not a count: the class priors (0), the
/// country-tuned CNOSSOS class defaults (Algeria, DR Congo, Ethiopia, Iran, Iraq, Kazakhstan,
/// Kenya, Morocco, Nigeria, Russia, Sudan, Turkey, Ukraine, Egypt, Tanzania, Uzbekistan: "no open
/// per-segment AADT") and the road-classification fallbacks (Japan, Argentina, Chile, Colombia,
/// Indonesia, Peru, Riyadh, Thailand). They put 9-15 % medium and heavy vehicles on urban main
/// roads and up to 40 % on every class, residential streets included.
pub const GUESSED_SPLIT_SOURCES: [u16; 25] = [
    PRIOR_SOURCE_ID,
    9012,
    9180,
    9231,
    9364,
    9368,
    9398,
    9404,
    9504,
    9566,
    9643,
    9729,
    9792,
    9804,
    9818,
    9834,
    9860,
    9865,
    9870,
    9871,
    9872,
    9873,
    9874,
    9875,
    9876,
];
/// dev4's rail sources whose train counts are a guess: the per-line priors (0) and the
/// operator-class CNOSSOS defaults of countries without open timetables.
pub const GUESSED_TRAIN_SOURCES: [u16; 19] = [
    0, 2044, 9013, 9181, 9232, 9263, 9365, 9369, 9399, 9405, 9505, 9567, 9644, 9730, 9793, 9805,
    9819, 9835, 9861,
];

/// dev4's road datasets that are its own values per road class on a national network (its registry
/// calls them proxies; the builder keeps them as data): Bolivia, China, Ecuador, India, the
/// Philippines, Paraguay, Venezuela.
pub const NETWORK_ESTIMATE_SOURCE_IDS: [u16; 7] = [1013, 1025, 1034, 1050, 1095, 1098, 1124];
/// Amsterdam's file of the EU city traffic volumes: the city's traffic model, not counts (dev4).
pub const AMSTERDAM_MODEL_SOURCE_ID: u16 = 1103;
/// dev4's rail sources whose counts come from a fixed table though the builder keeps them: China's
/// and India's networks by service type, Czechia's residual of 2 passenger and 1 freight train a day
/// where no timetable runs.
pub const TABLE_TRAIN_SOURCES: [u16; 3] = [2021, 2037, 9863];

/// Band levels in dB (Z-weighted) per period; `f64::NEG_INFINITY` is silence. Lines carry sound
/// power per metre, points sound power.
pub type Emission = [[f64; BANDS]; PERIODS];

/// One piece: a straight line from `ends[0]` to `ends[1]`, or a point where both are equal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Piece {
    pub ends: [[i16; 2]; 2],
    pub attribute: u32,
}

impl Piece {
    pub fn is_line(&self) -> bool {
        self.ends[0] != self.ends[1]
    }
}

/// The deck under a piece on a bridge: its altitude (m) at the piece's two ends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Deck {
    pub piece: u32,
    pub altitude_m: [f64; 2],
}

/// What the pieces of one source share.
#[derive(Clone, Debug, PartialEq)]
pub struct Attribute {
    pub layer: Layer,
    /// Source height above the terrain, metres (0.01 m steps).
    pub height_m: f64,
    /// CNOSSOS G under the source in percent, or [`GROUND_FROM_TERRAIN`].
    pub ground_percent: u8,
    /// Half-width within which the terrain may not rise above the source ground (0.1 m steps).
    pub platform_half_width_m: f64,
    /// Radius of the area an area-source cell stands for (0.1 m steps).
    pub exclusion_radius_m: f64,
    /// The building whose footprint this source is: it never screens its own emission. 0: none.
    pub footprint_id: u64,
    /// Groups pieces into one contributor across tiles and builds.
    pub group_key: u64,
    pub emission: Emission,
    /// The layer's display fields as a positional JSON array (see [`display_fields`]); empty when
    /// parsed (the popup reads it with [`Sources::display`] for the contributors it shows).
    pub display: String,
}

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

fn level_db(code: u16) -> f64 {
    if code == 0 {
        f64::NEG_INFINITY
    } else {
        f64::from(code) / 100.0 - LEVEL_OFFSET_DB
    }
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

fn write_attribute_record(bytes: &mut Vec<u8>, attribute: &Attribute, row: u32, text_start: u32) {
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
    bytes.extend_from_slice(&text_start.to_le_bytes());
    bytes.extend_from_slice(&(attribute.display.len() as u32).to_le_bytes());
}

/// Two attributes with equal keys are stored once: the record as written, its emission and its
/// display text.
pub fn attribute_key(attribute: &Attribute) -> Vec<u8> {
    let mut key = Vec::with_capacity(ATTRIBUTE_BYTES + EMISSION_BYTES + attribute.display.len());
    write_attribute_record(&mut key, attribute, 0, 0);
    key.extend_from_slice(&emission_row(&attribute.emission));
    key.extend_from_slice(attribute.display.as_bytes());
    key
}

/// The bytes of a sources file: header, pieces, attributes, emission rows and display texts (each
/// distinct row and text once, in first use's order), the deck entries in piece order.
pub fn encode(pieces: &[Piece], attributes: &[Attribute], decks: &[Deck]) -> Vec<u8> {
    let mut text: Vec<u8> = Vec::new();
    let mut starts: HashMap<&str, u32> = HashMap::new();
    let text_starts: Vec<u32> = attributes
        .iter()
        .map(|attribute| {
            *starts.entry(attribute.display.as_str()).or_insert_with(|| {
                let start = u32::try_from(text.len()).expect("tile too large");
                text.extend_from_slice(attribute.display.as_bytes());
                start
            })
        })
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
            + PIECE_BYTES * pieces.len()
            + ATTRIBUTE_BYTES * attributes.len()
            + EMISSION_BYTES * rows.len()
            + DECK_BYTES * decks.len()
            + text.len(),
    );
    bytes.extend_from_slice(MAGIC);
    for count in [
        pieces.len(),
        attributes.len(),
        rows.len(),
        decks.len(),
        text.len(),
    ] {
        bytes.extend_from_slice(&u32::try_from(count).expect("tile too large").to_le_bytes());
    }
    for piece in pieces {
        assert!((piece.attribute as usize) < attributes.len());
        for coordinate in piece.ends.iter().flatten() {
            bytes.extend_from_slice(&coordinate.to_le_bytes());
        }
        bytes.extend_from_slice(&piece.attribute.to_le_bytes());
    }
    for ((attribute, &row), &start) in attributes.iter().zip(&row_indices).zip(&text_starts) {
        write_attribute_record(&mut bytes, attribute, row, start);
    }
    for row in &rows {
        bytes.extend_from_slice(row);
    }
    for (k, deck) in decks.iter().enumerate() {
        assert!((deck.piece as usize) < pieces.len());
        assert!(
            k == 0 || decks[k - 1].piece < deck.piece,
            "decks in piece order"
        );
        bytes.extend_from_slice(&deck.piece.to_le_bytes());
        for altitude_m in deck.altitude_m {
            bytes.extend_from_slice(&code_of_height_m(altitude_m).to_le_bytes());
        }
    }
    bytes.extend_from_slice(&text);
    bytes
}

/// A parsed sources file borrowing its bytes.
pub struct Sources<'a> {
    pieces: &'a [u8],
    attributes: &'a [u8],
    emissions: &'a [u8],
    decks: &'a [u8],
    text: &'a [u8],
}

impl<'a> Sources<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, FormatError> {
        if bytes.len() < HEADER_BYTES || &bytes[..8] != MAGIC {
            return Err(FormatError("sources: bad magic"));
        }
        let count = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        let (pieces, attributes, rows, decks, text) =
            (count(8), count(12), count(16), count(20), count(24));
        let pieces_end = HEADER_BYTES + PIECE_BYTES * pieces;
        let attributes_end = pieces_end + ATTRIBUTE_BYTES * attributes;
        let emissions_end = attributes_end + EMISSION_BYTES * rows;
        let decks_end = emissions_end + DECK_BYTES * decks;
        if bytes.len() != decks_end + text {
            return Err(FormatError("sources: length does not match the counts"));
        }
        Ok(Sources {
            pieces: &bytes[HEADER_BYTES..pieces_end],
            attributes: &bytes[pieces_end..attributes_end],
            emissions: &bytes[attributes_end..emissions_end],
            decks: &bytes[emissions_end..decks_end],
            text: &bytes[decks_end..],
        })
    }

    /// The deck's altitude (m) at the two ends of a piece on a bridge.
    pub fn deck(&self, piece: usize) -> Option<[f64; 2]> {
        let entry = |k: usize| &self.decks[k * DECK_BYTES..(k + 1) * DECK_BYTES];
        let piece_of = |k: usize| u32::from_le_bytes(entry(k)[..4].try_into().unwrap()) as usize;
        let (mut low, mut high) = (0, self.decks.len() / DECK_BYTES);
        while low < high {
            let middle = (low + high) / 2;
            match piece_of(middle).cmp(&piece) {
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

    pub fn piece_count(&self) -> usize {
        self.pieces.len() / PIECE_BYTES
    }

    pub fn attribute_count(&self) -> usize {
        self.attributes.len() / ATTRIBUTE_BYTES
    }

    pub fn piece(&self, index: usize) -> Result<Piece, FormatError> {
        let record = &self.pieces[index * PIECE_BYTES..(index + 1) * PIECE_BYTES];
        let i16_at = |at: usize| i16::from_le_bytes([record[at], record[at + 1]]);
        let attribute = u32::from_le_bytes(record[8..12].try_into().unwrap());
        if attribute as usize >= self.attribute_count() {
            return Err(FormatError("sources: attribute index out of range"));
        }
        Ok(Piece {
            ends: [[i16_at(0), i16_at(2)], [i16_at(4), i16_at(6)]],
            attribute,
        })
    }

    fn attribute_record(&self, index: u32) -> Result<&'a [u8], FormatError> {
        self.attributes
            .get(index as usize * ATTRIBUTE_BYTES..(index as usize + 1) * ATTRIBUTE_BYTES)
            .ok_or(FormatError("sources: attribute index out of range"))
    }

    /// An attribute without its display text.
    pub fn attribute(&self, index: u32) -> Result<Attribute, FormatError> {
        let record = self.attribute_record(index)?;
        let u16_at = |at: usize| u16::from_le_bytes([record[at], record[at + 1]]);
        let u64_at = |at: usize| u64::from_le_bytes(record[at..at + 8].try_into().unwrap());
        let row = u32::from_le_bytes(record[24..28].try_into().unwrap()) as usize;
        let row = self
            .emissions
            .get(row * EMISSION_BYTES..(row + 1) * EMISSION_BYTES)
            .ok_or(FormatError("sources: emission row out of range"))?;
        Ok(Attribute {
            layer: Layer::from_code(record[0])?,
            ground_percent: record[1],
            platform_half_width_m: f64::from(u16_at(2)) / 10.0,
            height_m: f64::from(u16_at(4)) / 100.0,
            exclusion_radius_m: f64::from(u16_at(6)) / 10.0,
            footprint_id: u64_at(8),
            group_key: u64_at(16),
            emission: std::array::from_fn(|period| {
                std::array::from_fn(|band| {
                    let at = 2 * (period * BANDS + band);
                    level_db(u16::from_le_bytes([row[at], row[at + 1]]))
                })
            }),
            display: String::new(),
        })
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

    /// The display fields of an attribute as a JSON array text.
    pub fn display(&self, index: u32) -> Result<&'a str, FormatError> {
        let record = self.attribute_record(index)?;
        let start = u32::from_le_bytes(record[28..32].try_into().unwrap()) as usize;
        let length = u32::from_le_bytes(record[32..36].try_into().unwrap()) as usize;
        let text = self
            .text
            .get(start..start + length)
            .ok_or(FormatError("sources: display out of range"))?;
        std::str::from_utf8(text).map_err(|_| FormatError("sources: display is not UTF-8"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn road() -> Attribute {
        let mut emission = [[70.25; BANDS]; PERIODS];
        emission[2][7] = f64::NEG_INFINITY;
        Attribute {
            layer: Layer::Road,
            height_m: 0.05,
            ground_percent: 0,
            platform_half_width_m: 5.0,
            exclusion_radius_m: 0.0,
            footprint_id: 0,
            group_key: 7,
            emission,
            display: r#"["Legerova","","secondary"]"#.into(),
        }
    }

    #[test]
    fn sources_round_trip() {
        let building = Attribute {
            layer: Layer::Building,
            height_m: 7.5,
            ground_percent: GROUND_FROM_TERRAIN,
            exclusion_radius_m: 8.9,
            footprint_id: 0x0123_4567_89ab_cdef,
            display: "[]".into(),
            ..road()
        };
        let pieces = [
            Piece {
                ends: [[-100, 200], [16_000, -16_384]],
                attribute: 0,
            },
            Piece {
                ends: [[5, 5], [5, 5]],
                attribute: 1,
            },
            Piece {
                ends: [[16_000, -16_384], [16_383, 0]],
                attribute: 0,
            },
        ];
        let attributes = [road(), building.clone()];
        let decks = [Deck {
            piece: 2,
            altitude_m: [312.4, -12.2],
        }];
        let bytes = encode(&pieces, &attributes, &decks);
        let parsed = Sources::parse(&bytes).unwrap();
        assert_eq!(parsed.piece_count(), 3);
        let deck = parsed.deck(2).unwrap();
        assert!(
            (deck[0] - 312.4).abs() < 1e-9 && (deck[1] + 12.2).abs() < 1e-9,
            "0.2 m steps"
        );
        assert_eq!((parsed.deck(0), parsed.deck(1)), (None, None));
        for (index, piece) in pieces.iter().enumerate() {
            assert_eq!(parsed.piece(index).unwrap(), *piece);
        }
        assert!(!parsed.piece(1).unwrap().is_line());
        assert_eq!(
            parsed.attribute(0).unwrap(),
            Attribute {
                display: String::new(),
                ..road()
            }
        );
        assert_eq!(
            parsed.attribute(1).unwrap(),
            Attribute {
                display: String::new(),
                ..building
            }
        );
        assert_eq!(parsed.group_key(0).unwrap(), 7);
        assert_eq!(parsed.display(0).unwrap(), road().display);
        assert_eq!(parsed.display(1).unwrap(), "[]");
        assert!(Sources::parse(&bytes[..bytes.len() - 1]).is_err());
    }

    /// Two rows of a road that differ in emission keep two records and one copy of their text.
    #[test]
    fn a_shared_display_text_is_stored_once() {
        let mut uphill = road();
        uphill.emission[0][0] += 1.5;
        let pieces = [0, 1].map(|attribute| Piece {
            ends: [[0, 0], [10, 10]],
            attribute,
        });
        let bytes = encode(&pieces, &[road(), uphill.clone()], &[]);
        let parsed = Sources::parse(&bytes).unwrap();
        assert_eq!(parsed.attribute_count(), 2);
        assert_eq!(parsed.attribute(1).unwrap().emission, uphill.emission);
        for index in [0, 1] {
            assert_eq!(parsed.display(index).unwrap(), road().display);
        }
        let single = encode(&pieces[..1], &[road()], &[]);
        assert_eq!(
            bytes.len(),
            single.len() + PIECE_BYTES + ATTRIBUTE_BYTES + EMISSION_BYTES
        );
    }

    /// Buildings of one spectrum keep a record each and one copy of their emission; a platform
    /// wider than a byte's 25.5 m keeps its width.
    #[test]
    fn a_shared_emission_is_stored_once() {
        let building = |footprint_id| Attribute {
            layer: Layer::Building,
            footprint_id,
            group_key: footprint_id,
            display: "[]".into(),
            ..road()
        };
        let pieces = [0, 1, 2].map(|attribute| Piece {
            ends: [[3, 3], [3, 3]],
            attribute,
        });
        let wide = Attribute {
            platform_half_width_m: 36.5,
            ..road()
        };
        let bytes = encode(&pieces, &[building(1), building(2), wide.clone()], &[]);
        let parsed = Sources::parse(&bytes).unwrap();
        assert_eq!(parsed.attribute(1).unwrap().footprint_id, 2);
        assert_eq!(parsed.attribute(1).unwrap().emission, road().emission);
        assert_eq!(parsed.attribute(2).unwrap().platform_half_width_m, 36.5);
        let one = encode(&pieces[..1], &[building(1)], &[]);
        assert_eq!(
            bytes.len(),
            one.len()
                + 2 * (PIECE_BYTES + ATTRIBUTE_BYTES)
                + r#"["Legerova","","secondary"]"#.len()
        );
    }
}

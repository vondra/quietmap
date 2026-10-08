//! `sources` tiles: every ground source owned by the tile (each piece lies inside it) as a point or
//! a straight line piece. Pieces sharing emission and display share one attribute record: its
//! layer, the geometry of emission (height, ground, platform), emission per octave band and period
//! computed at build time, and the display fields the popup shows for its contributors. Records
//! sharing their display text point at one copy of it: the rows of a road differ in emission by
//! their gradient and junction corrections, not in what the popup shows of them.

use crate::FormatError;
use std::collections::HashMap;

const MAGIC: &[u8; 8] = b"qmsrc2\n\0";
const HEADER_BYTES: usize = 24;
const PIECE_BYTES: usize = 12;
const ATTRIBUTE_BYTES: usize = 80;
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

fn write_attribute_record(bytes: &mut Vec<u8>, attribute: &Attribute, text_start: u32) {
    bytes.push(attribute.layer as u8);
    bytes.push(attribute.ground_percent);
    bytes.push((attribute.platform_half_width_m * 10.0).round() as u8);
    bytes.push(0);
    bytes.extend_from_slice(&((attribute.height_m * 100.0).round() as u16).to_le_bytes());
    bytes.extend_from_slice(&((attribute.exclusion_radius_m * 10.0).round() as u16).to_le_bytes());
    bytes.extend_from_slice(&attribute.footprint_id.to_le_bytes());
    bytes.extend_from_slice(&attribute.group_key.to_le_bytes());
    for level in attribute.emission.iter().flatten() {
        bytes.extend_from_slice(&level_code(*level).to_le_bytes());
    }
    bytes.extend_from_slice(&text_start.to_le_bytes());
    bytes.extend_from_slice(&(attribute.display.len() as u32).to_le_bytes());
}

/// Two attributes with equal keys are stored once: the record as written plus the display text.
pub fn attribute_key(attribute: &Attribute) -> Vec<u8> {
    let mut key = Vec::with_capacity(ATTRIBUTE_BYTES + attribute.display.len());
    write_attribute_record(&mut key, attribute, 0);
    key.extend_from_slice(attribute.display.as_bytes());
    key
}

/// The bytes of a sources file: header, 12-byte pieces, 80-byte attributes, display texts (each
/// distinct text once).
pub fn encode(pieces: &[Piece], attributes: &[Attribute]) -> Vec<u8> {
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
    let mut bytes = Vec::with_capacity(
        HEADER_BYTES + PIECE_BYTES * pieces.len() + ATTRIBUTE_BYTES * attributes.len() + text.len(),
    );
    bytes.extend_from_slice(MAGIC);
    for count in [pieces.len(), attributes.len(), text.len()] {
        bytes.extend_from_slice(&u32::try_from(count).expect("tile too large").to_le_bytes());
    }
    bytes.extend_from_slice(&[0; 4]);
    for piece in pieces {
        assert!((piece.attribute as usize) < attributes.len());
        for coordinate in piece.ends.iter().flatten() {
            bytes.extend_from_slice(&coordinate.to_le_bytes());
        }
        bytes.extend_from_slice(&piece.attribute.to_le_bytes());
    }
    for (attribute, &start) in attributes.iter().zip(&text_starts) {
        write_attribute_record(&mut bytes, attribute, start);
    }
    bytes.extend_from_slice(&text);
    bytes
}

/// A parsed sources file borrowing its bytes.
pub struct Sources<'a> {
    pieces: &'a [u8],
    attributes: &'a [u8],
    text: &'a [u8],
}

impl<'a> Sources<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, FormatError> {
        if bytes.len() < HEADER_BYTES || &bytes[..8] != MAGIC {
            return Err(FormatError("sources: bad magic"));
        }
        let count = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        let (pieces, attributes, text) = (count(8), count(12), count(16));
        let pieces_end = HEADER_BYTES + PIECE_BYTES * pieces;
        let attributes_end = pieces_end + ATTRIBUTE_BYTES * attributes;
        if bytes.len() != attributes_end + text {
            return Err(FormatError("sources: length does not match the counts"));
        }
        Ok(Sources {
            pieces: &bytes[HEADER_BYTES..pieces_end],
            attributes: &bytes[pieces_end..attributes_end],
            text: &bytes[attributes_end..],
        })
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
        Ok(Attribute {
            layer: Layer::from_code(record[0])?,
            ground_percent: record[1],
            platform_half_width_m: f64::from(record[2]) / 10.0,
            height_m: f64::from(u16_at(4)) / 100.0,
            exclusion_radius_m: f64::from(u16_at(6)) / 10.0,
            footprint_id: u64_at(8),
            group_key: u64_at(16),
            emission: std::array::from_fn(|period| {
                std::array::from_fn(|band| level_db(u16_at(24 + 2 * (period * BANDS + band))))
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
        let start = u32::from_le_bytes(record[72..76].try_into().unwrap()) as usize;
        let length = u32::from_le_bytes(record[76..80].try_into().unwrap()) as usize;
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
        let bytes = encode(&pieces, &attributes);
        let parsed = Sources::parse(&bytes).unwrap();
        assert_eq!(parsed.piece_count(), 3);
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
        let bytes = encode(&pieces, &[road(), uphill.clone()]);
        let parsed = Sources::parse(&bytes).unwrap();
        assert_eq!(parsed.attribute_count(), 2);
        assert_eq!(parsed.attribute(1).unwrap().emission, uphill.emission);
        for index in [0, 1] {
            assert_eq!(parsed.display(index).unwrap(), road().display);
        }
        let single = encode(&pieces[..1], &[road()]);
        assert_eq!(bytes.len(), single.len() + PIECE_BYTES + ATTRIBUTE_BYTES);
    }
}

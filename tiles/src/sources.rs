//! `sources` tiles: every ground source owned by the tile (each piece lies inside it) as a point or
//! a straight line piece. Pieces sharing emission and display share one attribute record: its
//! layer, the geometry of emission (height, ground, platform), emission per octave band and period
//! computed at build time, and what the click panel shows of it ([`display`]: typed fields, one
//! schema per layer). Records sharing their display point at one copy of it: the rows of a road
//! differ in emission by their gradient and junction corrections, not in what the panel shows of
//! them. Records sharing their emission point at one row of it: a city's buildings, each its own
//! record by its footprint and group, share a few spectra. A line on a bridge has an entry in the
//! deck table: the deck's altitude at its two ends, which its source stands on instead of the
//! terrain under it.
//!
//! ```text
//! magic "qmsrc5\n\0", u32 lines, points, attributes, emission rows, decks, display bytes, string
//!   bytes
//! per line, 12 B: ends 4 x i16, attribute u32
//! per point, 8 B: x, y i16, attribute u32
//! per attribute, 32 B: layer u8, ground u8, platform half-width u16 (0.1 m), height u16 (0.01 m),
//!   exclusion radius u16 (0.1 m), footprint u64, group u64, emission row u32, display u32 (its
//!   record's offset)
//! per emission row, 48 B: per period and octave band a level code u16
//! per deck, in line order, 8 B: line u32, the deck's altitude at its ends 2 x u16 (terrain codes)
//! display records, each distinct one once: a u8 field count, then the layer's first fields
//! strings, each distinct text once: u16 length, UTF-8
//! ```

mod display;
mod parse;
mod write;

pub use display::{Field, FieldKind, display_fields};
pub use parse::Sources;
pub use write::{attribute_key, encode};

use crate::FormatError;

#[cfg(test)]
mod tests;

const MAGIC: &[u8; 8] = b"qmsrc5\n\0";
const HEADER_BYTES: usize = 36;
const LINE_BYTES: usize = 12;
const POINT_BYTES: usize = 8;
const ATTRIBUTE_BYTES: usize = 32;
/// An emission row: a level code per period and octave band.
const EMISSION_BYTES: usize = 2 * BANDS * PERIODS;
/// A deck entry: the line's index (u32) and the deck's altitude at its two ends (terrain codes).
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
/// Indonesia, Peru, Saudi Arabia (Riyadh's map and the national atlas), Thailand). They put
/// 9-15 % medium and heavy vehicles on urban main
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
    /// The layer's display fields as a positional JSON array (see [`display_fields`], each value
    /// to its field's step); empty when parsed (the popup reads it with [`Sources::display`] for
    /// the contributors it shows).
    pub display: Vec<serde_json::Value>,
}

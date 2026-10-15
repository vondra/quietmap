//! `aircraft` tiles: the tile's aircraft boxes and its flight table. A box sums every flight piece
//! that crossed one web-map cell of the tile within one clearance slab, for one aircraft group: per
//! period the NPD energy at the ten NPD distances, the pieces' scaled distance there, the
//! installation shares, and the geometry of its "average aircraft" (the emission-weighted centroid,
//! one mean axis and gradient, the mean piece length). Each box also names its loudest pieces for
//! the top-flights list; a piece names a flight of the table.
//!
//! Each number takes the fewest bytes its meaning needs (`codes`): a box stores only the periods
//! it is heard in, its slab as a row of the tile's band table, its centroid as its place in its
//! cell, an NPD curve as its first level and nine drops, its installation mix as a row of the
//! tile's table of them, its pieces from its centroid. A box finds its periods and pieces by
//! counting: every 32 boxes the counts before them, then the boxes' one-byte heads up to it.
//!
//! ```text
//! magic "qmair5\n\0", u32 boxes, heard periods, flights, pieces, bands, installation mixes
//! per band, 5 B: zoom u8, clearance u16 (m), height u16 (m)
//! per installation mix, 4 B: wing and fuselage shares at 1,000 ft and at the tail anchor (1/255)
//! per 32 boxes, 8 B: the heard periods and the pieces of the boxes before them, u32 each
//! per box, 1 B (its head): bit p the period p heard, bit 3 a helicopter, bits 4-5 its pieces
//! per box, 27 B: band u8, cell 2 x u8, ground i16 (m), centroid in its cell 2 x u8 (1/256), its
//!   altitude i16 (m), axis u16, gradient i16, spread u16, lg scaled distance u16 (1e-4) and nine
//!   changes i8 (0.004), loudest LAmax u8 (0.5 dB)
//! per heard period, in box order, 18 B: energy u16 (0.01 dB) and nine drops u8 (0.05 dB), the
//!   tail anchor's drop below the last u8 (0.1 dB), flights u16 (exact below 4,096, then a 4-bit
//!   exponent and 12-bit mantissa), piece length u16 (m), installation mix u16
//! per flight, 19 B: ICAO address u24, callsign 8, type designator 4, start u32
//! per piece, in box order, 17 B: flight u24, ends 2 x 2 x i8 (1/64 of the cell's edge from the
//!   box's centroid), altitudes 2 x i16 (m), speed u16 (0.1 kt), class | flags << 12 | period <<
//!   14 u16, power code u16
//! ```

mod codes;
mod parse;
mod write;

pub use parse::Aircraft;
pub use write::encode;

const MAGIC: &[u8; 8] = b"qmair5\n\0";
const HEADER_BYTES: usize = 32;
const BAND_BYTES: usize = 5;
const MIX_BYTES: usize = 4;
/// Boxes a block of the index counts: what finding a box's periods and pieces adds up at most.
const BOXES_PER_BLOCK: usize = 32;
const BLOCK_BYTES: usize = 8;
const BOX_BYTES: usize = 27;
const PERIOD_BYTES: usize = 18;
const FLIGHT_BYTES: usize = 19;
const PIECE_BYTES: usize = 17;
/// A box head's bits: the heard periods (bit p), a helicopter, then the piece count.
const HEAD_HELICOPTER: u8 = 1 << 3;
const HEAD_PIECES_SHIFT: u32 = 4;
/// The NPD distances of Doc 29 (200 ft .. 25,000 ft).
pub const NPD_DISTANCES: usize = 10;
/// Day, evening, night.
pub const PERIODS: usize = 3;
/// Wing-mounted jet, fuselage-mounted jet, propeller (Doc 29 engine installation).
pub const INSTALLATIONS: usize = 3;
/// The finest box's zoom: a cell's coordinates fit a byte. The finest band takes the zoom whose
/// cell is nearest 50 m (`physics::doc29::box_geometry::FIRST_LAYER_EDGE_M`): 20 at the equator
/// (38 m), coarser towards the poles.
pub const MAXIMUM_ZOOM: u8 = 20;

/// Aircraft with their own noise model: one box record per group.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Group {
    FixedWing = 0,
    Helicopter = 1,
}

/// One box: a web-map cell of `zoom` (`cell` counted from the tile's north-west corner, so
/// `0..2^(zoom - 12)` on each axis) and a slab of altitudes, for one group.
#[derive(Clone, Debug, PartialEq)]
pub struct AircraftBox {
    pub zoom: u8,
    pub cell: [u16; 2],
    pub group: Group,
    /// The terrain the clearance counts from: the highest within one edge of the cell (m above
    /// sea level), and the slab from `ground_m + clearance_m` up by `height_m` (1 m steps).
    pub ground_m: f64,
    pub clearance_m: f64,
    pub height_m: f64,
    /// Emission-weighted centroid: tile-local steps (as every kind), in its cell to 1/256 of the
    /// cell's edge (the step's centre; every step at zooms 19 and 20), and altitude (1 m steps).
    pub centroid: [i16; 2],
    pub centroid_altitude_m: f64,
    /// Mean horizontal axis in [0, pi) (there and back are one axis), radians anticlockwise from
    /// east, and the climb gradient (rise over run) along it, and the spread of the pieces'
    /// gradients about it (1e-4 steps).
    pub axis_rad: f64,
    pub gradient: f64,
    pub gradient_spread: f64,
    /// Per period, the energy-weighted mean length of the pieces (1 m steps; 0 silent).
    pub piece_length_m: [f64; PERIODS],
    /// Per period, the flights that crossed the box in a year of average days (the period of
    /// each flight's first piece in the box; exact below 4,096, within 0.013 % above).
    pub flights: [u32; PERIODS],
    /// Per period, the average day's summed SEL energy at each NPD distance (dB; all -inf silent,
    /// all finite heard; within 0.025 dB), and at the tail anchor past them (16 km; within 0.05
    /// dB).
    pub energy_db: [[f64; NPD_DISTANCES]; PERIODS],
    pub tail_energy_db: [f64; PERIODS],
    /// lg of the pieces' scaled distance d_lambda (m) at each NPD distance, their
    /// energy-weighted harmonic mean (what Delta_F of short pieces sums to), within 0.002.
    pub lg_scaled_distance: [f64; NPD_DISTANCES],
    /// Per period, the energy shares of the installations at 1,000 ft and at the tail anchor
    /// (1/255 steps; the last is what the first two leave; all propeller silent).
    pub installation_shares: [[[f64; INSTALLATIONS]; 2]; PERIODS],
    /// The box's loudest pieces: `first_piece .. first_piece + piece_count` in the piece table,
    /// which holds the boxes' pieces in box order (`first_piece` is the count before the box, at
    /// most three a box), and the loudest LAmax among them at 1,000 ft (dB, rounded up to 0.5 dB;
    /// -inf without pieces): with the most an LAmax curve can rise from there, a bound on what
    /// they reach.
    pub first_piece: u32,
    pub piece_count: u8,
    pub loudest_lamax_db: f64,
}

impl AircraftBox {
    /// Whether anything is heard of `period`: a silent period's energies are all -inf.
    pub fn heard(&self, period: usize) -> bool {
        self.energy_db[period]
            .iter()
            .any(|level| *level != f64::NEG_INFINITY)
    }
}

/// One flight of the tile's table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flight {
    /// 24-bit ICAO address.
    pub icao: u32,
    /// ASCII, space padded.
    pub callsign: [u8; 8],
    /// ICAO type designator, ASCII, space padded.
    pub type_designator: [u8; 4],
    /// Start of the flight, Unix seconds.
    pub start_unix: u32,
}

/// One flight piece a box names: its ends in tile-local steps (within 1/128 of its box's cell
/// edge) and altitudes, and what the Doc 29 kernel needs to compute its level at a click.
#[derive(Clone, Debug, PartialEq)]
pub struct FlightPiece {
    /// Index into the tile's flight table.
    pub flight: u32,
    pub ends: [[i16; 2]; 2],
    pub altitudes_m: [f64; 2],
    /// Ground speed (0.1 kt steps).
    pub speed_kt: f64,
    /// The Doc 29 noise class (the flight's designator gives a helicopter its levels; below 256).
    pub class: u16,
    /// The power bracket as `physics::doc29::thrust::PowerBracket::code`.
    pub power_code: u16,
    /// Bit 0: departure NPDs; bit 1: helicopter descent.
    pub flags: u8,
    /// The flight's period (its date is the flight's start).
    pub period: u8,
}

#[cfg(test)]
mod tests;

//! `aircraft` tiles: the tile's aircraft boxes and its flight table. A box sums every flight piece
//! that crossed one web-map cell of the tile within one clearance slab, for one aircraft group: per
//! period the NPD energy at the ten NPD distances, the pieces' scaled distance there, the
//! installation shares, and the geometry of its "average aircraft" (the emission-weighted centroid,
//! one mean axis and gradient, the mean piece length). Each box also names its loudest pieces for
//! the top-flights list; a piece names a flight of the table.
//!
//! A box stores only the periods it is heard in (a silent period's ten energies are all -inf, and
//! no reader takes anything else of it), its slab as one row of the tile's band table, and finds
//! its periods and pieces by counting: every 32 boxes the counts before them, then the boxes'
//! one-byte heads up to it.
//!
//! ```text
//! magic "qmair4\n\0", u32 boxes, heard periods, flights, pieces, bands
//! per band, 5 B: zoom u8, clearance u16 (m), height u16 (m)
//! per 32 boxes, 8 B: the heard periods and the pieces of the boxes before them, u32 each
//! per box, 1 B (its head): bit p the period p heard, bit 3 a helicopter, bits 4-5 its pieces
//! per box, 40 B: band u8, cell 2 x u16, ground i16, centroid 2 x i16 and its altitude i16, axis
//!   u16, gradient i16, spread u16, lg scaled distance 10 x u16, loudest LAmax u8
//! per heard period, in box order, 31 B: energy 10 x u16, tail energy u16, flights u24, piece
//!   length u16, installation shares wing and fuselage at 1,000 ft and at the tail anchor 4 x u8
//! per flight, 19 B: ICAO address u24, callsign 8, type designator 4, start u32
//! per piece, in box order, 21 B: flight u24, ends 4 x i16, altitudes 2 x i16, speed u16, class
//!   u8, power code u16, flags | period << 2 u8
//! ```

mod parse;

pub use parse::Aircraft;

const MAGIC: &[u8; 8] = b"qmair4\n\0";
const HEADER_BYTES: usize = 28;
const BAND_BYTES: usize = 5;
/// Boxes a block of the index counts: what finding a box's periods and pieces adds up at most.
const BOXES_PER_BLOCK: usize = 32;
const BLOCK_BYTES: usize = 8;
const BOX_BYTES: usize = 40;
const PERIOD_BYTES: usize = 31;
const FLIGHT_BYTES: usize = 19;
const PIECE_BYTES: usize = 21;
/// A box head's bits: the heard periods (bit p), a helicopter, then the piece count.
const HEAD_HELICOPTER: u8 = 1 << 3;
const HEAD_PIECES_SHIFT: u32 = 4;
/// The NPD distances of Doc 29 (200 ft .. 25,000 ft).
pub const NPD_DISTANCES: usize = 10;
/// Day, evening, night.
pub const PERIODS: usize = 3;
/// Wing-mounted jet, fuselage-mounted jet, propeller (Doc 29 engine installation).
pub const INSTALLATIONS: usize = 3;
/// Stored level = code / 100 - 100 dB; code 0 is silence.
const LEVEL_OFFSET_DB: f64 = 100.0;
/// A box's loudest LAmax = code / 2 + 30 dB, rounded up; code 0 is none.
const LAMAX_OFFSET_DB: f64 = 30.0;

fn lamax_code(lamax_db: f64) -> u8 {
    if lamax_db == f64::NEG_INFINITY {
        return 0;
    }
    (2.0 * (lamax_db - LAMAX_OFFSET_DB))
        .ceil()
        .clamp(1.0, 255.0) as u8
}

fn lamax_db(code: u8) -> f64 {
    if code == 0 {
        f64::NEG_INFINITY
    } else {
        f64::from(code) / 2.0 + LAMAX_OFFSET_DB
    }
}
/// The finest box: a web-map cell of this zoom is under 0.3 m wide anywhere.
pub const MAXIMUM_ZOOM: u8 = 28;

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
    /// Emission-weighted centroid: tile-local steps (as every kind) and altitude (1 m steps).
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
    /// each flight's first piece in the box; below 2^24).
    pub flights: [u32; PERIODS],
    /// Per period, the average day's summed SEL energy at each NPD distance (dB; -inf silent),
    /// and at the tail anchor past them (16 km).
    pub energy_db: [[f64; NPD_DISTANCES]; PERIODS],
    pub tail_energy_db: [f64; PERIODS],
    /// lg of the pieces' scaled distance d_lambda (m) at each NPD distance, their
    /// energy-weighted harmonic mean (what Delta_F of short pieces sums to), in 1e-4 steps.
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

/// One flight piece a box names: its ends in tile-local steps and altitudes, and what the Doc 29
/// kernel needs to compute its level exactly at a click.
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

fn level_code(level_db: f64) -> u16 {
    if level_db == f64::NEG_INFINITY {
        return 0;
    }
    let code = ((level_db + LEVEL_OFFSET_DB) * 100.0).round();
    assert!(
        (1.0..=f64::from(u16::MAX)).contains(&code),
        "aircraft energy {level_db} dB outside the u16 range"
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

fn metres_i16(value: f64) -> i16 {
    let metres = value.round();
    assert!(
        (f64::from(i16::MIN)..=f64::from(i16::MAX)).contains(&metres),
        "{value} m outside the i16 range"
    );
    metres as i16
}

fn metres_u16(value: f64) -> u16 {
    let metres = value.round();
    assert!(
        (0.0..=f64::from(u16::MAX)).contains(&metres),
        "{value} m outside the u16 range"
    );
    metres as u16
}

fn u24(bytes: &mut Vec<u8>, value: u32, what: &str) {
    assert!(value < 1 << 24, "{what} {value} above 24 bits");
    bytes.extend_from_slice(&value.to_le_bytes()[..3]);
}

/// A box's slab as its band table row: zoom, clearance and height.
fn band_of(record: &AircraftBox) -> (u8, u16, u16) {
    (
        record.zoom,
        metres_u16(record.clearance_m),
        metres_u16(record.height_m),
    )
}

fn head_of(record: &AircraftBox) -> u8 {
    assert!(record.piece_count <= 3, "{} pieces", record.piece_count);
    let heard = (0..PERIODS)
        .filter(|&period| record.heard(period))
        .fold(0, |head, period| head | 1 << period);
    assert!(heard != 0, "a box heard in no period");
    let helicopter = match record.group {
        Group::FixedWing => 0,
        Group::Helicopter => HEAD_HELICOPTER,
    };
    heard | helicopter | record.piece_count << HEAD_PIECES_SHIFT
}

fn write_box(bytes: &mut Vec<u8>, record: &AircraftBox, band: u8) {
    assert!(
        (12..=MAXIMUM_ZOOM).contains(&record.zoom),
        "box zoom {}",
        record.zoom
    );
    let cells = 1u32 << (record.zoom - 12);
    assert!(
        record.cell.iter().all(|&c| u32::from(c) < cells),
        "box cell outside the tile"
    );
    let start = bytes.len();
    bytes.push(band);
    for value in record.cell {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&metres_i16(record.ground_m).to_le_bytes());
    for value in record.centroid {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&metres_i16(record.centroid_altitude_m).to_le_bytes());
    let axis = record.axis_rad.rem_euclid(std::f64::consts::PI) / std::f64::consts::PI;
    bytes.extend_from_slice(&(((axis * 65_536.0).round() as u32) % 65_536).to_le_bytes()[..2]);
    let gradient = (record.gradient * 10_000.0).round();
    assert!(
        gradient.abs() <= f64::from(i16::MAX),
        "gradient {}",
        record.gradient
    );
    bytes.extend_from_slice(&(gradient as i16).to_le_bytes());
    let spread = (record.gradient_spread * 10_000.0).round();
    assert!(
        (0.0..=f64::from(u16::MAX)).contains(&spread),
        "gradient spread {}",
        record.gradient_spread
    );
    bytes.extend_from_slice(&(spread as u16).to_le_bytes());
    for lg_distance in record.lg_scaled_distance {
        let code = (lg_distance.max(0.0) * 10_000.0).round();
        assert!(
            code <= f64::from(u16::MAX),
            "scaled distance 10^{lg_distance} m"
        );
        bytes.extend_from_slice(&(code as u16).to_le_bytes());
    }
    bytes.push(lamax_code(record.loudest_lamax_db));
    assert_eq!(bytes.len() - start, BOX_BYTES);
}

/// A heard period's record. Of two shares summing to one each may round up (0.9 and 0.1 to 230
/// and 26): the second keeps what the first leaves.
fn write_period(bytes: &mut Vec<u8>, record: &AircraftBox, period: usize) {
    let start = bytes.len();
    for level in record.energy_db[period] {
        bytes.extend_from_slice(&level_code(level).to_le_bytes());
    }
    bytes.extend_from_slice(&level_code(record.tail_energy_db[period]).to_le_bytes());
    u24(bytes, record.flights[period], "flights");
    bytes.extend_from_slice(&metres_u16(record.piece_length_m[period]).to_le_bytes());
    for shares in &record.installation_shares[period] {
        assert!(
            shares.iter().all(|share| (0.0..=1.0).contains(share)),
            "installation shares {shares:?}"
        );
        let wing = (shares[0] * 255.0).round();
        let fuselage = (shares[1] * 255.0).round().min(255.0 - wing);
        bytes.extend_from_slice(&[wing as u8, fuselage as u8]);
    }
    assert_eq!(bytes.len() - start, PERIOD_BYTES);
}

fn write_piece(bytes: &mut Vec<u8>, piece: &FlightPiece) {
    let start = bytes.len();
    u24(bytes, piece.flight, "flight");
    for coordinate in piece.ends.iter().flatten() {
        bytes.extend_from_slice(&coordinate.to_le_bytes());
    }
    for altitude in piece.altitudes_m {
        bytes.extend_from_slice(&metres_i16(altitude).to_le_bytes());
    }
    let speed = (piece.speed_kt * 10.0)
        .round()
        .clamp(0.0, f64::from(u16::MAX));
    bytes.extend_from_slice(&(speed as u16).to_le_bytes());
    bytes.push(u8::try_from(piece.class).expect("a noise class below 256"));
    bytes.extend_from_slice(&piece.power_code.to_le_bytes());
    assert!(
        piece.flags < 4 && piece.period < 4,
        "piece flags {} period {}",
        piece.flags,
        piece.period
    );
    bytes.push(piece.flags | piece.period << 2);
    assert_eq!(bytes.len() - start, PIECE_BYTES);
}

/// The bytes of an aircraft file. The pieces follow their boxes' order, every piece's flight
/// exists, and a silent period carries no tail energy and no flights.
pub fn encode(boxes: &[AircraftBox], flights: &[Flight], pieces: &[FlightPiece]) -> Vec<u8> {
    let mut bands: Vec<(u8, u16, u16)> = boxes.iter().map(band_of).collect();
    bands.sort_unstable();
    bands.dedup();
    assert!(bands.len() <= 256, "{} bands in a tile", bands.len());
    let heads: Vec<u8> = boxes.iter().map(head_of).collect();
    let periods: usize = heads
        .iter()
        .map(|head| (head & 7).count_ones() as usize)
        .sum();
    let blocks = boxes.len().div_ceil(BOXES_PER_BLOCK);
    let mut bytes = Vec::with_capacity(
        HEADER_BYTES
            + BAND_BYTES * bands.len()
            + BLOCK_BYTES * blocks
            + (1 + BOX_BYTES) * boxes.len()
            + PERIOD_BYTES * periods
            + FLIGHT_BYTES * flights.len()
            + PIECE_BYTES * pieces.len(),
    );
    bytes.extend_from_slice(MAGIC);
    for count in [
        boxes.len(),
        periods,
        flights.len(),
        pieces.len(),
        bands.len(),
    ] {
        bytes.extend_from_slice(&u32::try_from(count).expect("tile too large").to_le_bytes());
    }
    for &(zoom, clearance, height) in &bands {
        bytes.push(zoom);
        bytes.extend_from_slice(&clearance.to_le_bytes());
        bytes.extend_from_slice(&height.to_le_bytes());
    }
    let (mut periods_before, mut pieces_before) = (0u32, 0u32);
    for (index, (record, head)) in boxes.iter().zip(&heads).enumerate() {
        if index % BOXES_PER_BLOCK == 0 {
            bytes.extend_from_slice(&periods_before.to_le_bytes());
            bytes.extend_from_slice(&pieces_before.to_le_bytes());
        }
        assert_eq!(
            record.first_piece, pieces_before,
            "a box's pieces must follow the boxes before it"
        );
        periods_before += (head & 7).count_ones();
        pieces_before += u32::from(record.piece_count);
    }
    assert!(
        pieces_before as usize <= pieces.len(),
        "a box names a missing piece"
    );
    bytes.extend_from_slice(&heads);
    for record in boxes {
        let band = bands.binary_search(&band_of(record)).expect("its band");
        write_box(&mut bytes, record, band as u8);
    }
    for record in boxes {
        for period in 0..PERIODS {
            if record.heard(period) {
                write_period(&mut bytes, record, period);
            } else {
                assert!(
                    record.tail_energy_db[period] == f64::NEG_INFINITY
                        && record.flights[period] == 0,
                    "a silent period with energy or flights"
                );
            }
        }
    }
    for flight in flights {
        u24(&mut bytes, flight.icao, "ICAO address");
        bytes.extend_from_slice(&flight.callsign);
        bytes.extend_from_slice(&flight.type_designator);
        bytes.extend_from_slice(&flight.start_unix.to_le_bytes());
    }
    for piece in pieces {
        assert!(
            (piece.flight as usize) < flights.len(),
            "a piece names a missing flight"
        );
        write_piece(&mut bytes, piece);
    }
    bytes
}

#[cfg(test)]
mod tests;

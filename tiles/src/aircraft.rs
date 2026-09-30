//! `aircraft` tiles: the tile's aircraft boxes and its flight table. A box sums every flight piece
//! that crossed one web-map cell of the tile within one clearance slab, for one aircraft group: per
//! period the NPD energy at the ten NPD distances, the energy-weighted SEL minus LAmax there, the
//! installation shares, and the geometry of its "average aircraft" (the emission-weighted centroid,
//! one mean axis and gradient, the mean piece length). Each box also names its loudest pieces for
//! the top-flights list; a piece names a flight of the table.

mod parse;

pub use parse::Aircraft;

const MAGIC: &[u8; 8] = b"qmair1\n\0";
const HEADER_BYTES: usize = 24;
const BOX_BYTES: usize = 104;
const FLIGHT_BYTES: usize = 20;
const PIECE_BYTES: usize = 24;
/// The NPD distances of Doc 29 (200 ft .. 25,000 ft).
pub const NPD_DISTANCES: usize = 10;
/// Day, evening, night.
pub const PERIODS: usize = 3;
/// Wing-mounted jet, fuselage-mounted jet, propeller (Doc 29 engine installation).
pub const INSTALLATIONS: usize = 3;
/// Stored level = code / 100 - 100 dB; code 0 is silence.
const LEVEL_OFFSET_DB: f64 = 100.0;
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
    /// The slab: altitudes above sea level from `floor_m` to `floor_m + height_m` (1 m steps).
    pub floor_m: f64,
    pub height_m: f64,
    /// Emission-weighted centroid: tile-local steps (as every kind) and altitude (1 m steps).
    pub centroid: [i16; 2],
    pub centroid_altitude_m: f64,
    /// Mean horizontal axis in [0, pi) (there and back are one axis), radians anticlockwise from
    /// east, and the climb gradient (rise over run) along it.
    pub axis_rad: f64,
    pub gradient: f64,
    /// Energy-weighted mean length of the pieces (1 m steps).
    pub piece_length_m: f64,
    /// Flights that crossed the box in the year.
    pub flights: u32,
    /// Per period, the average day's summed SEL energy at each NPD distance (dB; -inf silent).
    pub energy_db: [[f64; NPD_DISTANCES]; PERIODS],
    /// The energy-weighted SEL minus LAmax at each NPD distance (0.1 dB steps, 0..25.5 dB).
    pub sel_minus_lamax_db: [f64; NPD_DISTANCES],
    /// Energy shares of the installations (1/255 steps; the last is what the first two leave).
    pub installation_shares: [f64; INSTALLATIONS],
    /// The box's loudest pieces: `first_piece .. first_piece + piece_count` in the piece table.
    pub first_piece: u32,
    pub piece_count: u8,
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
    pub profile: u16,
    /// The kernel's power setting, as its code.
    pub power_code: u16,
    /// Kernel flags (departure, helicopter descent, ...).
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

fn write_box(bytes: &mut Vec<u8>, record: &AircraftBox) {
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
    bytes.push(record.zoom);
    bytes.push(record.group as u8);
    for value in record.cell {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&metres_i16(record.floor_m).to_le_bytes());
    bytes.extend_from_slice(&metres_u16(record.height_m).to_le_bytes());
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
    bytes.extend_from_slice(&metres_u16(record.piece_length_m).to_le_bytes());
    bytes.extend_from_slice(&record.flights.to_le_bytes());
    for level in record.energy_db.iter().flatten() {
        bytes.extend_from_slice(&level_code(*level).to_le_bytes());
    }
    for difference in record.sel_minus_lamax_db {
        bytes.push((difference * 10.0).round().clamp(0.0, 255.0) as u8);
    }
    let [wing, fuselage, _] = record
        .installation_shares
        .map(|share| (share * 255.0).round());
    assert!(wing + fuselage <= 255.0, "installation shares above one");
    bytes.extend_from_slice(&[wing as u8, fuselage as u8]);
    bytes.extend_from_slice(&record.first_piece.to_le_bytes());
    bytes.push(record.piece_count);
    bytes.resize(start + BOX_BYTES, 0);
}

fn write_piece(bytes: &mut Vec<u8>, piece: &FlightPiece) {
    let start = bytes.len();
    bytes.extend_from_slice(&piece.flight.to_le_bytes());
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
    bytes.extend_from_slice(&piece.profile.to_le_bytes());
    bytes.extend_from_slice(&piece.power_code.to_le_bytes());
    bytes.extend_from_slice(&[piece.flags, piece.period]);
    assert_eq!(bytes.len() - start, PIECE_BYTES);
}

/// The bytes of an aircraft file: header, 104-byte boxes, 20-byte flights, 24-byte pieces.
/// Every piece's flight and every box's pieces must exist.
pub fn encode(boxes: &[AircraftBox], flights: &[Flight], pieces: &[FlightPiece]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(
        HEADER_BYTES
            + BOX_BYTES * boxes.len()
            + FLIGHT_BYTES * flights.len()
            + PIECE_BYTES * pieces.len(),
    );
    bytes.extend_from_slice(MAGIC);
    for count in [boxes.len(), flights.len(), pieces.len()] {
        bytes.extend_from_slice(&u32::try_from(count).expect("tile too large").to_le_bytes());
    }
    bytes.extend_from_slice(&[0; 4]);
    for record in boxes {
        let end = record.first_piece as usize + usize::from(record.piece_count);
        assert!(end <= pieces.len(), "a box names a missing piece");
        write_box(&mut bytes, record);
    }
    for flight in flights {
        assert!(flight.icao < 1 << 24, "ICAO address above 24 bits");
        bytes.extend_from_slice(&flight.icao.to_le_bytes()[..3]);
        bytes.extend_from_slice(&flight.callsign);
        bytes.extend_from_slice(&flight.type_designator);
        bytes.extend_from_slice(&flight.start_unix.to_le_bytes());
        bytes.push(0);
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

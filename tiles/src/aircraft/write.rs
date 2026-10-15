//! Writing an aircraft file: the tables first (bands, installation mixes, the counts every 32
//! boxes), then the boxes' heads and records, their heard periods, the flights and the pieces.

use super::codes::{
    WIDE_SCALE, cell_steps, curve_codes, in_cell, in_cell_code, lamax_code, lg_distance_codes,
    offset_code, offset_scale, tail_code,
};
use super::{
    AircraftBox, BAND_BYTES, BLOCK_BYTES, BOX_BYTES, BOXES_PER_BLOCK, FLIGHT_BYTES, Flight,
    FlightPiece, Group, HEAD_HELICOPTER, HEAD_PIECES_SHIFT, HEAD_REACHING, HEADER_BYTES,
    INSTALLATIONS, KIND_FLAGS_SHIFT, KIND_PERIOD_SHIFT, KIND_SCALE_SHIFT, MAGIC, MAXIMUM_ZOOM,
    MIX_BYTES, PERIOD_BYTES, PERIODS, PIECE_BYTES, WIDE_BYTES,
};
use crate::rate::rate_code;
use std::collections::HashMap;

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

/// A period's installation mix as stored: wing and fuselage shares at the two slants. Of two
/// shares summing to one each may round up (0.9 and 0.1 to 230 and 26): the second keeps what the
/// first leaves.
fn mix_of(shares: &[[f64; INSTALLATIONS]; 2]) -> [u8; MIX_BYTES] {
    let mut mix = [0; MIX_BYTES];
    for (slant, shares) in shares.iter().enumerate() {
        assert!(
            shares.iter().all(|share| (0.0..=1.0).contains(share)),
            "installation shares {shares:?}"
        );
        let wing = (shares[0] * 255.0).round();
        let fuselage = (shares[1] * 255.0).round().min(255.0 - wing);
        mix[2 * slant..2 * slant + 2].copy_from_slice(&[wing as u8, fuselage as u8]);
    }
    mix
}

/// Whether a piece of a box reaches past its cell or its slab by more than an eighth of the edge
/// (the slab as stored, to the metre): a reader bounds the others by their box alone.
fn reaches_past(record: &AircraftBox, pieces: &[FlightPiece]) -> bool {
    let edge = cell_steps(record.zoom);
    let (ground, clearance, height) = (
        f64::from(metres_i16(record.ground_m)),
        f64::from(metres_u16(record.clearance_m)),
        f64::from(metres_u16(record.height_m)),
    );
    let floor = if clearance == 0.0 {
        f64::NEG_INFINITY
    } else {
        ground + clearance
    };
    let top = ground + clearance + height;
    pieces.iter().any(|piece| {
        let outside_cell = piece.ends.iter().any(|end| {
            (0..2).any(|axis| {
                let west = -16_384 + i32::from(record.cell[axis]) * edge;
                let coordinate = i32::from(end[axis]);
                coordinate < west - edge / 8 || coordinate > west + edge + edge / 8
            })
        });
        let outside_slab = piece
            .altitudes_m
            .iter()
            .any(|&altitude| altitude < floor - height / 8.0 || altitude > top + height / 8.0);
        outside_cell || outside_slab
    })
}

fn head_of(record: &AircraftBox, reaching: bool) -> u8 {
    assert!(record.piece_count <= 3, "{} pieces", record.piece_count);
    let heard = (0..PERIODS)
        .filter(|&period| record.heard(period))
        .fold(0, |head, period| head | 1 << period);
    assert!(heard != 0, "a box heard in no period");
    let helicopter = match record.group {
        Group::FixedWing => 0,
        Group::Helicopter => HEAD_HELICOPTER,
    };
    heard
        | helicopter
        | record.piece_count << HEAD_PIECES_SHIFT
        | if reaching { HEAD_REACHING } else { 0 }
}

/// A box's cell and centroid as stored, and the centroid they read back as (what its pieces are
/// stored from).
fn place_of(record: &AircraftBox) -> ([u8; 2], [u8; 2], [i16; 2]) {
    assert!(
        (12..=MAXIMUM_ZOOM).contains(&record.zoom),
        "box zoom {}",
        record.zoom
    );
    let edge = cell_steps(record.zoom);
    let cells: [u8; 2] = record.cell.map(|c| {
        u8::try_from(c)
            .ok()
            .filter(|&c| i32::from(c) * edge < 32_768)
            .expect("a box cell inside the tile")
    });
    let places: [u8; 2] =
        std::array::from_fn(|axis| in_cell_code(record.centroid[axis], cells[axis], edge));
    let centroid = std::array::from_fn(|axis| in_cell(places[axis], cells[axis], edge));
    (cells, places, centroid)
}

fn write_box(bytes: &mut Vec<u8>, record: &AircraftBox, band: u8) {
    let start = bytes.len();
    let (cells, places, _) = place_of(record);
    bytes.push(band);
    bytes.extend_from_slice(&cells);
    bytes.extend_from_slice(&metres_i16(record.ground_m).to_le_bytes());
    bytes.extend_from_slice(&places);
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
    let (first, steps) = lg_distance_codes(&record.lg_scaled_distance);
    bytes.extend_from_slice(&first.to_le_bytes());
    bytes.extend_from_slice(&steps);
    bytes.push(lamax_code(record.loudest_lamax_db));
    assert_eq!(bytes.len() - start, BOX_BYTES);
}

fn write_period(bytes: &mut Vec<u8>, record: &AircraftBox, period: usize, mix: u16) {
    let start = bytes.len();
    let (first, drops, end) = curve_codes(&record.energy_db[period]);
    bytes.extend_from_slice(&first.to_le_bytes());
    bytes.extend_from_slice(&drops);
    bytes.push(tail_code(end, record.tail_energy_db[period]));
    bytes.extend_from_slice(&rate_code(record.flights_per_day[period]).to_le_bytes());
    bytes.extend_from_slice(&metres_u16(record.piece_length_m[period]).to_le_bytes());
    bytes.extend_from_slice(&mix.to_le_bytes());
    assert_eq!(bytes.len() - start, PERIOD_BYTES);
}

/// A piece, its ends from its box's stored centroid at `scale` (a wide piece's in its table).
fn write_piece(bytes: &mut Vec<u8>, piece: &FlightPiece, centroid: [i16; 2], scale: u8) {
    let start = bytes.len();
    u24(bytes, piece.flight, "flight");
    for end in piece.ends {
        for axis in 0..2 {
            bytes.push(if scale == WIDE_SCALE {
                0
            } else {
                offset_code(end[axis], centroid[axis], scale) as u8
            });
        }
    }
    for altitude in piece.altitudes_m {
        bytes.extend_from_slice(&metres_i16(altitude).to_le_bytes());
    }
    let speed = (piece.speed_kt * 10.0)
        .round()
        .clamp(0.0, f64::from(u16::MAX));
    bytes.extend_from_slice(&(speed as u16).to_le_bytes());
    assert!(
        piece.class < 1 << KIND_FLAGS_SHIFT && piece.flags < 4 && piece.period < 4,
        "piece class {} flags {} period {}",
        piece.class,
        piece.flags,
        piece.period
    );
    let kind = piece.class
        | u16::from(piece.flags) << KIND_FLAGS_SHIFT
        | u16::from(piece.period) << KIND_PERIOD_SHIFT
        | u16::from(scale) << KIND_SCALE_SHIFT;
    bytes.extend_from_slice(&kind.to_le_bytes());
    bytes.extend_from_slice(&piece.power_code.to_le_bytes());
    assert_eq!(bytes.len() - start, PIECE_BYTES);
}

/// The bytes of an aircraft file. The pieces follow their boxes' order, every piece's flight
/// exists, and a silent period carries no tail energy and no flights.
pub fn encode(boxes: &[AircraftBox], flights: &[Flight], pieces: &[FlightPiece]) -> Vec<u8> {
    let mut bands: Vec<(u8, u16, u16)> = boxes.iter().map(band_of).collect();
    bands.sort_unstable();
    bands.dedup();
    assert!(bands.len() <= 256, "{} bands in a tile", bands.len());

    let mut mixes: Vec<[u8; MIX_BYTES]> = Vec::new();
    let mut mix_rows: HashMap<[u8; MIX_BYTES], u16> = HashMap::new();
    let mut periods: Vec<(usize, usize, u16)> = Vec::new();
    for (index, record) in boxes.iter().enumerate() {
        for period in 0..PERIODS {
            if !record.heard(period) {
                assert!(
                    record.tail_energy_db[period] == f64::NEG_INFINITY
                        && record.flights_per_day[period] == 0.0,
                    "a silent period with energy or flights"
                );
                continue;
            }
            let mix = mix_of(&record.installation_shares[period]);
            let row = *mix_rows.entry(mix).or_insert_with(|| {
                mixes.push(mix);
                u16::try_from(mixes.len() - 1).expect("installation mixes of a tile")
            });
            periods.push((index, period, row));
        }
    }
    // Each box's pieces follow the boxes before it; a piece's scale, the wide ones' ends exact.
    let (mut scales, mut wide) = (Vec::with_capacity(pieces.len()), Vec::new());
    for record in boxes {
        assert_eq!(
            record.first_piece as usize,
            scales.len(),
            "a box's pieces must follow the boxes before it"
        );
        let (first, count) = (record.first_piece as usize, usize::from(record.piece_count));
        assert!(first + count <= pieces.len(), "a box names a missing piece");
        let (_, _, centroid) = place_of(record);
        for (piece, index) in pieces[first..first + count].iter().zip(first..) {
            let scale = offset_scale(&piece.ends, centroid);
            if scale == WIDE_SCALE {
                wide.push((index as u32, piece.ends));
            }
            scales.push(scale);
        }
    }
    assert!(scales.len() == pieces.len(), "a piece no box names");
    let heads: Vec<u8> = boxes
        .iter()
        .map(|record| {
            let first = record.first_piece as usize;
            let mine = &pieces[first..first + usize::from(record.piece_count)];
            head_of(record, reaches_past(record, mine))
        })
        .collect();
    let blocks = boxes.len().div_ceil(BOXES_PER_BLOCK);
    let mut bytes = Vec::with_capacity(
        HEADER_BYTES
            + BAND_BYTES * bands.len()
            + MIX_BYTES * mixes.len()
            + BLOCK_BYTES * blocks
            + (1 + BOX_BYTES) * boxes.len()
            + PERIOD_BYTES * periods.len()
            + FLIGHT_BYTES * flights.len()
            + PIECE_BYTES * pieces.len()
            + WIDE_BYTES * wide.len(),
    );
    bytes.extend_from_slice(MAGIC);
    for count in [
        boxes.len(),
        periods.len(),
        flights.len(),
        pieces.len(),
        bands.len(),
        mixes.len(),
        wide.len(),
    ] {
        bytes.extend_from_slice(&u32::try_from(count).expect("tile too large").to_le_bytes());
    }
    for &(zoom, clearance, height) in &bands {
        bytes.push(zoom);
        bytes.extend_from_slice(&clearance.to_le_bytes());
        bytes.extend_from_slice(&height.to_le_bytes());
    }
    for mix in &mixes {
        bytes.extend_from_slice(mix);
    }
    let (mut periods_before, mut pieces_before) = (0u32, 0u32);
    for (index, (record, head)) in boxes.iter().zip(&heads).enumerate() {
        if index % BOXES_PER_BLOCK == 0 {
            bytes.extend_from_slice(&periods_before.to_le_bytes());
            bytes.extend_from_slice(&pieces_before.to_le_bytes());
        }
        periods_before += (head & 7).count_ones();
        pieces_before += u32::from(record.piece_count);
    }
    bytes.extend_from_slice(&heads);
    for record in boxes {
        let band = bands.binary_search(&band_of(record)).expect("its band");
        write_box(&mut bytes, record, band as u8);
    }
    for &(index, period, mix) in &periods {
        write_period(&mut bytes, &boxes[index], period, mix);
    }
    for flight in flights {
        u24(&mut bytes, flight.icao, "ICAO address");
        bytes.extend_from_slice(&flight.callsign);
        bytes.extend_from_slice(&flight.type_designator);
        bytes.extend_from_slice(&flight.start_unix.to_le_bytes());
    }
    for record in boxes {
        let (first, (_, _, centroid)) = (record.first_piece as usize, place_of(record));
        for (piece, &scale) in pieces[first..first + usize::from(record.piece_count)]
            .iter()
            .zip(&scales[first..])
        {
            assert!(
                (piece.flight as usize) < flights.len(),
                "a piece names a missing flight"
            );
            write_piece(&mut bytes, piece, centroid, scale);
        }
    }
    for (index, ends) in &wide {
        bytes.extend_from_slice(&index.to_le_bytes());
        for coordinate in ends.iter().flatten() {
            bytes.extend_from_slice(&coordinate.to_le_bytes());
        }
    }
    bytes
}

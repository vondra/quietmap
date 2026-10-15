//! The aircraft file round trip within its codes' steps, and its refusals.

use super::*;

/// A box at zoom 19 in cell (127, 3), heard in all three periods.
fn a_box(first_piece: u32, piece_count: u8) -> AircraftBox {
    let energy_db = std::array::from_fn(|period| {
        std::array::from_fn(|distance| 120.0 - 3.0 * distance as f64 - period as f64)
    });
    AircraftBox {
        zoom: 19,
        cell: [127, 3],
        group: Group::Helicopter,
        ground_m: 301.0,
        clearance_m: 49.0,
        height_m: 49.0,
        centroid: [16_200, -15_500],
        centroid_altitude_m: 371.0,
        axis_rad: 2.5,
        gradient: -0.052,
        gradient_spread: 0.0412,
        piece_length_m: [44.0, 51.0, 38.0],
        flights: [123_456, 7, 16_777_215],
        energy_db,
        lg_scaled_distance: [
            61.0f64, 150.0, 288.0, 500.0, 1_100.0, 2_566.5, 4_259.3, 6_780.3, 9_000.0, 1e4,
        ]
        .map(f64::log10),
        tail_energy_db: [80.77, 79.0, 70.31],
        installation_shares: [
            [[0.2, 0.4, 0.4], [0.1, 0.3, 0.6]],
            [[1.0, 0.0, 0.0], [0.9, 0.1, 0.0]],
            [[0.0, 0.0, 1.0], [0.05, 0.05, 0.9]],
        ],
        first_piece,
        piece_count,
        loudest_lamax_db: if piece_count > 0 {
            91.26
        } else {
            f64::NEG_INFINITY
        },
    }
}

fn a_flight(icao: u32) -> Flight {
    Flight {
        icao,
        callsign: *b"CSA123  ",
        type_designator: *b"A139",
        start_unix: 1_756_684_800,
    }
}

/// A piece across a_box's cell.
fn a_piece(flight: u32) -> FlightPiece {
    FlightPiece {
        flight,
        ends: [[16_150, -15_490], [16_420, -15_620]],
        altitudes_m: [420.0, 455.0],
        speed_kt: 142.3,
        class: 11,
        power_code: 4_321,
        flags: 1,
        period: 2,
    }
}

fn close(read: f64, written: f64, step: f64) -> bool {
    read == written || (read - written).abs() <= step
}

#[test]
fn boxes_flights_and_pieces_round_trip_within_their_steps() {
    let boxes = [a_box(0, 2), a_box(2, 0)];
    let flights = [a_flight(0x4b_a9_c1), a_flight(0x00_00_01)];
    let pieces = [a_piece(1), a_piece(0)];
    let bytes = encode(&boxes, &flights, &pieces);
    // One band, three mixes, one block of the index, two heads and boxes, three heard periods
    // each.
    assert_eq!(
        bytes.len(),
        32 + 5 + 3 * 4 + 8 + 2 * (1 + 27) + 6 * 18 + 2 * 19 + 2 * 17
    );
    let parsed = Aircraft::parse(&bytes).unwrap();
    assert_eq!(
        (
            parsed.box_count(),
            parsed.flight_count(),
            parsed.piece_count()
        ),
        (2, 2, 2)
    );
    let read = parsed.aircraft_box(0);
    let written = &boxes[0];
    assert_eq!(
        (read.zoom, read.cell, read.group),
        (19, [127, 3], Group::Helicopter)
    );
    assert_eq!(
        (read.ground_m, read.clearance_m, read.height_m),
        (301.0, 49.0, 49.0)
    );
    // Zoom 19: a cell of 256 steps keeps every step of its centroid.
    assert_eq!(
        (read.centroid, read.centroid_altitude_m),
        ([16_200, -15_500], 371.0)
    );
    assert_eq!(parsed.box_centroid(0), read.centroid);
    assert!((read.axis_rad - 2.5).abs() < 1e-4);
    assert!((read.gradient - -0.052).abs() < 1e-4);
    assert_eq!(read.piece_length_m, [44.0, 51.0, 38.0]);
    for (r, w) in read.flights.iter().zip(written.flights) {
        assert!(
            close(f64::from(*r), f64::from(w), f64::from(w) * 1.3e-4),
            "{r} vs {w}"
        );
    }
    assert_eq!(read.flights[1], 7);
    for (read_levels, written_levels) in read.energy_db.iter().zip(&written.energy_db) {
        for (r, w) in read_levels.iter().zip(written_levels) {
            assert!(close(*r, *w, 0.0251), "{r} vs {w}");
        }
    }
    for (r, w) in read.tail_energy_db.iter().zip(written.tail_energy_db) {
        assert!(close(*r, w, 0.0751), "{r} vs {w}");
    }
    for (r, w) in read
        .lg_scaled_distance
        .iter()
        .zip(written.lg_scaled_distance)
    {
        assert!(close(*r, w, 0.00201), "{r} vs {w}");
    }
    for (read, written) in read
        .installation_shares
        .iter()
        .flatten()
        .zip(written.installation_shares.iter().flatten())
    {
        for (r, w) in read.iter().zip(written) {
            assert!(close(*r, *w, 1.0 / 255.0), "{r} vs {w}");
        }
    }
    assert_eq!((read.first_piece, read.piece_count), (0, 2));
    // Rounded up, so a bound stays a bound.
    assert_eq!(read.loudest_lamax_db, 91.5);
    assert_eq!(parsed.flight(0), flights[0]);
    assert_eq!(parsed.flight(1), flights[1]);
    let piece = parsed.piece(&read, 0);
    assert_eq!(piece.flight, 1);
    // 1/64 of a 256-step cell: within 2 steps.
    for (r, w) in piece
        .ends
        .iter()
        .flatten()
        .zip(a_piece(0).ends.iter().flatten())
    {
        assert!((i32::from(*r) - i32::from(*w)).abs() <= 2, "{r} vs {w}");
    }
    assert_eq!(piece.altitudes_m, [420.0, 455.0]);
    assert!((piece.speed_kt - 142.3).abs() < 1e-9);
    assert_eq!(
        (piece.class, piece.power_code, piece.flags, piece.period),
        (11, 4_321, 1, 2)
    );
}

#[test]
fn a_silent_period_is_left_out_and_reads_back_silent() {
    let mut quiet_evening = a_box(0, 1);
    quiet_evening.energy_db[1] = [f64::NEG_INFINITY; NPD_DISTANCES];
    quiet_evening.tail_energy_db[1] = f64::NEG_INFINITY;
    quiet_evening.flights[1] = 0;
    let loud = a_box(1, 1);
    let bytes = encode(
        &[quiet_evening.clone(), loud.clone()],
        &[a_flight(1)],
        &[a_piece(0), a_piece(0)],
    );
    let parsed = Aircraft::parse(&bytes).unwrap();
    let read = parsed.aircraft_box(0);
    assert_eq!(read.energy_db[1], [f64::NEG_INFINITY; NPD_DISTANCES]);
    assert_eq!(read.tail_energy_db[1], f64::NEG_INFINITY);
    assert_eq!(
        (read.flights[1], read.piece_length_m[1]),
        (0, 0.0),
        "a silent period keeps no length"
    );
    assert_eq!(read.installation_shares[1], [[0.0, 0.0, 1.0]; 2]);
    // The next box finds its own periods and pieces after the left-out evening.
    let next = parsed.aircraft_box(1);
    assert_eq!((next.first_piece, next.piece_count), (1, 1));
    assert_eq!(next.flights[1], loud.flights[1]);
    assert_eq!(next.piece_length_m, loud.piece_length_m);
}

#[test]
fn boxes_beyond_a_block_find_their_periods_and_pieces() {
    let boxes: Vec<AircraftBox> = (0..70u32)
        .map(|index| {
            let mut record = a_box(0, (index % 2) as u8);
            // Evening or night silent in two boxes of three.
            for period in 1..PERIODS {
                if (index as usize + period).is_multiple_of(3) {
                    record.energy_db[period] = [f64::NEG_INFINITY; NPD_DISTANCES];
                    record.tail_energy_db[period] = f64::NEG_INFINITY;
                    record.flights[period] = 0;
                }
            }
            record.flights[0] = index;
            record
        })
        .scan(0u32, |first, mut record| {
            record.first_piece = *first;
            *first += u32::from(record.piece_count);
            Some(record)
        })
        .collect();
    let pieces = vec![a_piece(0); 35];
    let bytes = encode(&boxes, &[a_flight(1)], &pieces);
    let parsed = Aircraft::parse(&bytes).unwrap();
    for (index, written) in boxes.iter().enumerate() {
        let read = parsed.aircraft_box(index);
        assert_eq!(read.flights[0], index as u32);
        assert_eq!(
            (read.first_piece, read.piece_count),
            (written.first_piece, written.piece_count)
        );
        for period in 0..PERIODS {
            assert_eq!(read.heard(period), written.heard(period));
        }
    }
}

#[test]
fn a_file_that_does_not_add_up_is_refused() {
    let bytes = encode(&[a_box(0, 1)], &[a_flight(1)], &[a_piece(0)]);
    assert!(Aircraft::parse(&bytes).is_ok());
    assert!(Aircraft::parse(&bytes[..bytes.len() - 1]).is_err());
    let mut wrong_magic = bytes.clone();
    wrong_magic[0] = b'x';
    assert!(Aircraft::parse(&wrong_magic).is_err());
    // The head after the band, the three mixes and the block: three pieces where the table has
    // one, then a bit above the piece count.
    let head = 32 + 5 + 3 * 4 + 8;
    let mut missing_piece = bytes.clone();
    missing_piece[head] = missing_piece[head] & 0x0f | 3 << 4;
    assert!(Aircraft::parse(&missing_piece).is_err());
    let mut high_head = bytes.clone();
    high_head[head] |= 1 << 6;
    assert!(Aircraft::parse(&high_head).is_err());
    // A box of a band the table lacks.
    let mut missing_band = bytes.clone();
    missing_band[head + 1] = 1;
    assert!(Aircraft::parse(&missing_band).is_err());
    // An index block counting pieces before the first box.
    let mut miscounted = bytes.clone();
    miscounted[32 + 5 + 3 * 4 + 4] = 1;
    assert!(Aircraft::parse(&miscounted).is_err());
    // A period naming a mix beyond the table.
    let period = head + 1 + 27;
    let mut missing_mix = bytes.clone();
    missing_mix[period + 16] = 9;
    assert!(Aircraft::parse(&missing_mix).is_err());
    // A piece naming a flight beyond the table.
    let mut missing_flight = bytes;
    missing_flight[period + 3 * 18 + 19] = 9;
    assert!(Aircraft::parse(&missing_flight).is_err());
}

#[test]
#[should_panic(expected = "names a missing piece")]
fn encoding_a_box_without_its_pieces_panics() {
    encode(&[a_box(0, 3)], &[a_flight(1)], &[a_piece(0)]);
}

#[test]
#[should_panic(expected = "must follow the boxes before it")]
fn encoding_pieces_out_of_box_order_panics() {
    encode(
        &[a_box(1, 1), a_box(0, 1)],
        &[a_flight(1)],
        &[a_piece(0), a_piece(0)],
    );
}

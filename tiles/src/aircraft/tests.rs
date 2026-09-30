//! The aircraft file round trip and its refusals.

use super::*;

fn a_box(first_piece: u32, piece_count: u8) -> AircraftBox {
    let mut energy_db = [[f64::NEG_INFINITY; NPD_DISTANCES]; PERIODS];
    for (period, levels) in energy_db.iter_mut().enumerate() {
        for (distance, level) in levels.iter_mut().enumerate() {
            *level = 120.0 - 3.0 * distance as f64 - period as f64;
        }
    }
    energy_db[2][9] = f64::NEG_INFINITY;
    AircraftBox {
        zoom: 19,
        cell: [127, 3],
        group: Group::Helicopter,
        ground_m: 301.0,
        clearance_m: 49.0,
        height_m: 49.0,
        centroid: [-16_000, 12_345],
        centroid_altitude_m: 371.0,
        axis_rad: 2.5,
        gradient: -0.052,
        piece_length_m: 44.0,
        flights: 123_456,
        energy_db,
        scaled_distance_m: [
            61.0, 150.0, 288.0, 500.0, 1_100.0, 2_566.5, 4_259.3, 6_780.3, 9_000.0, 1e6,
        ],
        installation_shares: [0.2, 0.4, 0.4],
        first_piece,
        piece_count,
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

fn a_piece(flight: u32) -> FlightPiece {
    FlightPiece {
        flight,
        ends: [[-5, 7], [100, -300]],
        altitudes_m: [420.0, 455.0],
        speed_kt: 142.3,
        class: 11,
        power_code: 4_321,
        flags: 1,
        period: 2,
    }
}

#[test]
fn boxes_flights_and_pieces_round_trip_at_their_steps() {
    let boxes = [a_box(0, 2), a_box(2, 0)];
    let flights = [a_flight(0x4b_a9_c1), a_flight(0x00_00_01)];
    let pieces = [a_piece(1), a_piece(0)];
    let bytes = encode(&boxes, &flights, &pieces);
    assert_eq!(bytes.len(), 24 + 2 * 116 + 2 * 20 + 2 * 24);
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
    assert_eq!(
        (read.centroid, read.centroid_altitude_m),
        ([-16_000, 12_345], 371.0)
    );
    assert!((read.axis_rad - 2.5).abs() < 1e-4);
    assert!((read.gradient - -0.052).abs() < 1e-4);
    assert_eq!((read.piece_length_m, read.flights), (44.0, 123_456));
    for (read_levels, written_levels) in read.energy_db.iter().zip(&written.energy_db) {
        for (r, w) in read_levels.iter().zip(written_levels) {
            assert!(r == w || (r - w).abs() <= 0.005, "{r} vs {w}");
        }
    }
    for (r, w) in read.scaled_distance_m.iter().zip(written.scaled_distance_m) {
        assert!((r / w - 1.0).abs() < 1.2e-4, "{r} vs {w}");
    }
    for (r, w) in read
        .installation_shares
        .iter()
        .zip(written.installation_shares)
    {
        assert!((r - w).abs() <= 1.0 / 255.0, "{r} vs {w}");
    }
    assert_eq!((read.first_piece, read.piece_count), (0, 2));
    assert_eq!(parsed.flight(0), flights[0]);
    assert_eq!(parsed.flight(1), flights[1]);
    let piece = parsed.piece(0);
    assert_eq!((piece.flight, piece.ends), (1, [[-5, 7], [100, -300]]));
    assert_eq!(piece.altitudes_m, [420.0, 455.0]);
    assert!((piece.speed_kt - 142.3).abs() < 1e-9);
    assert_eq!(
        (piece.class, piece.power_code, piece.flags, piece.period),
        (11, 4_321, 1, 2)
    );
}

#[test]
fn a_file_that_does_not_add_up_is_refused() {
    let bytes = encode(&[a_box(0, 1)], &[a_flight(1)], &[a_piece(0)]);
    assert!(Aircraft::parse(&bytes[..bytes.len() - 1]).is_err());
    let mut wrong_magic = bytes.clone();
    wrong_magic[0] = b'x';
    assert!(Aircraft::parse(&wrong_magic).is_err());
    // A box naming a piece beyond the table, a piece naming a flight beyond it.
    let mut missing_piece = bytes.clone();
    missing_piece[24 + 110] = 7;
    assert!(Aircraft::parse(&missing_piece).is_err());
    let mut missing_flight = bytes;
    missing_flight[24 + 116 + 20] = 9;
    assert!(Aircraft::parse(&missing_flight).is_err());
}

#[test]
#[should_panic(expected = "names a missing piece")]
fn encoding_a_box_without_its_pieces_panics() {
    encode(&[a_box(0, 3)], &[a_flight(1)], &[a_piece(0)]);
}

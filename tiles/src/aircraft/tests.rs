//! The aircraft file round trip within its codes' steps, and its refusals.

use super::*;

/// A box at zoom 19 in cell (127, 3), heard in all three periods.
fn a_box(first_piece: u32, piece_count: u8) -> AircraftBox {
    let energy_db = std::array::from_fn(|period| {
        std::array::from_fn(|distance| 120.0 - 3.0 * distance as f64 - period as f64)
    });
    AircraftBox {
        place: BoxPlace {
            zoom: 19,
            cell: [127, 3],
            group: Group::Helicopter,
            ground_m: 301.0,
            clearance_m: 49.0,
            height_m: 49.0,
            centroid: [16_200, -15_500],
            first_piece,
            piece_count,
            loudest_lamax_db: if piece_count > 0 {
                91.26
            } else {
                f64::NEG_INFINITY
            },
        },
        centroid_altitude_m: 371.0,
        axis_rad: 2.5,
        gradient: -0.052,
        gradient_spread: 0.0412,
        piece_length_m: [44.0, 51.0, 38.0],
        flights_per_day: [338.2, 7.0 / 354.0, 61_000.0],
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
        36 + 5 + 3 * 4 + 8 + 2 * (1 + 27) + 6 * 18 + 2 * 19 + 2 * 17
    );
    let parsed = Aircraft::parse(&bytes).unwrap();
    assert_eq!((parsed.box_count(), parsed.piece_count()), (2, 2));
    let read = parsed.aircraft_box(0);
    let written = &boxes[0];
    assert_eq!(
        (read.place.zoom, read.place.cell, read.place.group),
        (19, [127, 3], Group::Helicopter)
    );
    assert_eq!(
        (
            read.place.ground_m,
            read.place.clearance_m,
            read.place.height_m
        ),
        (301.0, 49.0, 49.0)
    );
    // Zoom 19: a cell of 256 steps keeps every step of its centroid.
    assert_eq!(
        (read.place.centroid, read.centroid_altitude_m),
        ([16_200, -15_500], 371.0)
    );
    assert_eq!(parsed.box_centroid(0), read.place.centroid);
    assert!((read.axis_rad - 2.5).abs() < 1e-4);
    assert!((read.gradient - -0.052).abs() < 1e-4);
    assert_eq!(read.piece_length_m, [44.0, 51.0, 38.0]);
    for (r, w) in read.flights_per_day.iter().zip(written.flights_per_day) {
        assert!(close(*r, w, w * 2.5e-4), "{r} vs {w}");
    }
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
        assert!(close(*r, w, 0.00205), "{r} vs {w}");
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
    assert_eq!((read.place.first_piece, read.place.piece_count), (0, 2));
    // Rounded up, so a bound stays a bound.
    assert_eq!(read.place.loudest_lamax_db, 91.5);
    assert_eq!(parsed.flight(0), flights[0]);
    assert_eq!(parsed.flight(1), flights[1]);
    let piece = parsed.piece(&read.place, 0);
    assert_eq!(piece.flight, 1);
    // The finest scale: within a step.
    for (r, w) in piece
        .ends
        .iter()
        .flatten()
        .zip(a_piece(0).ends.iter().flatten())
    {
        assert!((i32::from(*r) - i32::from(*w)).abs() <= 1, "{r} vs {w}");
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
    quiet_evening.flights_per_day[1] = 0.0;
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
        (read.flights_per_day[1], read.piece_length_m[1]),
        (0.0, 0.0),
        "a silent period keeps no length"
    );
    assert_eq!(read.installation_shares[1], [[0.0, 0.0, 1.0]; 2]);
    // The next box finds its own periods and pieces after the left-out evening.
    let next = parsed.aircraft_box(1);
    assert_eq!((next.place.first_piece, next.place.piece_count), (1, 1));
    assert!(close(
        next.flights_per_day[1],
        loud.flights_per_day[1],
        loud.flights_per_day[1] * 2.5e-4
    ));
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
                    record.flights_per_day[period] = 0.0;
                }
            }
            record.flights_per_day[0] = f64::from(index);
            record
        })
        .scan(0u32, |first, mut record| {
            record.place.first_piece = *first;
            *first += u32::from(record.place.piece_count);
            Some(record)
        })
        .collect();
    let pieces = vec![a_piece(0); 35];
    let bytes = encode(&boxes, &[a_flight(1)], &pieces);
    let parsed = Aircraft::parse(&bytes).unwrap();
    for (index, written) in boxes.iter().enumerate() {
        let read = parsed.aircraft_box(index);
        assert_eq!(read.flights_per_day[0], index as f64, "whole rates exact");
        assert_eq!(
            (read.place.first_piece, read.place.piece_count),
            (written.place.first_piece, written.place.piece_count)
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
    let head = 36 + 5 + 3 * 4 + 8;
    let mut missing_piece = bytes.clone();
    missing_piece[head] = missing_piece[head] & 0x0f | 3 << 4;
    assert!(Aircraft::parse(&missing_piece).is_err());
    let mut high_head = bytes.clone();
    high_head[head] |= 1 << 7;
    assert!(Aircraft::parse(&high_head).is_err());
    // A box of a band the table lacks.
    let mut missing_band = bytes.clone();
    missing_band[head + 1] = 1;
    assert!(Aircraft::parse(&missing_band).is_err());
    // An index block counting pieces before the first box.
    let mut miscounted = bytes.clone();
    miscounted[36 + 5 + 3 * 4 + 4] = 1;
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

/// A piece beyond the coarsest scale keeps its ends exact in the wide table and reads back by its
/// index; a wide piece without its entry is refused.
#[test]
fn a_wide_piece_keeps_its_ends_exact() {
    let mut far = a_piece(0);
    far.ends = [[16_383, 16_383], [-16_384, -16_384]];
    let boxes = [a_box(0, 2), a_box(2, 1)];
    let pieces = [a_piece(0), far.clone(), far.clone()];
    let bytes = encode(&boxes, &[a_flight(1)], &pieces);
    let parsed = Aircraft::parse(&bytes).unwrap();
    let first = parsed.aircraft_box(0);
    assert_eq!(parsed.piece(&first.place, 1).ends, far.ends);
    assert!(parsed.box_reaches_past(0) && parsed.box_reaches_past(1));
    // a_piece ends 36 steps east of its cell and 56 m above its slab; one within both does not.
    let past = encode(&[a_box(0, 1)], &[a_flight(1)], &[a_piece(0)]);
    assert!(Aircraft::parse(&past).unwrap().box_reaches_past(0));
    let mut inside = a_piece(0);
    inside.ends = [[16_200, -15_500], [16_400, -15_380]];
    inside.altitudes_m = [345.0, 404.0];
    let near = encode(&[a_box(0, 1)], &[a_flight(1)], &[inside]);
    assert!(!Aircraft::parse(&near).unwrap().box_reaches_past(0));
    assert_eq!(parsed.piece(&parsed.box_place(1), 0).ends, far.ends);
    assert_ne!(parsed.piece(&first.place, 0).ends, far.ends);
    let mut lost = bytes.clone();
    lost[32] = 1;
    assert!(
        Aircraft::parse(&lost).is_err(),
        "a wide piece without its ends"
    );
}

/// A tile with more than 65,536 distinct installation mixes encodes with 32-bit mix indices and
/// round-trips every mix; a tile at or below 65,536 mixes keeps the exact 18-byte period layout.
#[test]
fn more_than_65536_distinct_mixes_round_trip_and_smaller_tiles_stay_byte_identical() {
    let share_at = |k: usize| -> [[f64; INSTALLATIONS]; 2] {
        // 128 * 128 = 16,384 valid (wing, fuselage) pairs per slant with wing + fuselage <= 255,
        // giving > 2.6e8 distinct four-byte mixes across the two slants.
        let near = k % 16_384;
        let far = k / 16_384;
        let pair = |code: usize| {
            let wing = (code % 128) as f64 / 255.0;
            let fuselage = (code / 128) as f64 / 255.0;
            [wing, fuselage, (1.0 - wing - fuselage).max(0.0)]
        };
        [pair(near), pair(far)]
    };

    // Today's size: byte-for-byte identical header, 18-byte period records and mix indices.
    let small = encode(&[a_box(0, 1)], &[a_flight(1)], &[a_piece(0)]);
    let small_period_offset = 36 + 5 + 3 * 4 + 8 + 1 + 27;
    assert_eq!(small.len(), small_period_offset + 3 * 18 + 19 + 17);
    assert_eq!(
        u16::from_le_bytes([
            small[small_period_offset + 16],
            small[small_period_offset + 17]
        ]),
        0
    );

    // Boundary: 65,536 distinct mixes still fits u16 (18 B per heard period).
    let at_bound: Vec<AircraftBox> = (0..21_846usize)
        .map(|box_index| {
            let mut record = a_box(0, 0);
            for period in 0..PERIODS {
                let mix_index = (box_index * PERIODS + period).min(65_535);
                record.installation_shares[period] = share_at(mix_index);
            }
            record
        })
        .collect();
    let at_bound_bytes = encode(&at_bound, &[], &[]);
    let at_bound_periods = at_bound.len() * PERIODS;
    let at_bound_blocks = at_bound.len().div_ceil(BOXES_PER_BLOCK);
    assert_eq!(
        at_bound_bytes.len(),
        36 + 5
            + 65_536 * 4
            + at_bound_blocks * 8
            + at_bound.len() * (1 + 27)
            + at_bound_periods * 18
    );
    let parsed_bound = Aircraft::parse(&at_bound_bytes).unwrap();
    assert_eq!(parsed_bound.box_count(), at_bound.len());

    // Overflow: 70,000 distinct mixes switches to u32 mix indices (20 B per heard period) and
    // preserves every mix within 1/255.
    let wide: Vec<AircraftBox> = (0..23_334usize)
        .map(|box_index| {
            let mut record = a_box(0, 0);
            for period in 0..PERIODS {
                let mix_index = (box_index * PERIODS + period).min(69_999);
                record.installation_shares[period] = share_at(mix_index);
            }
            record
        })
        .collect();
    let mut wide_bytes = encode(&wide, &[], &[]);
    let wide_periods = wide.len() * PERIODS;
    let wide_blocks = wide.len().div_ceil(BOXES_PER_BLOCK);
    let wide_period_offset = 36 + 5 + 70_000 * 4 + wide_blocks * 8 + wide.len() * (1 + 27);
    assert_eq!(wide_bytes.len(), wide_period_offset + wide_periods * 20);
    let parsed_wide = Aircraft::parse(&wide_bytes).unwrap();
    assert_eq!(parsed_wide.box_count(), wide.len());
    for index in [0, 1, 21_845, 21_846, wide.len() - 1] {
        let read = parsed_wide.aircraft_box(index);
        for (read_slant, written_slant) in read
            .installation_shares
            .iter()
            .flatten()
            .zip(wide[index].installation_shares.iter().flatten())
        {
            for (r, w) in read_slant.iter().zip(written_slant) {
                assert!(close(*r, *w, 1e-12), "{index}: {r} vs {w}");
            }
        }
    }
    // A wide period naming a mix past the table is refused.
    wide_bytes[wide_period_offset + 16..wide_period_offset + 20]
        .copy_from_slice(&70_000u32.to_le_bytes());
    assert!(Aircraft::parse(&wide_bytes).is_err());
}

//! The whole chain on synthetic flights over flat ground: segments into boxes, boxes into an
//! aircraft file, the file's boxes at receivers through the click-time equation, against the
//! kernel's exact sum of the segments.

use super::*;
use physics::doc29::box_geometry::BOX_EDGE_LEVEL_STEP_DB;
use physics::doc29::boxes::{AircraftBoxAtReceiver, box_sel_at_receiver};
use physics::doc29::screening::Unscreened;
use physics::doc29::segment::{SegmentGeometry, segment_sel_at_receiver};
use tiles::aircraft::Aircraft;
use tiles::geo::LocalFrame;

const PRAGUE: TileId = TileId { x: 2212, y: 1387 };

/// 30 departures climbing north through the tile's centre from 100 m to about 1,500 m over
/// 8 km, spread 60 m sideways, as 200 m segments.
fn departures() -> Vec<FlightSegment> {
    let frame = LocalFrame::at(PRAGUE.centre());
    let mut segments = Vec::new();
    for flight in 0..30u64 {
        let east = (flight as f64 * 17.0) % 60.0 - 30.0;
        for step in 0..40 {
            let (north0, north1) = (
                -4_000.0 + 200.0 * step as f64,
                -3_800.0 + 200.0 * step as f64,
            );
            let altitude = |north: f64| 100.0 + 0.17 * (north + 4_000.0);
            let point = |north: f64| {
                let (lat, lon) = frame.to_mercator([east, north]).to_degrees();
                [lat, lon, altitude(north)]
            };
            segments.push(FlightSegment {
                flight_id: (0x4b_0000 + flight) << 40 | 1_756_700_000,
                callsign: *b"CSA100  ",
                designator: *b"A320",
                source_id: 0,
                period: 0,
                flags: read::FLAG_DEPARTURE,
                start: point(north0),
                end: point(north1),
                pressure_altitude_m: [altitude(north0), altitude(north1)],
                speed_kt: 160.0,
                above_ground_m: 0.5 * (altitude(north0) + altitude(north1)),
                departure_field_m: 0.0,
                ground_m: [0.0, 0.0],
            });
        }
    }
    segments
}

/// The kernel's exact day SEL sum (dB) of the segments at a receiver 4 m above flat ground.
fn exact_db(segments: &[FlightSegment], receiver: [f64; 2]) -> f64 {
    let frame = LocalFrame::at(PRAGUE.centre());
    let local = |end: [f64; 3]| {
        let [east, north] = frame.to_metres(Mercator::from_degrees(end[0], end[1]));
        [east - receiver[0], north - receiver[1], end[2] - 4.0]
    };
    let energy: f64 = segments
        .iter()
        .filter_map(|segment| {
            let (_, emission) = emission_of(segment)?;
            let geometry = SegmentGeometry {
                start_m: local(segment.start),
                end_m: local(segment.end),
                ground_under_start_m: -4.0,
                ground_under_end_m: -4.0,
            };
            Some(segment_sel_at_receiver(&emission, &geometry, &Unscreened))
        })
        .map(|sel| 10f64.powf(sel.sel_db / 10.0))
        .sum();
    10.0 * energy.log10()
}

/// The boxes' day SEL sum (dB) at the receiver, read back from every written tile.
fn boxed_db(files: &[(TileId, Vec<u8>)], receiver: [f64; 2]) -> f64 {
    let frame = LocalFrame::at(PRAGUE.centre());
    let mut energy = 0.0;
    for (tile, bytes) in files {
        let aircraft = Aircraft::parse(bytes).unwrap();
        for index in 0..aircraft.box_count() {
            let record = aircraft.aircraft_box(index);
            let global = tile.global(record.centroid);
            let [east, north] = frame.metres_of_steps([global.x as f64, global.y as f64]);
            let at_receiver = AircraftBoxAtReceiver {
                centroid_m: [
                    east - receiver[0],
                    north - receiver[1],
                    record.centroid_altitude_m - 4.0,
                ],
                axis_rad: record.axis_rad,
                gradient: record.gradient,
                piece_length_m: record.piece_length_m,
                levels_db: &record.energy_db,
                tail_levels_db: &record.tail_energy_db,
                lg_scaled_distance: &record.lg_scaled_distance,
                installation_shares: record.installation_shares,
                ground_m: record.ground_m - 4.0,
            };
            energy += 10f64.powf(box_sel_at_receiver(&at_receiver, &Unscreened).sel_db[0] / 10.0);
        }
    }
    10.0 * energy.log10()
}

#[test]
fn boxes_of_a_departure_corridor_read_as_its_segments() {
    let segments = departures();
    let terrain = HashMap::new();
    let scope: HashSet<TileId> = [0, 1].iter().flat_map(|&ring| PRAGUE.ring(ring)).collect();
    let placement = Placement::new(&scope, &terrain, BOX_EDGE_LEVEL_STEP_DB);
    let mut boxes = Boxes::default();
    let weighted: Vec<(FlightSegment, f64)> = segments
        .iter()
        .map(|segment| (segment.clone(), 1.0))
        .collect();
    add_day(&mut boxes, &weighted, &placement, &scope, PIECES_PER_BOX);
    let out = std::env::temp_dir().join(format!("qm-boxes-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    assert!(
        write::write_tiles(&boxes, &placement, tiles::Kind::Aircraft, &out).unwrap() >= 2,
        "the corridor crosses tiles"
    );
    let files: Vec<(TileId, Vec<u8>)> = scope
        .iter()
        .filter_map(|&tile| {
            std::fs::read(tiles::tile_path(&out, tile, tiles::Kind::Aircraft))
                .ok()
                .map(|bytes| (tile, bytes))
        })
        .collect();
    let bytes = std::fs::read(tiles::tile_path(&out, PRAGUE, tiles::Kind::Aircraft)).unwrap();
    let aircraft = Aircraft::parse(&bytes).unwrap();
    assert!(aircraft.box_count() > 50, "{} boxes", aircraft.box_count());
    // A box counts each flight crossing it once: at most all 30 flights a day.
    let most = (0..aircraft.box_count())
        .map(|index| aircraft.aircraft_box(index).flights)
        .max()
        .unwrap();
    assert!(
        most > 365 && most <= (30.0 * 365.25f64).round() as u32,
        "{most}"
    );
    assert!(aircraft.piece_count() > 0 && aircraft.flight_count() > 0);
    for (receiver, tolerance) in [
        ([0.0, 0.0], 0.5),
        ([1_500.0, 0.0], 0.3),
        ([-3_000.0, 2_000.0], 0.3),
        ([2_000.0, -4_200.0], 0.3),
    ] {
        let (exact, boxed) = (exact_db(&segments, receiver), boxed_db(&files, receiver));
        eprintln!("corridor at {receiver:?}: box {boxed:.2} vs segments {exact:.2}");
        assert!(
            (boxed - exact).abs() < tolerance,
            "at {receiver:?}: box {boxed:.3} vs segments {exact:.3}"
        );
    }
    // Behind the corridor's start, where every extended climb runs under the ground, the climbs
    // are still heard (Doc 29 keeps them; dev4's Filter D dropped them) and the boxes follow.
    let behind = [500.0, -5_000.0];
    let (exact, boxed) = (exact_db(&segments, behind), boxed_db(&files, behind));
    eprintln!("corridor behind its start: box {boxed:.2} vs segments {exact:.2}");
    assert!(
        exact.is_finite() && (boxed - exact).abs() < 0.5,
        "{boxed:.3} vs {exact:.3}"
    );
    std::fs::remove_dir_all(&out).unwrap();
}

#[test]
fn a_box_keeps_its_loudest_pieces_one_per_flight() {
    let piece = |flight: u64, level: f64| KeptPiece {
        flight_id: flight,
        callsign: *b"CSA100  ",
        designator: *b"A320",
        flags: 0,
        period: 0,
        speed_kt: 160.0,
        class: 0,
        power_code: 0,
        lamax_reference_db: 80.0,
        keep_level_db: level,
        start: (PRAGUE.centre(), 300.0),
        end: (PRAGUE.centre(), 300.0),
    };
    let mut entry = BoxEntry::default();
    for (flight, level) in [
        (1, 80.0),
        (2, 85.0),
        (1, 83.0),
        (3, 82.0),
        (2, 84.0),
        (4, 79.0),
    ] {
        entry.keep(piece(flight, level), 2);
    }
    // Flight 2's louder piece and flight 1's; flight 3 falls out at 82 dB.
    let kept: Vec<(u64, f64)> = entry
        .kept
        .iter()
        .map(|kept| (kept.flight_id, kept.keep_level_db))
        .collect();
    assert_eq!(kept, [(1, 83.0), (2, 85.0)]);
}

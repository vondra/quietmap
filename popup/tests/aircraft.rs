//! Aircraft in a synthetic release: segment energy, far boxes, reach and complete terrain screening.

use physics::bands::{PERIOD_HOURS, PERIODS};
use physics::doc29::box_sums::{BoxSums, SegmentEnergies};
use physics::doc29::profiles_generated::profile_idx;
use physics::doc29::screening::Unscreened;
use physics::doc29::segment::{
    AircraftType, SegmentEmission, SegmentGeometry, segment_sel_at_receiver,
};
use physics::doc29::thrust::SegmentFlight;
use physics::weather::{COLUMNS, ROWS, SECTORS, WeatherNode, encode as encode_weather};
use popup::answer::{Options, answer};
use popup::release::Release;
use std::path::PathBuf;
use tiles::Kind;
use tiles::aircraft::{AircraftBox, BoxPlace, Group, encode};
use tiles::geo::{LocalFrame, TileId};
use tiles::sources::Layer;
use tiles::square::write_tiles;

const TILE: TileId = TileId { x: 2212, y: 1387 };
/// The receiver stands 4 m above the ocean.
const RECEIVER_ALTITUDE_M: f64 = 4.0;

fn release_with_boxes(name: &str, tile: TileId, kind: Kind, boxes: &[AircraftBox]) -> PathBuf {
    release_with_files(name, &[(tile, kind, boxes)])
}

/// A release of `files`: per tile and kind its boxes.
fn release_with_files(name: &str, files: &[(TileId, Kind, &[AircraftBox])]) -> PathBuf {
    let root = std::env::temp_dir().join(format!("qm-aircraft-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("2026")).unwrap();
    let weather = vec![
        WeatherNode {
            percent: [[50u8; SECTORS]; PERIODS],
            ..WeatherNode::default()
        };
        ROWS * COLUMNS
    ];
    std::fs::write(root.join("2026").join("weather"), encode_weather(&weather)).unwrap();
    for &(tile, kind, boxes) in files {
        write_tiles(&root.join("2026"), kind, &[(tile, encode(boxes, &[], &[]))]).unwrap();
    }
    tiles::mark_year_complete(&root.join("2026"), "test").unwrap();
    root
}

/// An A320 departure piece 300 m long at 600 m, `east_m` east of the tile centre.
fn departure(east_m: f64) -> (SegmentEmission, [f64; 3], [f64; 3]) {
    let flight = SegmentFlight {
        departure: true,
        on_ground: false,
        speed_kt: 160.0,
        pressure_altitude_m: 600.0,
        climb_sine: 0.08,
        acceleration_ms2: None,
        height_above_field_m: 600.0,
    };
    let emission = SegmentEmission::new(
        &AircraftType::of(profile_idx("A320"), "A320"),
        &flight,
        false,
    )
    .unwrap();
    (emission, [east_m, -150.0, 588.0], [east_m, 150.0, 612.0])
}

/// The box of one piece, its centroid in the tile's steps.
fn box_of(emission: &SegmentEmission, start: [f64; 3], end: [f64; 3]) -> AircraftBox {
    box_of_flights(&[(*emission, start, end)])
}

/// The box of one flight a year along each of `pieces` (emission, start, end).
fn box_of_flights(pieces: &[(SegmentEmission, [f64; 3], [f64; 3])]) -> AircraftBox {
    let mut sums = BoxSums::default();
    for (emission, start, end) in pieces {
        sums.add(
            &SegmentEnergies::of(&emission.npd_distance_levels()),
            emission.installation,
            [1.0, 0.0, 0.0],
            *start,
            *end,
        );
    }
    let values = sums.values().unwrap();
    let steps_per_metre = [
        32_768.0 / LocalFrame::at(TILE.centre()).east_m_per_unit,
        32_768.0 / LocalFrame::at(TILE.centre()).north_m_per_unit,
    ];
    let centroid = [
        (values.centroid_m[0] * steps_per_metre[0]).round() as i16,
        (-values.centroid_m[1] * steps_per_metre[1]).round() as i16,
    ];
    // The zoom-18 cell (512 steps) holding the centroid.
    let cell = centroid.map(|c| ((i32::from(c) + 16_384) / 512) as u16);
    AircraftBox {
        place: BoxPlace {
            zoom: 18,
            cell,
            group: Group::FixedWing,
            ground_m: 0.0,
            clearance_m: 588.0,
            height_m: 98.0,
            centroid,
            first_piece: 0,
            piece_count: 0,
            loudest_lamax_db: f64::NEG_INFINITY,
        },
        centroid_altitude_m: values.centroid_m[2],
        axis_rad: values.axis_rad,
        gradient: values.gradient,
        gradient_spread: values.gradient_spread,
        piece_length_m: values.piece_length_m,
        flights_per_day: [pieces.len() as f64 / 365.25, 0.0, 0.0],
        energy_db: values.levels_db,
        tail_energy_db: values.tail_levels_db,
        lg_scaled_distance: values.scaled_distance_m.map(f64::log10),
        installation_shares: values.installation_shares,
    }
}

/// The aircraft layer's day energy of a click at the tile centre.
fn aircraft_day_energy(release: &Release) -> (f64, usize) {
    let (energy, boxes, _, _) = aircraft_answer(release);
    (energy, boxes)
}

/// The aircraft layer's day energy, boxes heard and own Nden (sone) of a click at the tile centre,
/// and the payloads it read.
fn aircraft_answer(release: &Release) -> (f64, usize, f64, usize) {
    let (lat, lon) = TILE.centre().to_degrees();
    let mut last = None;
    answer(
        release,
        lat,
        lon,
        &Options {
            exact: false,
            source: Vec::new(),
            piece: None,
        },
        &mut |update| {
            let layer = update
                .layers
                .iter()
                .find(|layer| layer.layer == Layer::Aircraft)
                .unwrap();
            last = Some((
                layer.energy[0],
                layer.evaluated,
                layer.nden_sone.unwrap_or(f64::NAN),
                update.statistics.reads,
            ));
            Ok(())
        },
    )
    .unwrap();
    last.unwrap()
}

#[test]
fn a_box_of_one_segment_answers_as_the_kernel_reads_it() {
    let (emission, start, end) = departure(1_000.0);
    let root = release_with_boxes(
        "one",
        TILE,
        Kind::Aircraft,
        &[box_of(&emission, start, end)],
    );
    let release = Release::open(&root, "2026").unwrap();
    let (energy, boxes) = aircraft_day_energy(&release);
    assert_eq!(boxes, 1);
    let relative = |p: [f64; 3]| [p[0], p[1], p[2] - RECEIVER_ALTITUDE_M];
    let geometry = SegmentGeometry {
        start_m: relative(start),
        end_m: relative(end),
        ground_under_start_m: -RECEIVER_ALTITUDE_M,
        ground_under_end_m: -RECEIVER_ALTITUDE_M,
    };
    let sel = segment_sel_at_receiver(&emission, &geometry, &Unscreened).sel_db;
    let expected_leq = sel - 10.0 * (PERIOD_HOURS[0] * 3_600.0).log10();
    let got_leq = 10.0 * energy.log10();
    // Metre altitudes and step-rounded centroids shift the level by far less than 0.05 dB.
    assert!(
        (got_leq - expected_leq).abs() < 0.05,
        "popup {got_leq:.3} vs kernel {expected_leq:.3}"
    );
    std::fs::remove_dir_all(&root).unwrap();
}

/// Two tiles east (about 12.6 km in Prague) the far file is heard and the fine one is not read.
#[test]
fn a_tile_beyond_3_km_reads_the_far_boxes() {
    let (emission, start, end) = departure(0.0);
    let second_ring = TileId {
        x: TILE.x + 2,
        y: TILE.y,
    };
    for (kind, heard) in [(Kind::AircraftFar, 1), (Kind::Aircraft, 0)] {
        let root = release_with_boxes(
            "second",
            second_ring,
            kind,
            &[box_of(&emission, start, end)],
        );
        let release = Release::open(&root, "2026").unwrap();
        let (energy, boxes) = aircraft_day_energy(&release);
        assert_eq!((boxes, energy > 0.0), (heard, heard == 1), "{kind:?}");
        std::fs::remove_dir_all(&root).unwrap();
    }
}

/// A box three tiles east (about 19 km in Prague) is read, the ring loop reading aircraft to
/// 16 km, and not heard.
#[test]
fn a_box_beyond_the_reach_is_not_heard() {
    let (emission, start, end) = departure(0.0);
    let far_tile = TileId {
        x: TILE.x + 3,
        y: TILE.y,
    };
    let root = release_with_boxes(
        "far",
        far_tile,
        Kind::AircraftFar,
        &[box_of(&emission, start, end)],
    );
    let release = Release::open(&root, "2026").unwrap();
    let (energy, boxes) = aircraft_day_energy(&release);
    assert_eq!((energy, boxes), (0.0, 0));
    std::fs::remove_dir_all(&root).unwrap();
}

/// A box's flights are counted per period (Codex, review of the r054 plan): two flights a day by
/// day and two at night, the night 10 dB quieter, give the night its own rate, 1.5 times the day's
/// per hour (12 h against 8 h), where splitting the four by the SEL energy gave the night a tenth.
#[test]
fn each_period_counts_its_own_flights() {
    use popup::aircraft::boxes::{AircraftReceiver, tile_energy};
    let (emission, start, end) = departure(1_000.0);
    let mut record = box_of(&emission, start, end);
    // The night flies the day's pieces 10 dB quieter.
    record.energy_db[2] = record.energy_db[0].map(|level| level - 10.0);
    record.tail_energy_db[2] = record.tail_energy_db[0] - 10.0;
    record.piece_length_m[2] = record.piece_length_m[0];
    record.installation_shares[2] = record.installation_shares[0];
    record.flights_per_day = [731.0 / 365.25, 0.0, 731.0 / 365.25];
    let bytes = encode(&[record], &[], &[]);
    let aircraft = tiles::aircraft::Aircraft::parse(&bytes).unwrap();
    let receiver = AircraftReceiver {
        position: [0.0, 0.0],
        altitude_m: RECEIVER_ALTITUDE_M,
    };
    let heard = tile_energy(
        &aircraft,
        TILE,
        &LocalFrame::at(TILE.centre()),
        receiver,
        &Unscreened,
    );
    let passing = |period: usize| heard.passes.passing[period].iter().sum::<f64>();
    let ratio = passing(2) / passing(0);
    assert!(
        (ratio - PERIOD_HOURS[0] / PERIOD_HOURS[2]).abs() < 1e-9,
        "{ratio}"
    );
}

/// A first-ring flight is screened by a ridge in the next terrain ring: the horizon's sector
/// centre falls across the tile edge. Both streamed answers must equal the complete scene.
#[test]
fn every_flight_ring_uses_the_complete_terrain_horizon() {
    use popup::aircraft::boxes::{AircraftReceiver, tile_energy};
    use popup::aircraft::horizons::Horizons;
    use popup::obstacles::Scene;
    use popup::scene::Ground;
    use tiles::aircraft::Aircraft;
    use tiles::terrain::{Node, Terrain, Window, code_of_height_m};

    let tile_frame = LocalFrame::at(TILE.centre());
    let receiver_position = tile_frame.to_mercator([0.3 * tile_frame.east_m_per_unit, 0.0]);
    let frame = LocalFrame::at(receiver_position);
    let aircraft_tile = TileId {
        x: TILE.x + 1,
        ..TILE
    };
    let ridge_tile = TileId {
        x: TILE.x + 2,
        ..TILE
    };
    let (emission, _, _) = departure(0.0);
    let east = 0.49 * tile_frame.east_m_per_unit;
    let north = 0.49 * tile_frame.north_m_per_unit;
    let record = box_of(
        &emission,
        [east - 2.0, north + 5.0, 600.0],
        [east + 2.0, north - 5.0, 600.0],
    );
    let root = release_with_boxes("horizon", aircraft_tile, Kind::Aircraft, &[record]);
    let window = Window::of_tile(ridge_tile);
    let nodes = vec![
        Node {
            height_code: code_of_height_m(1_000.0),
            impervious_percent: 0,
            forest_percent: 0,
            canopy_m: 0,
        };
        window.node_count()
    ];
    let ridge = tiles::terrain::encode(window, &nodes);
    write_tiles(
        &root.join("2026"),
        Kind::Terrain,
        &[(ridge_tile, ridge.clone())],
    )
    .unwrap();

    let mut ground = Ground::new(frame, TILE, 3, None);
    for ring in 0..=3 {
        for tile in TILE.ring(ring) {
            ground.insert(
                tile,
                (tile == ridge_tile).then(|| Terrain::parse(&ridge).unwrap()),
            );
        }
    }
    let mut obstacles = Scene::new(frame);
    obstacles.insert_all(
        (0..=3)
            .flat_map(|ring| TILE.ring(ring))
            .map(|tile| (tile, None))
            .collect(),
    );
    let horizons = Horizons::build(&ground, &obstacles, [0.0; 2], RECEIVER_ALTITUDE_M, 0).unwrap();
    let receiver = AircraftReceiver {
        position: [0.0; 2],
        altitude_m: RECEIVER_ALTITUDE_M,
    };
    let bytes = tiles::square::read_tile(&root.join("2026"), aircraft_tile, Kind::Aircraft)
        .unwrap()
        .unwrap();
    let aircraft = Aircraft::parse(&bytes).unwrap();
    let expected = tile_energy(&aircraft, aircraft_tile, &frame, receiver, &horizons).energy[0];
    let unscreened = tile_energy(&aircraft, aircraft_tile, &frame, receiver, &Unscreened).energy[0];
    assert!(
        10.0 * (unscreened / expected).log10() > 1.0,
        "the far ridge must screen the flight"
    );

    let release = Release::open(&root, "2026").unwrap();
    let (lat, lon) = receiver_position.to_degrees();
    let mut updates = 0;
    answer(
        &release,
        lat,
        lon,
        &Options {
            exact: false,
            source: Vec::new(),
            piece: None,
        },
        &mut |update| {
            let heard = update
                .layers
                .iter()
                .find(|layer| layer.layer == Layer::Aircraft)
                .unwrap();
            assert!((10.0 * (heard.energy[0] / expected).log10()).abs() < 1e-6);
            assert_eq!(
                update.statistics.reads, 3,
                "weather, aircraft, terrain, each read once"
            );
            updates += 1;
            Ok(())
        },
    )
    .unwrap();
    assert!(updates > 1);
    std::fs::remove_dir_all(root).unwrap();
}

/// A box's flights are in it for its cell's and slab's mean chord along their axis over their
/// speed: twenty departures a day northward up a gradient of 0.08 through a zoom-18 cell and its
/// 98 m slab at their kept pieces' 320 and 160 kt (the harmonic mean, 213 kt), at 135 kt where the
/// box keeps none, at the NPD curves' 160 kt where its piece hovers; the passes carry all the
/// box's energy.
#[test]
fn a_boxs_flights_pass_through_it_at_their_kept_speed() {
    use popup::aircraft::boxes::{AircraftReceiver, tile_energy};
    use tiles::aircraft::{Aircraft, Flight, FlightPiece};
    let (emission, start, end) = departure(1_000.0);
    let record = AircraftBox {
        flights_per_day: [20.0, 0.0, 0.0],
        ..box_of(&emission, start, end)
    };
    let frame = LocalFrame::at(TILE.centre());
    let receiver = AircraftReceiver {
        position: [0.0, 0.0],
        altitude_m: RECEIVER_ALTITUDE_M,
    };
    let flight = Flight {
        icao: 0x49_d0_a1,
        callsign: *b"CSA123  ",
        type_designator: *b"A320",
        start_unix: 1_756_684_800,
    };
    let passing = |speeds_kt: &[f64]| {
        let pieces: Vec<FlightPiece> = (speeds_kt.iter())
            .map(|&speed_kt| FlightPiece {
                flight: 0,
                ends: [record.place.centroid; 2],
                altitudes_m: [588.0, 612.0],
                speed_kt,
                class: emission.class as u16,
                power_code: 0,
                flags: 1,
                period: 0,
            })
            .collect();
        let mut kept = record.clone();
        kept.place.piece_count = pieces.len() as u8;
        kept.place.loudest_lamax_db = if pieces.is_empty() {
            f64::NEG_INFINITY
        } else {
            80.0
        };
        let flights = if pieces.is_empty() {
            vec![]
        } else {
            vec![flight.clone()]
        };
        let bytes = encode(&[kept], &flights, &pieces);
        let aircraft = Aircraft::parse(&bytes).unwrap();
        let heard = tile_energy(&aircraft, TILE, &frame, receiver, &Unscreened);
        let classes: f64 = heard.passes.energy[0].iter().sum();
        assert!((classes / heard.energy[0] - 1.0).abs() < 1e-12);
        assert_eq!(heard.passes.passing[1].iter().sum::<f64>(), 0.0);
        heard.passes.passing[0].iter().sum::<f64>()
    };
    let (east_m, north_m) = (frame.east_m_per_unit / 64.0, frame.north_m_per_unit / 64.0);
    // Northward (its axis a quarter turn from east): the cell's north edge, shortened by the
    // climb through the slab's top.
    let chord_m =
        east_m * north_m * 98.0 / (east_m * 98.0 + record.gradient.abs() * east_m * north_m);
    assert!(
        (record.axis_rad - std::f64::consts::FRAC_PI_2).abs() < 1e-6
            && (chord_m - 91.0).abs() < 2.0
    );
    let expected = |speed_kt: f64| 20.0 / 43_200.0 * chord_m / (speed_kt * 1_852.0 / 3_600.0);
    for (speeds_kt, speed_kt) in [
        (&[320.0, 160.0][..], 2.0 / (1.0 / 320.0 + 1.0 / 160.0)),
        (&[][..], 70.0 * 3_600.0 / 1_852.0),
        (&[5.0][..], 160.0),
    ] {
        let got = passing(speeds_kt);
        assert!(
            (got / expected(speed_kt) - 1.0).abs() < 1e-3,
            "{speeds_kt:?}: {got}"
        );
    }
}

/// A click hears the flights as the passes of the boxes it hears: two departures a day 100 m
/// apart, a fine box each. Their tile's far file is not read (ten decibels louder in it, the
/// answer and its reads stay: the far boxes' lambda took it). Ten times the flights at the same
/// energy, each a tenth as loud, are in progress ten times as long and sound louder in sone.
#[test]
fn a_click_hears_the_flights_as_the_passes_of_its_boxes() {
    let [west, east] = [-50.0, 50.0].map(departure);
    let daily = |pieces: &[(SegmentEmission, [f64; 3], [f64; 3])], flights: f64| AircraftBox {
        flights_per_day: [flights, 0.0, 0.0],
        ..box_of_flights(pieces)
    };
    let answer = |name: &str, flights: f64, far: bool| {
        let fine = [daily(&[west], flights), daily(&[east], flights)];
        let mut louder = daily(&[west, east], 2.0 * flights);
        louder.energy_db = louder
            .energy_db
            .map(|levels| levels.map(|level| level + 10.0));
        louder.tail_energy_db = louder.tail_energy_db.map(|level| level + 10.0);
        let louder = [louder];
        let mut files: Vec<(TileId, Kind, &[AircraftBox])> = vec![(TILE, Kind::Aircraft, &fine)];
        if far {
            files.push((TILE, Kind::AircraftFar, &louder));
        }
        let root = release_with_files(name, &files);
        let heard = aircraft_answer(&Release::open(&root, "2026").unwrap());
        std::fs::remove_dir_all(&root).unwrap();
        heard
    };
    let (own, with_far, often) = (
        answer("passes-own", 1.0, false),
        answer("passes-far", 1.0, true),
        answer("passes-often", 10.0, false),
    );
    assert_eq!(own, with_far);
    assert_eq!((own.0, own.1), (often.0, often.1));
    assert!(often.2 > 1.5 * own.2, "{} sone vs {}", often.2, own.2);
}

//! Airport ground operations in the ring loop on a synthetic release (flat ocean): their line
//! pieces join the aircraft layer next to the boxes as one contributor per airport, and the boxes'
//! energy counts in the layer's omitted-energy account.

use physics::bands::{BANDS, PERIODS};
use physics::doc29::box_sums::BoxSums;
use physics::doc29::segment::{AircraftType, SegmentEmission};
use physics::doc29::thrust::SegmentFlight;
use physics::weather::{COLUMNS, ROWS, SECTORS, encode as encode_weather};
use popup::answer::{Options, answer};
use popup::json::update_line;
use popup::release::Release;
use std::path::PathBuf;
use tiles::aircraft::{AircraftBox, Group, encode as encode_aircraft};
use tiles::geo::{LocalFrame, TileId};
use tiles::sources::{Attribute, Layer, Piece, encode as encode_sources};
use tiles::{COMPLETION_MARKER, Kind, tile_path};

const TILE: TileId = TileId { x: 2212, y: 1387 };

fn steps_per_metre() -> [f64; 2] {
    let frame = LocalFrame::at(TILE.centre());
    [
        32_768.0 / frame.east_m_per_unit,
        32_768.0 / frame.north_m_per_unit,
    ]
}

/// One box of a hundred A320 departures a day, a piece 300 m long at 600 m, 1 km east of the tile
/// centre.
fn departure_box() -> AircraftBox {
    let flight = SegmentFlight {
        departure: true,
        on_ground: false,
        speed_kt: 160.0,
        pressure_altitude_m: 600.0,
        climb_sine: 0.08,
        acceleration_ms2: 0.0,
        height_above_field_m: 600.0,
    };
    let emission =
        SegmentEmission::new(&AircraftType::from_designator("A320"), &flight, false).unwrap();
    let mut sums = BoxSums::default();
    sums.add(
        &emission.npd_distance_levels(),
        emission.installation,
        [100.0, 100.0, 100.0],
        [1_000.0, -150.0, 588.0],
        [1_000.0, 150.0, 612.0],
    );
    let values = sums.values().unwrap();
    let steps = steps_per_metre();
    AircraftBox {
        zoom: 18,
        cell: [10, 10],
        group: Group::FixedWing,
        ground_m: 0.0,
        clearance_m: 588.0,
        height_m: 98.0,
        centroid: [
            (values.centroid_m[0] * steps[0]).round() as i16,
            (-values.centroid_m[1] * steps[1]).round() as i16,
        ],
        centroid_altitude_m: values.centroid_m[2],
        axis_rad: values.axis_rad,
        gradient: values.gradient,
        gradient_spread: values.gradient_spread,
        piece_length_m: values.piece_length_m,
        flights: 1,
        energy_db: values.levels_db,
        tail_energy_db: values.tail_levels_db,
        lg_scaled_distance: values.scaled_distance_m.map(f64::log10),
        installation_shares: values.installation_shares,
        first_piece: 0,
        piece_count: 0,
        loudest_lamax_db: f64::NEG_INFINITY,
    }
}

/// A release with airport pieces (250 m, north-south, `east_m` east of the tile centre, emitting
/// `level_db` per metre in every band) and, if asked, the box.
fn release(name: &str, east_m: &[f64], level_db: f64, with_box: bool) -> PathBuf {
    let root = std::env::temp_dir().join(format!("qm-airport-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("2026")).unwrap();
    let weather = vec![[[50u8; SECTORS]; PERIODS]; ROWS * COLUMNS];
    std::fs::write(root.join("weather"), encode_weather(&weather)).unwrap();
    let write = |kind, bytes: Vec<u8>| {
        let path = tile_path(&root.join("2026"), TILE, kind);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    };
    let steps = steps_per_metre();
    let half = (125.0 * steps[1]).round() as i16;
    let pieces: Vec<Piece> = east_m
        .iter()
        .map(|east| {
            let x = (east * steps[0]).round() as i16;
            Piece {
                ends: [[x, -half], [x, half]],
                attribute: 0,
            }
        })
        .collect();
    let attribute = Attribute {
        layer: Layer::Aircraft,
        height_m: 4.0,
        ground_percent: 0,
        platform_half_width_m: 22.5,
        exclusion_radius_m: 0.0,
        footprint_id: 0,
        group_key: 99,
        emission: [[level_db; BANDS]; PERIODS],
        display: r#"["TEST ground operations","airport_traffic:TEST","Test Airport",1.0,2.0,3.0]"#
            .into(),
    };
    write(Kind::Sources, encode_sources(&pieces, &[attribute]));
    if with_box {
        write(
            Kind::Aircraft,
            encode_aircraft(&[departure_box()], &[], &[]),
        );
    }
    std::fs::write(root.join("2026").join(COMPLETION_MARKER), "test").unwrap();
    root
}

/// The aircraft layer of a click at the tile centre: per-period energy, the sources and boxes
/// evaluated, and the last update as JSON.
fn aircraft_answer(root: &PathBuf, exact: bool) -> ([f64; PERIODS], usize, serde_json::Value) {
    let release = Release::open(root, "2026").unwrap();
    let (lat, lon) = TILE.centre().to_degrees();
    let mut last = None;
    answer(
        &release,
        lat,
        lon,
        &Options { exact, pieces: 0 },
        &mut |update| {
            let layer = update
                .layers
                .iter()
                .find(|layer| layer.layer == Layer::Aircraft)
                .unwrap();
            let line = update_line(update, 1)?;
            last = Some((layer.energy, layer.evaluated, line));
            Ok(())
        },
    )
    .unwrap();
    std::fs::remove_dir_all(root).unwrap();
    let (energy, evaluated, line) = last.unwrap();
    (energy, evaluated, serde_json::from_str(&line).unwrap())
}

#[test]
fn airport_pieces_join_the_boxes_in_the_aircraft_layer() {
    let (boxed, _, _) = aircraft_answer(&release("box", &[], 0.0, true), true);
    let (ground, _, _) = aircraft_answer(&release("ground", &[300.0], 90.0, false), true);
    let (both, evaluated, json) = aircraft_answer(&release("both", &[300.0], 90.0, true), true);
    assert_eq!(evaluated, 2, "one piece and one box");
    for period in 0..PERIODS {
        assert!(boxed[period] > 0.0 && ground[period] > 0.0);
        let sum = boxed[period] + ground[period];
        assert!((both[period] / sum - 1.0).abs() < 1e-9, "period {period}");
    }
    let contributors = json["top_contributors"].as_array().unwrap();
    assert_eq!(
        contributors.len(),
        1,
        "the boxes list flights, not contributors"
    );
    assert_eq!(contributors[0]["source_type"], "aircraft");
    assert_eq!(contributors[0]["name"], "TEST ground operations");
    assert_eq!(contributors[0]["subtype"], "airport_traffic:TEST");
    assert_eq!(contributors[0]["metadata"]["departures_per_day"], 2.0);
}

/// Faint airport pieces from 200 m to 10 km beside departures overhead (about 40 dB louder): the
/// box's energy lets the stop rule leave out more of them, and the fast answer stays within 0.1 dB
/// of the exact one.
#[test]
fn the_boxes_count_in_the_stop_rule_of_airport_pieces() {
    let east: Vec<f64> = (0..40)
        .map(|i| 200.0 * 1.1f64.powi(i))
        .filter(|east| *east < 10_000.0)
        .collect();
    let evaluated = |with_box: bool| {
        let name = if with_box { "boxed" } else { "alone" };
        let (exact, all, _) = aircraft_answer(&release(name, &east, 40.0, with_box), true);
        let (fast, some, _) = aircraft_answer(&release(name, &east, 40.0, with_box), false);
        assert_eq!(all, east.len() + usize::from(with_box));
        for period in 0..PERIODS {
            let difference = 10.0 * (exact[period] / fast[period]).log10();
            assert!(
                (0.0..=0.1).contains(&difference),
                "{name} {period}: {difference}"
            );
        }
        some - usize::from(with_box)
    };
    let (alone, boxed) = (evaluated(false), evaluated(true));
    assert!(
        boxed < alone,
        "{boxed} pieces with the box, {alone} without"
    );
}

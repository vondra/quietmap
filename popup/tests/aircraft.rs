//! The aircraft layer of the ring loop on a synthetic release (flat ocean): a box of one segment
//! answers as the kernel's SEL of that segment, as the day's Leq; for tiles beyond 3 km the far
//! boxes are read; a box beyond the reach is not heard.

use physics::bands::{PERIOD_HOURS, PERIODS};
use physics::doc29::box_sums::BoxSums;
use physics::doc29::screening::Unscreened;
use physics::doc29::segment::{
    AircraftType, SegmentEmission, SegmentGeometry, segment_sel_at_receiver,
};
use physics::doc29::thrust::SegmentFlight;
use physics::weather::{COLUMNS, ROWS, SECTORS, WeatherNode, encode as encode_weather};
use popup::answer::{Options, answer};
use popup::release::Release;
use std::path::PathBuf;
use tiles::aircraft::{AircraftBox, Group, encode};
use tiles::geo::{LocalFrame, TileId};
use tiles::sources::Layer;
use tiles::{COMPLETION_MARKER, Kind, tile_path};

const TILE: TileId = TileId { x: 2212, y: 1387 };
/// The receiver stands 4 m above the ocean.
const RECEIVER_ALTITUDE_M: f64 = 4.0;

fn release_with_boxes(name: &str, tile: TileId, kind: Kind, boxes: &[AircraftBox]) -> PathBuf {
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
    std::fs::write(root.join("weather"), encode_weather(&weather)).unwrap();
    let path = tile_path(&root.join("2026"), tile, kind);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, encode(boxes, &[], &[])).unwrap();
    std::fs::write(root.join("2026").join(COMPLETION_MARKER), "test").unwrap();
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
    let emission =
        SegmentEmission::new(&AircraftType::from_designator("A320"), &flight, false).unwrap();
    (emission, [east_m, -150.0, 588.0], [east_m, 150.0, 612.0])
}

/// The box of one piece, its centroid in the tile's steps.
fn box_of(emission: &SegmentEmission, start: [f64; 3], end: [f64; 3]) -> AircraftBox {
    let mut sums = BoxSums::default();
    sums.add(
        &emission.npd_distance_levels(),
        emission.installation,
        [1.0, 0.0, 0.0],
        start,
        end,
    );
    let values = sums.values().unwrap();
    let steps_per_metre = [
        32_768.0 / LocalFrame::at(TILE.centre()).east_m_per_unit,
        32_768.0 / LocalFrame::at(TILE.centre()).north_m_per_unit,
    ];
    AircraftBox {
        zoom: 18,
        cell: [10, 10],
        group: Group::FixedWing,
        ground_m: 0.0,
        clearance_m: 588.0,
        height_m: 98.0,
        centroid: [
            (values.centroid_m[0] * steps_per_metre[0]).round() as i16,
            (-values.centroid_m[1] * steps_per_metre[1]).round() as i16,
        ],
        centroid_altitude_m: values.centroid_m[2],
        axis_rad: values.axis_rad,
        gradient: values.gradient,
        gradient_spread: values.gradient_spread,
        piece_length_m: values.piece_length_m,
        flights: [1, 0, 0],
        energy_db: values.levels_db,
        tail_energy_db: values.tail_levels_db,
        lg_scaled_distance: values.scaled_distance_m.map(f64::log10),
        installation_shares: values.installation_shares,
        first_piece: 0,
        piece_count: 0,
        loudest_lamax_db: f64::NEG_INFINITY,
    }
}

/// The aircraft layer's day energy of a click at the tile centre.
fn aircraft_day_energy(release: &Release) -> (f64, usize) {
    let (lat, lon) = TILE.centre().to_degrees();
    let mut last = None;
    answer(
        release,
        lat,
        lon,
        &Options {
            exact: false,
            source: Vec::new(),
        },
        &mut |update| {
            let layer = update
                .layers
                .iter()
                .find(|layer| layer.layer == Layer::Aircraft)
                .unwrap();
            last = Some((layer.energy[0], layer.evaluated));
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
    record.energy_db[2] = record.energy_db[0].map(|level| level - 10.0);
    record.tail_energy_db[2] = record.tail_energy_db[0] - 10.0;
    record.flights = [731, 0, 731];
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
    let lambda = |period: usize| heard.energy_lambda[period] / heard.energy[period];
    let ratio = lambda(2) / lambda(0);
    assert!(
        (ratio - PERIOD_HOURS[0] / PERIOD_HOURS[2]).abs() < 1e-9,
        "{ratio}"
    );
}

//! The tiered flights against every box summed: never more, and short of them by at most the share
//! the tiers may leave out.

use super::{LEFT_OUT_AT_MOST, TIERED_FROM_FACADES, flights_at_facades, tiers};
use crate::exact::Point;
use crate::flight_boxes::{FlightBoxes, flights_at_points};
use crate::paint::{Grid, corner_points, metres_at};
use crate::square::{Files, Square};
use physics::bands::PERIODS;
use physics::doc29::box_sums::{BoxSums, SegmentEnergies};
use physics::doc29::profiles_generated::profile_idx;
use physics::doc29::segment::{AircraftType, SegmentEmission};
use physics::doc29::thrust::SegmentFlight;
use physics::weather::{COLUMNS, ROWS, WeatherNode, WeatherTable, encode as encode_weather};
use popup::aircraft::boxes::ReceiverArea;
use popup::aircraft::ceilings::CeilingArea;
use popup::release::Release;
use popup::selection::SplitMix;
use tiles::Kind;
use tiles::aircraft::{AircraftBox, BoxPlace, Group, encode};
use tiles::geo::{LocalFrame, TileId};
use tiles::square::write_tiles;

const TILE: TileId = TileId { x: 2212, y: 1387 };

/// A box of one piece of an A320 or B738 climbing or a C172, `length_m` long at `centre_m` (metres
/// from `tile`'s centre, height above the sea) heading `heading_rad`, in a cell of `zoom`; flown by
/// day and less by night.
fn box_at(
    tile: TileId,
    zoom: u8,
    centre_m: [f64; 3],
    heading_rad: f64,
    length_m: f64,
    kind: usize,
) -> AircraftBox {
    let (designator, departure) = [("A320", true), ("B738", false), ("C172", true)][kind % 3];
    let climb = if departure { 0.07 } else { -0.052 };
    let flight = SegmentFlight {
        departure,
        on_ground: false,
        speed_kt: 160.0,
        pressure_altitude_m: centre_m[2],
        climb_sine: climb,
        acceleration_ms2: None,
        height_above_field_m: centre_m[2],
    };
    let emission = SegmentEmission::new(
        &AircraftType::of(profile_idx(designator), designator),
        &flight,
        false,
    )
    .unwrap();
    let half = 0.5 * length_m;
    let step = [
        heading_rad.cos() * half,
        heading_rad.sin() * half,
        climb * half,
    ];
    let mut sums = BoxSums::default();
    sums.add(
        &SegmentEnergies::of(&emission.npd_distance_levels()),
        emission.installation,
        [1.0, 0.0, 0.3],
        std::array::from_fn(|axis| centre_m[axis] - step[axis]),
        std::array::from_fn(|axis| centre_m[axis] + step[axis]),
    );
    let values = sums.values().unwrap();
    let frame = LocalFrame::at(tile.centre());
    let centroid = [
        (values.centroid_m[0] * 32_768.0 / frame.east_m_per_unit).round() as i16,
        (-values.centroid_m[1] * 32_768.0 / frame.north_m_per_unit).round() as i16,
    ];
    let cell_steps = 32_768 >> (zoom - 12);
    let clearance_m = (values.centroid_m[2] / 250.0).floor() * 250.0;
    AircraftBox {
        place: BoxPlace {
            zoom,
            cell: centroid.map(|c| ((i32::from(c) + 16_384) / cell_steps) as u16),
            group: Group::FixedWing,
            ground_m: 0.0,
            clearance_m,
            height_m: 250.0,
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
        flights_per_day: [3.0, 0.0, 0.9],
        energy_db: values.levels_db,
        tail_energy_db: values.tail_levels_db,
        lg_scaled_distance: values.scaled_distance_m.map(f64::log10),
        installation_shares: values.installation_shares,
    }
}

/// Six hundred receivers within 300 m of a square's middle, as a town's façades, among boxes in it
/// and the eight tiles around, fine and far, low and high, near and kilometres off: the tiers leave
/// boxes out, and no receiver's sums fall short of every box's by more than the share they may.
#[test]
fn the_tiers_leave_out_at_most_their_share() {
    in_square(&|_| true, &|square, boxes, points| {
        let every = flights_at_points(square, boxes, points, None).unwrap();
        let tiered = flights_at_facades(square, boxes, points, None).unwrap();
        let mut short = 0;
        for (every, tiered) in every.iter().zip(&tiered) {
            let parts = |sums: &crate::flights::FlightSums| {
                [sums.energy, sums.lambda.energy, sums.lambda.energy_lambda]
            };
            for (every, tiered) in parts(every).iter().zip(parts(tiered)) {
                for period in 0..PERIODS {
                    let (all, some) = (every[period], tiered[period]);
                    assert!(some <= all * (1.0 + 1e-9), "{some} over {all}");
                    assert!(
                        some >= all * (1.0 - LEFT_OUT_AT_MOST),
                        "{some} short of {all}"
                    );
                    short += usize::from(some < all * (1.0 - 1e-6));
                }
            }
        }
        assert!(short > 1_000, "{short} sums short of every box's");
    });
}

/// Without far files the fine boxes weigh the lambda too (`crate::flights::heard_parts`): what the
/// tiers leave of the lambda's energy is what they leave of the energy.
#[test]
fn fine_boxes_without_far_files_weigh_the_lambda_left_out() {
    in_square(&|_| false, &|square, boxes, points| {
        let area = ReceiverArea::from_points(points.iter().map(|p| (p.position, p.altitude_m)));
        let tiers = tiers(square, boxes, &CeilingArea::of(area));
        assert!(tiers.left[0][0][0] > 0.0);
        assert_eq!(tiers.left[0][0], tiers.left[0][1]);
    });
}

/// `check` of a square with fine boxes in it and the eight tiles around and far boxes in the tiles
/// `far` names, its boxes, and receivers within 300 m of its middle.
fn in_square(far: &dyn Fn(TileId) -> bool, check: &dyn Fn(&Square, &FlightBoxes, &[Point])) {
    let root = tempfile::tempdir().unwrap();
    let year = root.path().join("2026");
    std::fs::create_dir(&year).unwrap();
    let weather_bytes = encode_weather(&vec![WeatherNode::default(); ROWS * COLUMNS]);
    std::fs::write(year.join("weather"), &weather_bytes).unwrap();
    let weather = WeatherTable::parse(&weather_bytes).unwrap();
    let mut draw = SplitMix(20_261_111);
    let mut between =
        |low: f64, high: f64| low + (high - low) * (draw.next_u64() % 1_000_000) as f64 / 1e6;
    for (kind, zoom, count) in [(Kind::Aircraft, 18, 300), (Kind::AircraftFar, 15, 60)] {
        let mut files = Vec::new();
        for (east, south) in (0..9).map(|k| (k % 3, k / 3)) {
            let tile = TileId {
                x: TILE.x + east - 1,
                y: TILE.y + south - 1,
            };
            if kind == Kind::AircraftFar && !far(tile) {
                continue;
            }
            let boxes: Vec<AircraftBox> = (0..count)
                .map(|k| {
                    let centre = [
                        between(-2_900.0, 2_900.0),
                        between(-2_900.0, 2_900.0),
                        between(150.0, 2_500.0),
                    ];
                    box_at(
                        tile,
                        zoom,
                        centre,
                        between(0.0, std::f64::consts::TAU),
                        between(80.0, 600.0),
                        k,
                    )
                })
                .collect();
            files.push((tile, encode(&boxes, &[], &[])));
        }
        write_tiles(&year, kind, &files).unwrap();
    }
    tiles::mark_year_complete(&year, "test").unwrap();
    let release = Release::open(root.path(), "2026").unwrap();
    let files = Files::read(&release, TILE).unwrap();
    let square = Square::new(&weather, TILE, &files).unwrap();
    let grid = Grid {
        pixels: 64,
        block: 8,
        hum_spacing: 16,
    };
    let boxes = FlightBoxes::new(&square, grid, &corner_points(&square, grid).unwrap());
    let points: Vec<Point> = (0..TIERED_FROM_FACADES + 88)
        .map(|_| {
            let at = metres_at(
                &square,
                grid.pixels,
                between(29.0, 35.0),
                between(29.0, 35.0),
            );
            Point::with_receiver_properties(&square, at, 0, 0.0).unwrap()
        })
        .collect();
    check(&square, &boxes, &points);
}

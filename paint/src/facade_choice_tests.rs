//! The painter's façade must be the one a click inside the building is answered at.

use super::{buildings, choose};
use crate::exact::Point;
use crate::paint::{Grid, corners_of, metres_at};
use crate::points::PointPainter;
use crate::square::{Files, Square};
use physics::bands::{BANDS, PERIODS};
use physics::weather::{COLUMNS, ROWS, WeatherNode, WeatherTable, encode as encode_weather};
use popup::building::loudest_facade;
use popup::candidates::GROUND_REACH_M;
use popup::release::Release;
use popup::selection::SplitMix;
use std::sync::atomic::AtomicUsize;
use tiles::Kind;
use tiles::geo::TileId;
use tiles::obstacles::{EnvelopeClass, Outline, OutlineKind, encode as encode_obstacles};
use tiles::sources::{Attribute, Layer, Piece, encode as encode_sources};
use tiles::square::write_tiles;

/// Three buildings among 150 sources of many powers: two side by side, whose facing façades take a
/// reflection bonus, where more than a hundred are loud, and one far enough that fewer are, so that
/// its ranking takes every source in reach.
#[test]
fn every_building_takes_the_popups_facade() {
    let root = tempfile::tempdir().unwrap();
    let year = root.path().join("2026");
    std::fs::create_dir(&year).unwrap();
    let weather_bytes = encode_weather(&vec![WeatherNode::default(); ROWS * COLUMNS]);
    std::fs::write(year.join("weather"), &weather_bytes).unwrap();
    std::fs::write(year.join("complete"), b"qmsq1\n").unwrap();
    let weather = WeatherTable::parse(&weather_bytes).unwrap();
    let tile = TileId { x: 2212, y: 1387 };
    let rectangle = |id: u64, [x, y]: [i16; 2], [half_x, half_y]: [i16; 2]| Outline {
        footprint_id: id,
        kind: OutlineKind::Exterior,
        envelope: EnvelopeClass::Residential,
        height_m: 12.0,
        vertices: vec![
            [x - half_x, y - half_y],
            [x + half_x, y - half_y],
            [x + half_x, y + half_y],
            [x - half_x, y + half_y],
            [x - half_x, y - half_y],
        ],
    };
    let outlines = [
        rectangle(1, [-3_000, 0], [600, 400]),
        rectangle(2, [-1_900, 600], [300, 500]),
        rectangle(3, [-13_000, -13_000], [600, 400]),
    ];
    write_tiles(
        &year,
        Kind::Obstacles,
        &[(tile, encode_obstacles(&outlines))],
    )
    .unwrap();
    let mut draw = SplitMix(20_261_111);
    let mut within = |half: f64| ((draw.next_u64() % 2_000) as f64 / 1_000.0 - 1.0) * half;
    let attributes: Vec<Attribute> = (0..10)
        .map(|k| Attribute {
            layer: [Layer::Industry, Layer::Road][k % 2],
            height_m: 1.0,
            ground_percent: 0,
            platform_half_width_m: 0.0,
            exclusion_radius_m: 0.0,
            footprint_id: 0,
            group_key: k as u64 + 1,
            emission: [[55.0 + 4.0 * k as f64; BANDS]; PERIODS],
            display: vec![serde_json::Value::from("test source")],
        })
        .collect();
    let pieces: Vec<Piece> = (0..150)
        .map(|k| {
            let a = [within(9_000.0) as i16, within(9_000.0) as i16];
            let b = match k % 3 {
                0 => [a[0].saturating_add(within(1_500.0) as i16), a[1]],
                _ => a,
            };
            Piece {
                ends: [a, b],
                attribute: k % 10,
            }
        })
        .collect();
    write_tiles(
        &year,
        Kind::Sources,
        &[(tile, encode_sources(&pieces, &attributes, &[]))],
    )
    .unwrap();
    let release = Release::open(root.path(), "2026").unwrap();
    let grid = Grid {
        pixels: 64,
        block: 8,
        hum_spacing: 16,
    };
    let files = Files::read(&release, tile).unwrap();
    let square = Square::new(&weather, tile, &files).unwrap();
    let corners = corners_of(&square, grid, None).unwrap().unwrap();
    let painter = PointPainter {
        square: &square,
        grid,
        batch: None,
        corners,
        pairs: AtomicUsize::new(0),
    };
    let n = grid.pixels;
    let indoor: Vec<(usize, u64)> = (0..n * n)
        .filter_map(|at| {
            let centre = metres_at(&square, n, (at % n) as f64 + 0.5, (at / n) as f64 + 0.5);
            let point = Point::at(&square, centre).unwrap();
            (point.own_footprint != 0).then_some((at, point.own_footprint))
        })
        .collect();
    let found = buildings(&square, n, &indoor).unwrap();
    assert_eq!(found.len(), 3);
    let chosen = choose(&painter, &found).unwrap();
    for (building, &facade) in found.iter().zip(&chosen) {
        let &(at, _) = (indoor.iter())
            .find(|(_, footprint)| *footprint == building.footprint)
            .unwrap();
        let inside = metres_at(&square, n, (at % n) as f64 + 0.5, (at / n) as f64 + 0.5);
        let footprint = square
            .obstacles
            .enclosing_building(inside)
            .unwrap()
            .unwrap();
        let receivers = square.obstacles.facade_receivers(&footprint).unwrap();
        let reach: Vec<_> = (square.candidates.iter())
            .filter(|candidate| {
                (receivers.iter())
                    .any(|receiver| candidate.distance_from(receiver.position) <= GROUND_REACH_M)
            })
            .collect();
        let (lat, lon) = square.frame.to_mercator(receivers[0].position).to_degrees();
        let popup = loudest_facade(
            &footprint,
            &receivers,
            &reach,
            &square.attributes,
            &square.ground,
            &square.obstacles,
            square.weather.place(lat, lon),
        )
        .unwrap();
        assert_eq!(facade, popup.index, "building {}", building.footprint);
    }
}

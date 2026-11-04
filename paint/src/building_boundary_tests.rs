//! Adjacent squares must answer a split building at the same complete façade.

use crate::paint::{Grid, corners_of, paint_pixels};
use crate::square::{Files, Square};
use physics::bands::{BANDS, PERIODS};
use physics::weather::{COLUMNS, ROWS, WeatherNode, WeatherTable, encode as encode_weather};
use popup::release::Release;
use tiles::Kind;
use tiles::geo::TileId;
use tiles::obstacles::{EnvelopeClass, Outline, OutlineKind, encode as encode_obstacles};
use tiles::sources::{Attribute, Layer, Piece, encode as encode_sources};
use tiles::square::write_tiles;

#[test]
fn both_halves_of_a_building_take_the_facade_facing_its_source() {
    let root = tempfile::tempdir().unwrap();
    let year = root.path().join("2026");
    std::fs::create_dir(&year).unwrap();
    let weather_bytes = encode_weather(&vec![WeatherNode::default(); ROWS * COLUMNS]);
    std::fs::write(year.join("weather"), &weather_bytes).unwrap();
    std::fs::write(year.join("complete"), b"qmsq1\n").unwrap();
    let weather = WeatherTable::parse(&weather_bytes).unwrap();
    let tiles = [TileId { x: 2212, y: 1387 }, TileId { x: 2213, y: 1387 }];
    let outlines: Vec<_> = tiles
        .iter()
        .enumerate()
        .map(|(side, &tile)| {
            let centre = if side == 0 { 16_384 } else { -16_384 };
            let vertices = [
                [-1500, -1500],
                [1500, -1500],
                [1500, 1500],
                [-1500, 1500],
                [-1500, -1500],
            ]
            .map(|[x, y]| [centre + x, y]);
            let mut parts = vec![Outline {
                footprint_id: 7,
                kind: OutlineKind::Exterior,
                envelope: EnvelopeClass::Residential,
                height_m: 15.0,
                vertices: vertices.to_vec(),
            }];
            // A detached wing is stored only in the square it occupies; the complete footprint
            // seen from the other square must still rank all its façades with prepared reflection.
            if side == 0 {
                parts.push(Outline {
                    footprint_id: 7,
                    kind: OutlineKind::Exterior,
                    envelope: EnvelopeClass::Residential,
                    height_m: 15.0,
                    vertices: vec![
                        [-8500, -500],
                        [-7500, -500],
                        [-7500, 500],
                        [-8500, 500],
                        [-8500, -500],
                    ],
                });
            }
            (tile, encode_obstacles(&parts))
        })
        .collect();
    write_tiles(&year, Kind::Obstacles, &outlines).unwrap();
    let source = Attribute {
        layer: Layer::Industry,
        height_m: 1.0,
        ground_percent: 0,
        platform_half_width_m: 0.0,
        exclusion_radius_m: 0.0,
        footprint_id: 0,
        group_key: 1,
        emission: [[100.0; BANDS]; PERIODS],
        display: vec![serde_json::Value::from("test source")],
    };
    write_tiles(
        &year,
        Kind::Sources,
        &[(
            tiles[1],
            encode_sources(
                &[Piece {
                    ends: [[-14_000, 0]; 2],
                    attribute: 0,
                }],
                &[source],
                &[],
            ),
        )],
    )
    .unwrap();
    let release = Release::open(root.path(), "2026").unwrap();
    let grid = Grid {
        pixels: 64,
        block: 8,
        hum: 16,
    };
    let painted: Vec<_> = tiles
        .into_iter()
        .map(|tile| {
            let files = Files::read(&release, tile).unwrap();
            let square = Square::new(&weather, tile, &files).unwrap();
            let corners = corners_of(&square, grid, None).unwrap().unwrap();
            paint_pixels(&square, grid, None, corners).unwrap()
        })
        .collect();
    let left = 32 * grid.pixels + grid.pixels - 1;
    let right = 32 * grid.pixels;
    assert!(painted[0].lden[left][Layer::Industry as usize] < 254);
    assert_eq!(painted[0].lden[left], painted[1].lden[right]);
    assert_eq!(painted[0].nden[left], painted[1].nden[right]);
}

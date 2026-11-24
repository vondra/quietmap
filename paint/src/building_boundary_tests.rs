//! Adjacent squares must answer a split building at the same complete façade.

use crate::paint::{Grid, corners_of, metres_at, paint_pixels};

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
    tiles::mark_year_complete(&year, br#"{"year":"2026","as_of":"2027-01-01"}"#, "test").unwrap();
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
            // A second building in the same input record stands farther from the source.
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
        hum_spacing: 16,
    };
    let painted: Vec<_> = tiles
        .into_iter()
        .map(|tile| {
            let files = Files::read(&release, tile).unwrap();
            let square = Square::new(&weather, tile, &files).unwrap();
            if tile == tiles[0] {
                // The detached building covers column 16. The gap to the split building covers 30.
                for (column, expected) in [(16.5, Some(7)), (30.5, None)] {
                    let point = metres_at(&square, grid.pixels, column, 32.5);
                    assert_eq!(
                        square.obstacles.enclosing_building_id(point).unwrap(),
                        expected
                    );
                }
            }
            let corners = corners_of(&square, grid, None).unwrap().unwrap();
            paint_pixels(&square, grid, None, corners).unwrap()
        })
        .collect();
    let left = 32 * grid.pixels + grid.pixels - 1;
    let right = 32 * grid.pixels;
    assert!(painted[0].lden[left][Layer::Industry as usize] < 254);
    assert_eq!(painted[0].lden[left], painted[1].lden[right]);
    assert_eq!(painted[0].nden[left], painted[1].nden[right]);
    let detached = 32 * grid.pixels + 16;
    let industry = Layer::Industry as usize;
    assert!(painted[0].lden[detached][industry] < painted[0].lden[left][industry]);
    assert_ne!(painted[0].nden[detached], painted[0].nden[left]);
}

/// Two separate homes sharing one `footprint_id` (detached interior A in `tiles[0]` and split
/// boundary B across `tiles[0]`/`tiles[1]`): each excludes only its own part's home plant and
/// still hears the other part's plant, identically on both halves of the split building, while
/// outdoor pixels beside A still hear A.
#[test]
fn multi_part_home_excludes_only_its_own_parts_plant_across_tiles() {
    let root = tempfile::tempdir().unwrap();
    let year = root.path().join("2026");
    std::fs::create_dir(&year).unwrap();
    let weather_bytes = encode_weather(&vec![WeatherNode::default(); ROWS * COLUMNS]);
    std::fs::write(year.join("weather"), &weather_bytes).unwrap();
    tiles::mark_year_complete(&year, br#"{"year":"2026","as_of":"2027-01-01"}"#, "test").unwrap();
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
    let home_attr = |group_key: u64, level: f64| Attribute {
        layer: Layer::Building,
        height_m: 4.0,
        ground_percent: 50,
        platform_half_width_m: 0.0,
        exclusion_radius_m: 0.0,
        footprint_id: 7,
        group_key,
        emission: [[level; BANDS]; PERIODS],
        display: vec![
            serde_json::json!(""),
            serde_json::json!("residential_house"),
        ],
    };
    // Loud home plant inside detached building A (tiles[0], x=-8000, y=0) alone.
    write_tiles(
        &year,
        Kind::Sources,
        &[(
            tiles[0],
            encode_sources(
                &[Piece {
                    ends: [[-8000, 0]; 2],
                    attribute: 0,
                }],
                &[home_attr(1, 95.0)],
                &[],
            ),
        )],
    )
    .unwrap();
    let release = Release::open(root.path(), "2026").unwrap();
    let grid = Grid {
        pixels: 64,
        block: 8,
        hum_spacing: 16,
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
    let bldg = Layer::Building as usize;
    let detached = 32 * grid.pixels + 16;
    let outdoor_beside_a = 32 * grid.pixels + 19;
    let left_b = 32 * grid.pixels + grid.pixels - 1;
    let right_b = 32 * grid.pixels;
    // Detached A excludes its own plant (no other source exists -> SILENCE = 254).
    assert_eq!(painted[0].lden[detached][bldg], crate::levels::SILENCE);
    // Outdoor pixel beside A hears A's plant loud and clear.
    assert!(painted[0].lden[outdoor_beside_a][bldg] < crate::levels::SILENCE);
    // Split building B (sharing footprint_id 7!) still hears A's plant identically on both tiles.
    assert!(painted[0].lden[left_b][bldg] < crate::levels::SILENCE);
    assert_eq!(painted[0].lden[left_b], painted[1].lden[right_b]);
    assert_eq!(painted[0].nden[left_b], painted[1].nden[right_b]);
}

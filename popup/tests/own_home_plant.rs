//! A home's own statistical outdoor plant leaves its own façade while neighbours, bells, non-home
//! building plant, outdoor terraces, industrial sites and outdoor receivers stay audible.

use physics::bands::{BANDS, PERIODS};
use physics::weather::{COLUMNS, ROWS, SECTORS, WeatherNode, encode as encode_weather};
use popup::answer::{Options, answer};
use popup::release::Release;
use serde_json::json;
use std::collections::HashSet;
use std::path::PathBuf;
use tiles::Kind;
use tiles::geo::{LocalFrame, TileId};
use tiles::obstacles::{EnvelopeClass, Outline, OutlineKind, encode as encode_obstacles};
use tiles::sources::{Attribute, Layer, Piece, encode};
use tiles::square::write_tiles;

const TILE: TileId = TileId { x: 2212, y: 1387 };

fn release_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("qm-own-home-{name}-{}", std::process::id()));
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
    root
}

fn steps_per_metre() -> f64 {
    32_768.0 / LocalFrame::at(TILE.centre()).east_m_per_unit
}

fn step(metres: f64) -> i16 {
    (metres * steps_per_metre()).round() as i16
}

fn box_ring(cx: f64, cy: f64, half_x: f64, half_y: f64) -> Vec<[i16; 2]> {
    let (x0, x1) = (step(cx - half_x), step(cx + half_x));
    let (y0, y1) = (step(cy - half_y), step(cy + half_y));
    vec![[x0, y1], [x1, y1], [x1, y0], [x0, y0], [x0, y1]]
}

fn building_attr(
    footprint_id: u64,
    group_key: u64,
    building_type: &str,
    level_db: f64,
) -> Attribute {
    Attribute {
        layer: Layer::Building,
        height_m: 4.0,
        ground_percent: 50,
        platform_half_width_m: 0.0,
        exclusion_radius_m: 0.0,
        footprint_id,
        group_key,
        emission: [[level_db; BANDS]; PERIODS],
        display: vec![json!(""), json!(building_type)],
    }
}

struct ClickResult {
    bearing: Option<f64>,
    building_energy: f64,
    road_energy: f64,
    industry_energy: f64,
    contributor_keys: HashSet<u64>,
    part_lines: Vec<(u64, usize)>,
}

fn click_at(release: &Release, east_m: f64, north_m: f64) -> ClickResult {
    let frame = LocalFrame::at(TILE.centre());
    let (lat, lon) = frame.to_mercator([east_m, north_m]).to_degrees();
    let mut result = None;
    answer(
        release,
        lat,
        lon,
        &Options {
            exact: true,
            source: Vec::new(),
            piece: None,
        },
        &mut |update| {
            let energy_of = |layer: Layer| {
                let entry = update.layers.iter().find(|l| l.layer == layer).unwrap();
                physics::bands::lden_energy(&entry.energy)
            };
            let mut contributor_keys = HashSet::new();
            let mut part_lines = Vec::new();
            for row in &update.rows {
                for part in &row.parts {
                    contributor_keys.insert(part.group_key);
                    part_lines.push((part.group_key, part.lines.len()));
                }
            }
            result = Some(ClickResult {
                bearing: update
                    .building
                    .and_then(|b| b.facade)
                    .map(|f| f.outward_bearing_deg),
                building_energy: energy_of(Layer::Building),
                road_energy: energy_of(Layer::Road),
                industry_energy: energy_of(Layer::Industry),
                contributor_keys,
                part_lines,
            });
            Ok(())
        },
    )
    .unwrap();
    result.expect("final update")
}

/// Detached home H excludes its own `residential_house` plant (ring 1 and ring 2), picks the
/// road-facing façade unbiased by an off-centre own plant, still screens a road behind it, and
/// keeps church bells on its id, an in-building industrial source, a neighbour's home and an
/// outdoor terrace. Outdoors beside H still hears H's plant.
#[test]
fn a_home_excludes_its_own_plant_at_its_facade_while_keeping_external_sources() {
    let root = release_root("detached");
    let outlines = vec![
        Outline {
            footprint_id: 7,
            kind: OutlineKind::Exterior,
            envelope: EnvelopeClass::Residential,
            height_m: 12.0,
            vertices: box_ring(0.0, 0.0, 12.0, 6.0),
        },
        Outline {
            footprint_id: 8,
            kind: OutlineKind::Exterior,
            envelope: EnvelopeClass::Residential,
            height_m: 8.0,
            vertices: box_ring(40.0, 0.0, 6.0, 6.0),
        },
    ];
    write_tiles(
        &root.join("2026"),
        Kind::Obstacles,
        &[(TILE, encode_obstacles(&outlines))],
    )
    .unwrap();

    // Sources in tile 0 (ring 1):
    // 0: own residential_house plant near the west wall (-10 m) — very loud, must be excluded.
    // 1: church_bells sharing footprint_id 7 (nearest_host trap) — must stay.
    // 2: Layer::Industry inside footprint 7 (in-building substation) — must stay.
    // 3: neighbour home (footprint_id 8) — must stay.
    // 4: outdoor restaurant terrace (footprint_id 0) — must stay.
    // 5: road east of H (+30 m) — must choose the east façade (+90 deg).
    let attributes = vec![
        building_attr(7, 101, "residential_house", 92.0),
        building_attr(7, 102, "church_bells", 78.0),
        Attribute {
            layer: Layer::Industry,
            height_m: 3.0,
            ground_percent: 50,
            platform_half_width_m: 0.0,
            exclusion_radius_m: 0.0,
            footprint_id: 7,
            group_key: 103,
            emission: [[75.0; BANDS]; PERIODS],
            display: vec![json!("substation"), json!("substation")],
        },
        building_attr(8, 104, "residential_house", 76.0),
        building_attr(0, 105, "people_restaurant", 74.0),
        Attribute {
            layer: Layer::Road,
            height_m: 0.05,
            ground_percent: 0,
            platform_half_width_m: 5.0,
            exclusion_radius_m: 0.0,
            footprint_id: 0,
            group_key: 106,
            emission: [[72.0; BANDS]; PERIODS],
            display: vec![json!("east road")],
        },
    ];
    let pieces = vec![
        Piece {
            ends: [[step(-10.0), 0]; 2],
            attribute: 0,
        },
        Piece {
            ends: [[step(0.0), 0]; 2],
            attribute: 1,
        },
        Piece {
            ends: [[step(0.0), 0]; 2],
            attribute: 2,
        },
        Piece {
            ends: [[step(40.0), 0]; 2],
            attribute: 3,
        },
        Piece {
            ends: [[step(14.0), 0]; 2],
            attribute: 4,
        },
        Piece {
            ends: [[step(30.0), step(-40.0)], [step(30.0), step(40.0)]],
            attribute: 5,
        },
    ];
    // Ring 2 tile also carries an own home_plant piece for footprint_id 7: ring > 1 must drop it.
    let ring2_tile = TileId {
        x: TILE.x + 2,
        y: TILE.y,
    };
    let ring2_attr = [building_attr(7, 109, "residential_house", 110.0)];
    let ring2_piece = [Piece {
        ends: [[-15_000, 0]; 2],
        attribute: 0,
    }];
    write_tiles(
        &root.join("2026"),
        Kind::Sources,
        &[
            (TILE, encode(&pieces, &attributes, &[])),
            (ring2_tile, encode(&ring2_piece, &ring2_attr, &[])),
        ],
    )
    .unwrap();
    tiles::mark_year_complete(
        &root.join("2026"),
        br#"{"year":"2026","as_of":"2026-10-31"}"#,
        "test",
    )
    .unwrap();

    let release = Release::open(&root, "2026").unwrap();
    let inside = click_at(&release, 0.0, 0.0);
    let bearing = inside.bearing.expect("inside home H");
    assert!(
        (bearing - 90.0).abs() < 1.0,
        "east road should win over excluded west own plant, got bearing {bearing}"
    );
    assert!(
        !inside.contributor_keys.contains(&101),
        "own ring-1 home plant must be excluded"
    );
    assert!(
        !inside.contributor_keys.contains(&109),
        "own ring-2 home plant must be excluded"
    );
    for kept in [102, 103, 104, 105, 106] {
        assert!(
            inside.contributor_keys.contains(&kept),
            "external/non-home source {kept} must remain audible"
        );
    }
    assert!(inside.industry_energy > 0.0 && inside.road_energy > 0.0);

    // Outdoors west of H: H's own home plant IS heard, and H screens the east road behind it.
    let outside_west = click_at(&release, -18.0, 0.0);
    assert!(outside_west.bearing.is_none());
    assert!(
        outside_west.contributor_keys.contains(&101),
        "outdoor click beside home H must still hear H's plant"
    );
    assert!(
        10.0 * (inside.road_energy / outside_west.road_energy).log10() > 8.0,
        "H's walls must still screen the east road at the west side"
    );

    std::fs::remove_dir_all(&root).unwrap();
}

/// Non-home building plant (`food_retail`, `kitchen_extract`) stays audible at its own façade.
#[test]
fn non_home_buildings_keep_their_own_plant_at_their_facade() {
    let root = release_root("non-home");
    let outline = Outline {
        footprint_id: 9,
        kind: OutlineKind::Exterior,
        envelope: EnvelopeClass::Commercial,
        height_m: 10.0,
        vertices: box_ring(0.0, 0.0, 10.0, 10.0),
    };
    write_tiles(
        &root.join("2026"),
        Kind::Obstacles,
        &[(TILE, encode_obstacles(&[outline]))],
    )
    .unwrap();
    let attributes = vec![
        building_attr(9, 201, "food_retail", 75.0),
        building_attr(9, 202, "kitchen_extract", 72.0),
    ];
    let pieces = vec![
        Piece {
            ends: [[0, 0]; 2],
            attribute: 0,
        },
        Piece {
            ends: [[0, 0]; 2],
            attribute: 1,
        },
    ];
    write_tiles(
        &root.join("2026"),
        Kind::Sources,
        &[(TILE, encode(&pieces, &attributes, &[]))],
    )
    .unwrap();
    tiles::mark_year_complete(
        &root.join("2026"),
        br#"{"year":"2026","as_of":"2026-10-31"}"#,
        "test",
    )
    .unwrap();

    let release = Release::open(&root, "2026").unwrap();
    let inside = click_at(&release, 0.0, 0.0);
    assert!(inside.bearing.is_some());
    assert!(inside.contributor_keys.contains(&201));
    assert!(inside.contributor_keys.contains(&202));
    assert!(inside.building_energy > 0.0);

    std::fs::remove_dir_all(&root).unwrap();
}

/// A multi-part record (`id = 10`, separate homes A and B) excludes only the part clicked, and
/// `whole_lines` highlights only the kept part's pieces; a single-part courtyard home (`id = 11`)
/// still excludes its centroid plant sitting inside its courtyard hole.
#[test]
fn multi_part_home_excludes_only_its_own_part_and_courtyard_home_still_excludes_centroid() {
    let root = release_root("multipart-courtyard");
    let outlines = vec![
        // Multi-part record id = 10: Home A at (-30, 0) and Home B at (+30, 0).
        Outline {
            footprint_id: 10,
            kind: OutlineKind::Exterior,
            envelope: EnvelopeClass::Residential,
            height_m: 8.0,
            vertices: box_ring(-30.0, 0.0, 6.0, 6.0),
        },
        Outline {
            footprint_id: 10,
            kind: OutlineKind::Exterior,
            envelope: EnvelopeClass::Residential,
            height_m: 8.0,
            vertices: box_ring(30.0, 0.0, 6.0, 6.0),
        },
        // Single-part courtyard home id = 11 at (0, 60): exterior 24x24 m with 12x12 m courtyard.
        Outline {
            footprint_id: 11,
            kind: OutlineKind::Exterior,
            envelope: EnvelopeClass::Residential,
            height_m: 10.0,
            vertices: box_ring(0.0, 60.0, 12.0, 12.0),
        },
        Outline {
            footprint_id: 11,
            kind: OutlineKind::Hole,
            envelope: EnvelopeClass::Residential,
            height_m: 10.0,
            vertices: box_ring(0.0, 60.0, 6.0, 6.0),
        },
    ];
    write_tiles(
        &root.join("2026"),
        Kind::Obstacles,
        &[(TILE, encode_obstacles(&outlines))],
    )
    .unwrap();

    // Shared group 301 has one piece inside A and one inside B; courtyard home 11 has centroid
    // inside its courtyard hole at (0, 60).
    let attributes = vec![
        building_attr(10, 301, "residential_multi", 72.0),
        building_attr(11, 302, "residential_house", 75.0),
    ];
    let pieces = vec![
        Piece {
            ends: [[step(-30.0), 0]; 2],
            attribute: 0,
        },
        Piece {
            ends: [[step(30.0), 0]; 2],
            attribute: 0,
        },
        Piece {
            ends: [[0, step(60.0)]; 2],
            attribute: 1,
        },
    ];
    write_tiles(
        &root.join("2026"),
        Kind::Sources,
        &[(TILE, encode(&pieces, &attributes, &[]))],
    )
    .unwrap();
    tiles::mark_year_complete(
        &root.join("2026"),
        br#"{"year":"2026","as_of":"2026-10-31"}"#,
        "test",
    )
    .unwrap();

    let release = Release::open(&root, "2026").unwrap();
    // Click inside Home A: keeps B's piece of group 301 (1 point highlighted, not 2!).
    let in_a = click_at(&release, -30.0, 0.0);
    assert!(in_a.bearing.is_some());
    assert!(
        in_a.contributor_keys.contains(&301),
        "Home A must still hear Home B's piece of multi-part record 10"
    );
    let lines_301 = in_a
        .part_lines
        .iter()
        .find(|&&(k, _)| k == 301)
        .map(|&(_, n)| n)
        .unwrap_or(0);
    assert_eq!(
        lines_301, 1,
        "whole_lines must omit A's excluded piece and highlight only B's kept piece"
    );

    // Click inside the north wing of single-part courtyard home 11 at (0, -51) (tile y grows south):
    // its centroid plant at (0, +60 steps = -60 m north) inside the courtyard hole must still be excluded!
    let in_courtyard_home = click_at(&release, 0.0, -51.0);
    assert!(in_courtyard_home.bearing.is_some());
    assert!(
        !in_courtyard_home.contributor_keys.contains(&302),
        "single-part courtyard home must exclude its own centroid source inside the courtyard"
    );
    assert!(
        in_courtyard_home.contributor_keys.contains(&301),
        "courtyard home must still hear neighbour 10"
    );

    std::fs::remove_dir_all(&root).unwrap();
}

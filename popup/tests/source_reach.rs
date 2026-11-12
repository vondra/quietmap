//! Source tiles beyond the chosen receiver's reach are not read; every streamed answer stays the same.

use physics::bands::{BANDS, PERIODS};
use physics::weather::{COLUMNS, ROWS, SECTORS, WeatherNode, encode as encode_weather};
use popup::answer::{Options, answer};
use popup::candidates::GROUND_REACH_M;
use popup::json::update_line;
use popup::release::Release;
use tiles::Kind;
use tiles::geo::{LocalFrame, Mercator, TILES_PER_AXIS, TileId};
use tiles::sources::{Attribute, Layer, Piece, encode};
use tiles::square::write_tiles;

#[test]
fn a_source_tile_outside_reach_adds_no_reads_or_changes_to_the_answer() {
    let root = std::env::temp_dir().join(format!("qm-source-reach-{}", std::process::id()));
    std::fs::create_dir_all(root.join("2026")).unwrap();
    let weather = vec![
        WeatherNode {
            percent: [[50; SECTORS]; PERIODS],
            ..WeatherNode::default()
        };
        ROWS * COLUMNS
    ];
    std::fs::write(root.join("2026/weather"), encode_weather(&weather)).unwrap();
    tiles::mark_year_complete(&root.join("2026"), "test").unwrap();
    let release = Release::open(&root, "2026").unwrap();
    let source = |group, at| {
        encode(
            &[Piece {
                ends: [at; 2],
                attribute: 0,
            }],
            &[Attribute {
                layer: Layer::Industry,
                height_m: 2.0,
                ground_percent: 0,
                platform_half_width_m: 0.0,
                exclusion_radius_m: 0.0,
                footprint_id: 0,
                group_key: group,
                emission: [[100.0; BANDS]; PERIODS],
                display: vec![serde_json::Value::from("source")],
            }],
            &[],
        )
    };
    for (lat, lon) in [(50.0, 14.0), (0.0, 179.99), (78.0, 15.0)] {
        let centre = TileId::containing(Mercator::from_degrees(lat, lon));
        let origin = Mercator {
            x: f64::from(centre.x) + 0.001,
            y: f64::from(centre.y) + 0.001,
        };
        let frame = LocalFrame::at(origin);
        let ring =
            (GROUND_REACH_M / frame.east_m_per_unit.min(frame.north_m_per_unit)).ceil() as u32;
        let near = TileId {
            x: (centre.x + TILES_PER_AXIS - ring) % TILES_PER_AXIS,
            y: centre.y,
        };
        let far = TileId {
            x: (centre.x + ring) % TILES_PER_AXIS,
            y: centre.y + ring,
        };
        let nearest = frame.to_metres(far.to_mercator([-16_384; 2]));
        assert!(nearest[0].hypot(nearest[1]) > GROUND_REACH_M + 1.0);
        let mut files = vec![
            (centre, source(1, [100, 0])),
            (near, source(2, [16_383, 0])),
        ];
        write_tiles(&release.year_root, Kind::Sources, &files).unwrap();
        let (lat, lon) = origin.to_degrees();
        let query = || {
            let mut updates = Vec::new();
            answer(
                &release,
                lat,
                lon,
                &Options {
                    exact: true,
                    source: Vec::new(),
                    piece: None,
                },
                &mut |update| {
                    let mut json: serde_json::Value =
                        serde_json::from_str(&update_line(update, 1)?).unwrap();
                    json.as_object_mut().unwrap().remove("stats");
                    updates.push((json, update.statistics.reads, update.statistics.bytes));
                    Ok(())
                },
            )
            .unwrap();
            updates
        };
        let before = query();
        files.push((far, source(3, [-16_384; 2])));
        write_tiles(&release.year_root, Kind::Sources, &files).unwrap();
        let after = query();
        assert_eq!(before, after, "out-of-reach tile at {lat}, {lon}");
        assert_eq!(after.last().unwrap().1, 3, "weather and two source tiles");
    }
    std::fs::remove_dir_all(root).unwrap();
}

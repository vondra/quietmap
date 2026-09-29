//! The ring loop on a synthetic release (flat ocean, one tile of road pieces): the fast answer
//! stays within 0.1 dB of the exact one while evaluating fewer pieces, and a complete release is
//! required.

use physics::bands::{BANDS, PERIODS};
use physics::weather::{COLUMNS, ROWS, SECTORS, encode as encode_weather};
use popup::answer::{Options, answer};
use popup::release::Release;
use std::path::{Path, PathBuf};
use tiles::geo::{LocalFrame, TileId};
use tiles::sources::{Attribute, Layer, Piece, encode};
use tiles::{COMPLETION_MARKER, Kind, tile_path};

const TILE: TileId = TileId { x: 2212, y: 1387 };

fn release_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("qm-ring-loop-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("2026")).unwrap();
    let weather = vec![[[50u8; SECTORS]; PERIODS]; ROWS * COLUMNS];
    std::fs::write(root.join("weather"), encode_weather(&weather)).unwrap();
    root
}

/// Road pieces of 100 m running north-south at `distances` east of the tile centre.
fn write_roads(root: &Path, distances: &[f64]) {
    let frame = LocalFrame::at(TILE.centre());
    let steps_per_metre = 32_768.0 / frame.east_m_per_unit;
    let half = (50.0 * steps_per_metre).round() as i16;
    let pieces: Vec<Piece> = distances
        .iter()
        .map(|&distance| {
            let x = (distance * steps_per_metre).round() as i16;
            Piece {
                ends: [[x, -half], [x, half]],
                attribute: 0,
            }
        })
        .collect();
    let attributes = [Attribute {
        layer: Layer::Road,
        height_m: 0.05,
        ground_percent: 0,
        platform_half_width_m: 5.0,
        exclusion_radius_m: 0.0,
        footprint_id: 0,
        group_key: 1,
        emission: [[70.0; BANDS]; PERIODS],
        display: r#"["test road"]"#.into(),
    }];
    let path = tile_path(&root.join("2026"), TILE, Kind::Sources);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, encode(&pieces, &attributes)).unwrap();
}

/// The road layer's per-period energies and evaluated count of the last update.
fn road_answer(release: &Release, exact: bool) -> ([f64; PERIODS], usize, usize) {
    let (lat, lon) = TILE.centre().to_degrees();
    let mut last = None;
    answer(
        release,
        lat,
        lon,
        &Options { exact, pieces: 0 },
        &mut |update| {
            let road = update
                .layers
                .iter()
                .find(|layer| layer.layer == Layer::Road)
                .unwrap();
            last = Some((road.energy, road.evaluated, update.contributors.len()));
            Ok(())
        },
    )
    .unwrap();
    last.unwrap()
}

#[test]
fn the_fast_answer_is_within_a_tenth_of_a_decibel_of_the_exact_one() {
    let root = release_root("fast");
    let distances: Vec<f64> = (0..40)
        .map(|i| 20.0 * 1.15f64.powi(i))
        .filter(|d| *d < 2_900.0)
        .collect();
    write_roads(&root, &distances);
    std::fs::write(root.join("2026").join(COMPLETION_MARKER), "test").unwrap();
    let release = Release::open(&root, "2026").unwrap();
    let (exact, exact_count, contributors) = road_answer(&release, true);
    let (fast, fast_count, _) = road_answer(&release, false);
    assert_eq!(exact_count, distances.len());
    assert_eq!(contributors, 1, "every piece shares one display group");
    for period in 0..PERIODS {
        let difference = 10.0 * (exact[period] / fast[period]).log10();
        assert!(
            (0.0..=0.1).contains(&difference),
            "period {period}: {difference} dB"
        );
    }
    assert!(fast_count < exact_count, "{fast_count} of {exact_count}");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn an_incomplete_release_is_never_served() {
    let root = release_root("incomplete");
    assert!(Release::open(&root, "2026").is_err());
    std::fs::remove_dir_all(&root).unwrap();
}

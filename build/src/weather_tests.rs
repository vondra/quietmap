//! Exact climatology rounding, validation and opt-in comparison with frozen meteorology windows.

use super::*;
use crate::era5::numpy::Array;

#[test]
fn probabilities_use_ties_even_and_absorption_keeps_the_published_f32_rounding() {
    let mut statistics = era5::Statistics {
        counts: Array::floats(vec![1, 3], &[200.0; 3]),
        favourable: Array::floats(vec![1, 3, 16], &[49.0; 48]),
        means: Array::floats(
            vec![1, 3, 8],
            &(0..24)
                .map(|index| index as f64 + 0.123456789)
                .collect::<Vec<_>>(),
        ),
    };
    let node = Node::from_statistics(&statistics, 0).unwrap();
    assert_eq!(node.percent, [[24; 16]; 3]);
    assert_eq!(node.means[1][0], 8.123456789f64 as f32);
    assert_eq!(
        node.native().alpha_db_per_km[0],
        (12.0 * f64::from(0.123456789f64 as f32)
            + 4.0 * f64::from(8.123456789f64 as f32)
            + 8.0 * f64::from(16.123456789f64 as f32))
            / 24.0
    );
    statistics.favourable = Array::floats(vec![1, 3, 16], &[51.0; 48]);
    assert_eq!(
        Node::from_statistics(&statistics, 0).unwrap().percent,
        [[26; 16]; 3]
    );
    statistics.counts = Array::floats(vec![1, 3], &[0.0; 3]);
    assert!(Node::from_statistics(&statistics, 0).is_err());
    statistics.counts = Array::floats(vec![1, 3], &[50.0; 3]);
    assert!(Node::from_statistics(&statistics, 0).is_err());
    statistics.counts = Array::floats(vec![1, 3], &[200.0; 3]);
    statistics.means = Array::floats(vec![1, 3, 8], &[f64::NAN; 24]);
    assert!(Node::from_statistics(&statistics, 0).is_err());
}

/// Run with QM_PORT_YEAR, QM_PORT_SOURCES, QM_PORT_RASTERS, QM_PORT_SQUARES and QM_PORT_OUTPUT.
/// The projection exists only in this test; production never writes the discarded variance.
#[test]
#[ignore = "requires retained ERA5 statistics and immutable prepared fixtures"]
fn climatology_matches_frozen_meteorology_windows() {
    let variable = |name| std::env::var(name).unwrap_or_else(|_| panic!("missing {name}"));
    let inputs = YearInputs::read(
        Path::new(&variable("QM_PORT_YEAR")),
        Path::new(&variable("QM_PORT_SOURCES")),
    )
    .unwrap();
    let native = Path::new(&variable("QM_PORT_OUTPUT")).join("weather");
    std::fs::create_dir_all(native.parent().unwrap()).unwrap();
    build(&inputs, &native).unwrap();
    assert_eq!(
        std::fs::read(&native).unwrap(),
        std::fs::read(variable("QM_PORT_NATIVE_WEATHER")).unwrap()
    );
    let statistics = era5::Statistics::read(&inputs).unwrap();
    let mut archive =
        zip::ZipArchive::new(std::fs::File::open(variable("QM_PORT_ORIGINAL_WEATHER")).unwrap())
            .unwrap();
    let m2 = Array::read(&mut archive, "m2.npy").unwrap();
    m2.require("<f8", &[era5::ROWS * era5::COLUMNS, 3, 8])
        .unwrap();
    let reference = variable("QM_PORT_RASTERS");
    let output = variable("QM_PORT_OUTPUT");
    let mut failures = Vec::new();
    for square in crate::parse_squares(&variable("QM_PORT_SQUARES")).unwrap() {
        let west = (square.x as i32 * 1440 / 512) - 720;
        let east = (((square.x + 1) as i32 * 1440 + 511) / 512) - 720;
        let edge = |y: u32| {
            (std::f64::consts::PI * (1.0 - 2.0 * f64::from(y) / 512.0))
                .sinh()
                .atan()
                .to_degrees()
                * 4.0
        };
        let north = if square.y == 0 {
            360
        } else {
            edge(square.y).ceil() as i32
        };
        let south = if square.y == 511 {
            -360
        } else {
            edge(square.y + 1).floor() as i32
        };
        let (columns, rows) = ((east - west + 1) as u16, (north - south + 1) as u16);
        let mut bytes = b"qm-met1\n".to_vec();
        bytes.extend((west as i16).to_le_bytes());
        bytes.extend((north as i16).to_le_bytes());
        bytes.extend(columns.to_le_bytes());
        bytes.extend(rows.to_le_bytes());
        for row in 0..rows as i32 {
            for column in 0..columns as i32 {
                let index = (360 - north + row) as usize * era5::COLUMNS
                    + (west + column).rem_euclid(1440) as usize;
                let node = Node::from_statistics(&statistics, index).unwrap();
                bytes.extend(node.percent.into_iter().flatten());
                bytes.extend(node.means.into_iter().flatten().flat_map(f32::to_le_bytes));
                for period in 0..3 {
                    for band in 0..8 {
                        let variance = (m2.value((index * 3 + period) * 8 + band)
                            / statistics.counts.value(index * 3 + period))
                            as f32;
                        bytes.extend(variance.to_le_bytes());
                    }
                }
            }
        }
        let relative = format!("z9/{}/{}/meteorology.bin", square.x, square.y);
        let baseline = std::fs::read(Path::new(&reference).join(&relative)).unwrap();
        let path = Path::new(&output).join(&relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let different = baseline
            .iter()
            .zip(&bytes)
            .filter(|(left, right)| left != right)
            .count();
        println!(
            "{relative}: {} nodes, {} differing bytes, {} bytes",
            rows as usize * columns as usize,
            different,
            bytes.len()
        );
        if baseline != bytes {
            failures.push(relative);
        }
    }
    assert!(
        failures.is_empty(),
        "different weather fixtures: {failures:?}"
    );
}

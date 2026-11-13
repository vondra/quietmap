//! Shared survey caches retain publisher interleaving and exclude unadmitted sources.

use super::*;
use arrow_array::{ArrayRef, BinaryArray, Float32Array};
use arrow_schema::{DataType, Field, Schema};
use geos::{Geom, Geometry};
use parquet::arrow::ArrowWriter;
use std::{fs::File, sync::Arc};

#[test]
fn shared_survey_objects_are_read_once_in_original_order() {
    let root = std::env::temp_dir().join(format!("qm-survey-order-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("observations.parquet");
    let polygon = Geometry::new_from_wkt(
        "POLYGON ((14.17 49.78,14.1702 49.78,14.1702 49.7802,14.17 49.7802,14.17 49.78))",
    )
    .unwrap();
    let wkb = polygon.to_wkb().unwrap();
    let schema = Arc::new(Schema::new_with_metadata(
        vec![
            Field::new("geometry", DataType::Binary, false),
            Field::new("height_m", DataType::Float32, false),
            Field::new("source", DataType::Utf8, false),
        ],
        std::collections::HashMap::from([(
            "measured_heights_contract".to_owned(),
            "measured_heights_v1".to_owned(),
        )]),
    ));
    let columns: Vec<ArrayRef> = vec![
        Arc::new(BinaryArray::from_vec(vec![&wkb, &wkb, &wkb])),
        Arc::new(Float32Array::from(vec![12.0, 34.0, 99.0])),
        Arc::new(StringArray::from(vec!["second", "first", "later"])),
    ];
    let mut writer =
        ArrowWriter::try_new(File::create(&path).unwrap(), schema.clone(), None).unwrap();
    writer
        .write(&RecordBatch::try_new(schema, columns).unwrap())
        .unwrap();
    writer.close().unwrap();
    let source = |name: &str| {
        (
            Objects {
                format: "measured-heights".into(),
                source: Some(name.into()),
                objects: vec![Object {
                    path: "observations.parquet".into(),
                    degree: Some([49, 14]),
                }],
            },
            root.clone(),
        )
    };
    let mut rows = vec![Vec::new()];
    surveys::convert(
        &[source("first"), source("second")],
        &[Square { x: 276, y: 174 }],
        &mut rows,
        false,
    )
    .unwrap();
    assert_eq!(
        rows[0].iter().map(|r| r.height_m).collect::<Vec<_>>(),
        vec![Some(12.0), Some(34.0)]
    );
    let (mut unscoped, directory) = source("first");
    unscoped.source = None;
    assert!(
        surveys::convert(
            &[(unscoped, directory)],
            &[Square { x: 276, y: 174 }],
            &mut rows,
            false
        )
        .unwrap_err()
        .to_string()
        .contains("requires a publisher source")
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn neighbour_selection_equals_a_scan_of_every_square() {
    let squares: Vec<Square> = (0..512)
        .flat_map(|x| (0..512).map(move |y| Square { x, y }))
        .collect();
    let index = square_index(&squares);
    // A checkerboard selection: half the points' owners are unselected while neighbours are.
    let sparse: Vec<Square> = squares
        .iter()
        .copied()
        .filter(|s| (s.x + s.y) % 2 == 0)
        .collect();
    let sparse_index = square_index(&sparse);
    // Square corners and edges (the halo's reach), the antimeridian and both poles.
    let mut points = vec![(179.9999, 0.0), (-180.0, 0.0), (0.0, 90.0), (12.3, -90.0)];
    for (x, y) in [
        (276, 174),
        (0, 0),
        (511, 511),
        (511, 200),
        (0, 300),
        (100, 1),
    ] {
        let [west, south, east, north] = bounds(Square { x, y });
        for (lon, lat) in [(west, south), (east, north), (west, (south + north) / 2.0)] {
            for d in [-0.003, -0.0001, 0.0, 0.0001, 0.003] {
                points.push((lon + d, (lat - d).clamp(-90.0, 90.0)));
            }
        }
    }
    for (lon, lat) in points {
        for halo in [0.0, 31.0, SUPPORT_HALO_M] {
            let scanned: Vec<(usize, bool)> = squares
                .iter()
                .enumerate()
                .filter_map(|(i, &s)| {
                    let (keep, owned) = selected(lon, lat, s, halo);
                    keep.then_some((i, owned))
                })
                .collect();
            let mut chosen = selected_squares(lon, lat, halo, &index);
            chosen.sort_unstable();
            assert_eq!(chosen, scanned, "{lon},{lat} at {halo} m");
            let mut expected: Vec<(usize, bool)> = scanned
                .iter()
                .filter_map(|&(i, owned)| sparse_index.get(&squares[i]).map(|&j| (j, owned)))
                .collect();
            expected.sort_unstable();
            let mut chosen = selected_squares(lon, lat, halo, &sparse_index);
            chosen.sort_unstable();
            assert_eq!(chosen, expected, "sparse {lon},{lat} at {halo} m");
        }
    }
}

//! End-to-end occupancy, empty squares, priority, grid metadata, completion and deterministic row order.
use super::*;
use crate::dev4::column;
use crate::ship_fetch::tests::Fixture;
use serde_json::json;

#[test]
fn build_writes_native_grids_in_producer_order_and_binds_all_squares_to_the_manifest() {
    let fixture = Fixture::new("build");
    let inputs = fixture.inputs();
    let out = fixture.root.join("2000/ships");
    std::fs::create_dir_all(&out).unwrap();
    assert!(input_root(&inputs).is_err());
    assert_eq!(build(&inputs, &[fixture.square], &out).unwrap(), 2);
    input_root(&inputs).unwrap();
    check_squares(&out, &[fixture.square]).unwrap();
    let mut rows = Vec::new();
    let mut empty = None;
    for square in fixture.square.with_neighbours() {
        let table = table(&out, square).unwrap();
        let grid = grids(&table).unwrap();
        assert_eq!(
            grid[&42].native,
            NativeGrid {
                epsg: 3035,
                width: 1000.0,
                height: 1000.0
            }
        );
        assert_eq!(
            grid[&43].native,
            NativeGrid {
                epsg: 4326,
                width: 0.01,
                height: 0.01
            }
        );
        let mut count = 0;
        for batch in &table.batches {
            let ids = column::<UInt16Array>(batch, "source_id").unwrap();
            for row in 0..batch.num_rows() {
                rows.push(ids.value(row));
                count += 1;
            }
        }
        if count == 0 {
            empty = Some(square);
        }
    }
    assert_eq!(
        rows,
        [43, 42],
        "the earlier block must precede the higher-priority density source"
    );
    assert!(check_squares(&out, &[Square { x: 0, y: 0 }]).is_err());
    let empty = empty.unwrap();
    std::fs::remove_file(path(&out, empty)).unwrap();
    assert!(table(&out, empty).err().unwrap().contains("missing"));
    assert!(
        check_squares(&out, &[fixture.square])
            .unwrap_err()
            .contains("missing")
    );
    let manifest = std::fs::read(&inputs.manifest_path).unwrap();
    std::fs::write(&inputs.manifest_path, [manifest.as_slice(), b"\n"].concat()).unwrap();
    assert!(input_root(&inputs).unwrap_err().contains("differs"));
}
#[test]
fn unmasked_priority_and_duplicate_source_ids_are_rejected() {
    let mut fixture = Fixture::new("priority");
    fixture.manifest["inputs"][1]["conversion"]["priority"] = json!(30);
    let error = build(
        &fixture.inputs(),
        &[fixture.square],
        &fixture.root.join("output"),
    )
    .unwrap_err();
    assert!(error.contains("lowest priority"));
    fixture.manifest["inputs"][1]["conversion"]["priority"] = json!(10);
    fixture.manifest["inputs"][1]["conversion"]["source_id"] = json!(42);
    assert!(
        build(
            &fixture.inputs(),
            &[fixture.square],
            &fixture.root.join("output")
        )
        .unwrap_err()
        .contains("distinct")
    );
}
#[test]
fn native_grid_axes_follow_crs_and_dimensions_without_a_publisher_number() {
    let grid = |epsg, width, height| {
        Grid::new(NativeGrid {
            epsg,
            width,
            height,
        })
        .unwrap()
    };
    let laea = grid(3035, 1000.0, 1000.0);
    let [east, north] = laea.axes((28.1, -15.4), 1e6);
    let bearing = |step: [f64; 2]| step[0].atan2(step[1]).to_degrees();
    assert!((bearing(east) - 71.6).abs() < 0.5 && (bearing(north) + 14.5).abs() < 0.5);
    assert!((east[0] * north[1] - east[1] * north[0] - 1e6).abs() < 1e-3);
    let rectangle = grid(3035, 2000.0, 500.0).axes((28.1, -15.4), 1e6);
    assert_eq!(rectangle, [east.map(|v| v * 2.0), north.map(|v| v / 2.0)]);
    let geographic = grid(4326, 0.01, 0.01);
    assert_eq!(
        geographic.axes((60.0, 25.0), 6e5),
        [
            [0.01 * (111320.0 * 60f64.to_radians().cos()), 0.0],
            [0.0, 1105.4]
        ]
    );
    assert_eq!(
        geographic.area_m2(60.0),
        (0.01 * 111320.0) * (0.01 * 111320.0) * 60f64.to_radians().cos()
    );
    assert!(
        Grid::new(NativeGrid {
            epsg: 3857,
            width: 1000.0,
            height: 1000.0
        })
        .is_err()
    );
}
#[test]
fn every_listed_year_manifest_has_ship_decoding_parameters_and_admitted_dependencies() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../years");
    let mut count = 0;
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if !path
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .chars()
            .all(|c| c.is_ascii_digit())
            || path.extension().is_none_or(|e| e != "json")
        {
            continue;
        }
        let manifest = crate::year::Manifest::read(&path).unwrap();
        for input in manifest
            .inputs
            .iter()
            .filter(|i| i.kind == Kind::PrepareShips)
        {
            ship_fetch::specification(input).unwrap();
            assert!(manifest.admitted(&input.id).unwrap());
            count += 1;
        }
    }
    assert!(count > 0);
}

/// The reference records are emitted by the pre-port Rust cell_axes, extracted unchanged by the parity harness.
#[test]
#[ignore = "requires frozen ship rows and the independently compiled previous geometry"]
fn every_frozen_ship_row_keeps_its_exact_acoustic_grid_axes() {
    let root = std::env::var("QM_PORT_SHIP_OCCUPANCY").unwrap();
    let square = crate::parse_squares(&std::env::var("QM_PORT_GRID_SQUARE").unwrap()).unwrap()[0];
    let grids = grids(&table(Path::new(&root), square).unwrap()).unwrap();
    let bytes = std::fs::read(std::env::var("QM_PORT_REFERENCE_AXES").unwrap()).unwrap();
    const RECORD_BYTES: usize = 4 + 4 + 4 + 2 + 4 * 8;
    assert_eq!(bytes.len() % RECORD_BYTES, 0);
    for (index, record) in bytes.chunks_exact(RECORD_BYTES).enumerate() {
        let gx = i32::from_le_bytes(record[..4].try_into().unwrap());
        let gy = i32::from_le_bytes(record[4..8].try_into().unwrap());
        let area = f32::from_le_bytes(record[8..12].try_into().unwrap());
        let source = u16::from_le_bytes(record[12..14].try_into().unwrap());
        let axes = grids[&source].axes(crate::dev4::z30_corner_degrees(gx, gy), f64::from(area));
        let actual: Vec<u8> = axes
            .into_iter()
            .flatten()
            .flat_map(f64::to_le_bytes)
            .collect();
        assert_eq!(
            actual,
            &record[14..],
            "row {index}, source {source}, {gx},{gy}"
        );
    }
    println!(
        "{} frozen rows: acoustic axes are bit-equal",
        bytes.len() / RECORD_BYTES
    );
}

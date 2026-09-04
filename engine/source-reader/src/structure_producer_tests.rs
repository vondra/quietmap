//! The real Python structure producer feeds all three Rust popup consumers.

use std::path::Path;
use std::process::Command;

#[test]
fn empty_files_do_not_bypass_any_screening_height_reader() {
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::ipc::writer::FileWriter;
    use std::collections::HashMap;
    use std::fs::File;

    for (stamp, dtype) in [
        ("structures_v2", DataType::Int16),
        (
            square_store::structure_contract::CONTRACT,
            DataType::Float32,
        ),
    ] {
        let output = tempfile::tempdir().unwrap();
        let dir = output.path().join("z9/276/174");
        std::fs::create_dir_all(&dir).unwrap();
        let schema =
            Schema::new(vec![Field::new("height_m", dtype, false)]).with_metadata(HashMap::from([
                ("structures_contract".to_string(), stamp.to_string()),
                ("grid".to_string(), "z30".to_string()),
            ]));
        let file = File::create(dir.join("structures.arrow")).unwrap();
        FileWriter::try_new(file, &schema)
            .unwrap()
            .finish()
            .unwrap();
        assert!(square_store::store::load_square(&dir).is_err());
        assert!(crate::structure_store::load_obstacle_set(
            output.path(),
            output.path(),
            49.78,
            14.17
        )
        .is_err());
        assert!(crate::structure_store::footprints_in_bbox(
            output.path(),
            49.77,
            14.16,
            49.80,
            14.20
        )
        .is_err());
    }
}

#[test]
#[ignore = "requires the project .venv geospatial Python dependencies"]
fn python_structure_producer_preserves_screening_and_emission_contracts() {
    let project = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let output = tempfile::tempdir().unwrap();
    let generated = Command::new(project.join(".venv/bin/python"))
        .current_dir(project.join("scripts/structures"))
        .args(["-c", "import sys; from test_structures_fixtures import write_prepared_roundtrip; write_prepared_roundtrip(sys.argv[1])"])
        .arg(output.path())
        .output().unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let square = square_store::store::load_square(&output.path().join("z9/276/174")).unwrap();
    let batches = square.structures.batches_all();
    let buildings = crate::query::query_buildings_from_batches(&batches, 49.78, 14.17, 2000.0);
    assert_eq!(buildings.len(), 1);
    assert_eq!(buildings[0].height, 4.5);
    let walls = square_store::barriers::query_barriers_from_batches(&batches, 49.78, 14.17, 2000.0)
        .unwrap();
    assert_eq!(walls.len(), 1);
    assert_eq!(walls[0].height, 3.0);
    let footprints =
        crate::structure_store::footprints_in_bbox(output.path(), 49.77, 14.16, 49.80, 14.20)
            .unwrap();
    let mut heights: Vec<_> = footprints.iter().map(|row| row.height_m).collect();
    heights.sort_by(f32::total_cmp);
    assert_eq!(heights, [5.0, 13.0]);
    let obstacles =
        crate::structure_store::load_obstacle_set(output.path(), output.path(), 49.78, 14.17)
            .unwrap();
    let (_, height) =
        crate::structure_store::point_inside_footprint(&obstacles, 49.78008, 14.17010).unwrap();
    assert_eq!(height, 5.0);
}

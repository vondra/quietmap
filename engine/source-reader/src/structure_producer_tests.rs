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
    let batches = square.structures.batches_all().unwrap();
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

#[test]
#[ignore = "requires project geospatial Python and server Node dependencies"]
fn real_parts_and_courtyards_reach_native_json_and_png() {
    use serde_json::Value;
    let project = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let output = tempfile::tempdir().unwrap();
    let generated = Command::new(project.join(".venv/bin/python"))
        .current_dir(project.join("scripts/structures"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .args(["-c", "import sys; from test_structures_fixtures import write_topology_roundtrip; write_topology_roundtrip(sys.argv[1])"])
        .arg(output.path()).output().unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let cases: Vec<Value> =
        serde_json::from_slice(&std::fs::read(output.path().join("topology.json")).unwrap())
            .unwrap();
    let mut png_cases = Vec::new();
    for case in cases {
        let root = Path::new(case["root"].as_str().unwrap());
        let (lat, lon) = (case["lat"].as_f64().unwrap(), case["lon"].as_f64().unwrap());
        let index = super::build_square_index(
            grid::square_of(lat, lon),
            &root
                .join(case["square"].as_str().unwrap())
                .join("structures.arrow"),
        )
        .unwrap();
        let obstacles = noise_compute::propagation::obstacle_index::ObstacleSet {
            indexes: vec![std::sync::Arc::new(index)],
        };
        for point in case["points"].as_array().unwrap() {
            let (lat, lon, inside) = (
                point[0].as_f64().unwrap(),
                point[1].as_f64().unwrap(),
                point[2].as_bool().unwrap(),
            );
            assert_eq!(
                super::point_inside_footprint(&obstacles, lat, lon).is_some(),
                inside,
                "{point}"
            );
            assert_eq!(
                super::point_inside_enclosed(&obstacles, lat, lon).is_some(),
                inside,
                "{point}"
            );
            let mut hits = Vec::new();
            obstacles.crossings(lat, lon - 0.001, lat, lon + 0.001, &mut hits);
            assert!(
                hits.iter().all(|hit| hit.id == 0),
                "parts/holes must share one obstacle id: {hits:?}"
            );
        }
        let square =
            square_store::store::load_square(&root.join(case["square"].as_str().unwrap())).unwrap();
        let batches = square.structures.batches_all().unwrap();
        let buildings = crate::query::query_buildings_from_batches(&batches, lat, lon, 2000.0);
        assert_eq!(buildings.len(), 1, "one original OSM emission");
        assert_eq!(buildings[0].height, 4.5);
        let emission_bytes = grid::poly::encode_grid_poly(&buildings[0].polygon_grid);
        let emission_hex: String = emission_bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(emission_hex, case["emission_geom"].as_str().unwrap());
        let footprints =
            super::footprints_in_bbox(root, lat - 0.01, lon - 0.01, lat + 0.01, lon + 0.01)
                .unwrap();
        assert_eq!(footprints.len(), 1);
        assert_eq!(
            footprints[0].polygons.len(),
            case["parts"].as_u64().unwrap() as usize
        );
        assert_eq!(
            footprints[0].polygons.iter().map(Vec::len).sum::<usize>(),
            case["rings"].as_u64().unwrap() as usize
        );
        assert_eq!(footprints[0].height_m, 12.0);
        // Same serialization as query_obstacle_footprints; the actual PNG renderer consumes it.
        png_cases.push(serde_json::json!({"rows": footprints, "points": case["points"]}));
    }
    let payload = output.path().join("native-footprints.json");
    std::fs::write(&payload, serde_json::to_vec(&png_cases).unwrap()).unwrap();
    let rendered = Command::new("node")
        .current_dir(project.join("server"))
        .args(["--import", "tsx", "--input-type=module", "-e", r#"
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { inflateSync } from 'node:zlib';
import { renderBuildingVectorTile } from './src/engine/raster-tile-renderer.ts';
let checked = 0;
for (const { rows, points } of JSON.parse(readFileSync(process.argv[1], 'utf8'))) {
  for (const [lat, lon, inside] of points) {
    const z = 22, axis = 2 ** z;
    const tx = (lon + 180) / 360 * axis;
    const ty = (1 - Math.asinh(Math.tan(lat * Math.PI / 180)) / Math.PI) / 2 * axis;
    const png = await renderBuildingVectorTile(z, Math.floor(tx), Math.floor(ty), async () => JSON.stringify(rows));
    const compressed = [];
    for (let offset = 8; offset < png.length;) {
      const length = png.readUInt32BE(offset);
      if (png.toString('ascii', offset + 4, offset + 8) === 'IDAT') compressed.push(png.subarray(offset + 8, offset + 8 + length));
      offset += length + 12;
    }
    const pixels = inflateSync(Buffer.concat(compressed));
    const px = Math.floor((tx % 1) * 256), py = Math.floor((ty % 1) * 256);
    assert.equal(pixels[py * 1025 + 1 + px * 4 + 3], inside ? 210 : 0, `${lat},${lon}`);
    checked++;
  }
}
console.log(`native topology PNG points checked: ${checked}`);
"#])
        .arg(&payload).output().unwrap();
    assert!(
        rendered.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&rendered.stdout),
        String::from_utf8_lossy(&rendered.stderr)
    );
    eprintln!("{}", String::from_utf8_lossy(&rendered.stdout));
}

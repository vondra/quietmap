//! Missing/stale data fail closed; interpolation preserves power and the longitude seam.
use super::*;
use grid::surface_corner::{SurfaceCorner, WORLD_BLOCK_SIDE};
use tile_painter::{
    corner_store::{CornerEnergy, CornerGeneration, SourceEnergy, SourceIdentity},
    generation_receipt::SURFACE_CODE_DIGEST,
};

fn prepare(root: &Path) -> (PathBuf, PathBuf, GenerationReceipt) {
    let prepared = root.join("prepared");
    let corners = root.join("corners");
    std::fs::create_dir(&prepared).unwrap();
    // These files are opaque externally pinned bytes to the reader; the producer validates their schemas.
    std::fs::write(
        prepared.join("inputs.sqlite"),
        b"vector manifest generation A",
    )
    .unwrap();
    let receipt = GenerationReceipt {
        sources: file_digest(&prepared.join("inputs.sqlite")).unwrap(),
        code: SURFACE_CODE_DIGEST,
        producer: [4; 32],
    };
    (prepared, corners, receipt)
}
fn midpoint(corner: SurfaceCorner) -> [f64; 2] {
    let [x, y] = corner.coordinates();
    let side = f64::from(WORLD_BLOCK_SIDE);
    let lon = (f64::from(x) + 0.5) / side * 360.0 - 180.0;
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * (f64::from(y) + 0.5) / side))
        .sinh()
        .atan()
        .to_degrees();
    [lat, lon]
}
fn fill(
    root: &Path,
    generation: CornerGeneration,
    vertices: &[SurfaceCorner],
    all: &[SurfaceCorner; 4],
) {
    CornerDirectory::new(root, generation)
        .resolve(vertices, |_, missing| {
            Ok(missing
                .iter()
                .map(|corner| {
                    let index = all.iter().position(|value| value == corner).unwrap();
                    let base = [
                        [1.0, 4.0, 9.0],
                        [9.0, 16.0, 25.0],
                        [25.0, 36.0, 49.0],
                        [49.0, 64.0, 81.0],
                    ][index];
                    CornerEnergy(
                        (0..5)
                            .map(|layer| SourceEnergy {
                                layer,
                                source: SourceIdentity::arrow_row([2; 32], u64::from(layer), 0),
                                periods: base.map(|power| power * f32::from(layer + 1)),
                            })
                            .collect(),
                    )
                })
                .collect())
        })
        .unwrap();
}

#[test]
fn preview_interpolates_linear_power_across_the_dateline_and_labels_every_limit() {
    let temp = tempfile::tempdir().unwrap();
    let (prepared, root, receipt) = prepare(temp.path());
    receipt.publish(&root).unwrap();
    let center = midpoint(SurfaceCorner::new(WORLD_BLOCK_SIDE - 1, WORLD_BLOCK_SIDE / 2).unwrap());
    let lattice = SurfaceCornerInterpolation::at(center[0], center[1]).unwrap();
    assert_eq!(lattice.corners[0].owner().x, 511);
    assert_eq!(lattice.corners[1].owner().x, 0);
    fill(
        &root,
        receipt.generation(),
        &lattice.corners,
        &lattice.corners,
    );
    let reader = SurfaceCornerReader::open(&prepared, &root).unwrap();
    let preview = reader.query(center[0], center[1]).unwrap();
    let wrapped = reader.query(center[0], center[1] - 360.0).unwrap();
    assert_eq!(preview.center, center);
    assert_eq!(
        (preview.status, preview.receiver, preview.accuracy),
        ("provisional", "outdoor", "unmeasured")
    );
    for (layer, value) in preview.layers.iter().enumerate() {
        for (actual, expected) in value.period_power.iter().zip([21.0, 30.0, 41.0]) {
            assert!(
                (actual - expected * (layer + 1) as f64).abs() < 1e-5,
                "actual={actual} expected={expected} weights={:?}",
                lattice.weights
            );
        }
        assert_eq!(value.period_power, wrapped.layers[layer].period_power);
    }
    assert!(reader.query(90.0, center[1]).is_none());
    assert!(reader.query(f64::NAN, center[1]).is_none());
    let json = serde_json::to_value(preview).unwrap();
    assert_eq!(json["layers"].as_array().unwrap().len(), 5);
    assert!(json.get("total").is_none());
}

#[test]
fn missing_or_different_generation_corners_return_none_without_creating_data() {
    let temp = tempfile::tempdir().unwrap();
    let (prepared, root, receipt) = prepare(temp.path());
    assert!(SurfaceCornerReader::open(&prepared, &root).is_none());
    assert!(!root.exists());
    receipt.publish(&root).unwrap();
    let center = midpoint(SurfaceCorner::new(1000, 1000).unwrap());
    let vertices = SurfaceCornerInterpolation::at(center[0], center[1])
        .unwrap()
        .corners;
    fill(&root, receipt.generation(), &vertices[..3], &vertices);
    let reader = SurfaceCornerReader::open(&prepared, &root).unwrap();
    assert!(reader.query(center[0], center[1]).is_none());
    let stale_root = temp.path().join("stale-corners");
    receipt.publish(&stale_root).unwrap();
    fill(&stale_root, CornerGeneration([9; 32]), &vertices, &vertices);
    let stale = SurfaceCornerReader::open(&prepared, &stale_root).unwrap();
    assert!(stale.query(center[0], center[1]).is_none());
}

#[test]
fn replaced_prepared_manifest_disables_existing_and_new_readers() {
    let temp = tempfile::tempdir().unwrap();
    let (prepared, root, receipt) = prepare(temp.path());
    receipt.publish(&root).unwrap();
    let center = midpoint(SurfaceCorner::new(1000, 1000).unwrap());
    let vertices = SurfaceCornerInterpolation::at(center[0], center[1])
        .unwrap()
        .corners;
    fill(&root, receipt.generation(), &vertices, &vertices);
    let reader = SurfaceCornerReader::open(&prepared, &root).unwrap();
    assert!(reader.query(center[0], center[1]).is_some());
    let manifest = prepared.join("inputs.sqlite");
    let modified = std::fs::metadata(&manifest).unwrap().modified().unwrap();
    let replacement = prepared.join("replacement.sqlite");
    std::fs::write(&replacement, b"vector manifest generation B").unwrap();
    std::fs::File::open(&replacement)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    std::fs::rename(replacement, manifest).unwrap();
    assert!(reader.query(center[0], center[1]).is_none());
    assert!(SurfaceCornerReader::open(&prepared, &root).is_none());
}

//! Continuity, orientation and malformed-input regressions for the climatology reader.
use super::*;

fn field(x: usize, y: usize) -> MeteorologyNode {
    let mut node = MeteorologyNode::default();
    for k in 0..3 {
        for s in 0..SECTORS {
            node.p_percent[k][s] = ((x + y + s + k) % 101) as u8;
        }
        for b in 0..8 {
            node.alpha_mean[k][b] = (x + y + b + 1) as f32;
            node.alpha_variance[k][b] = (x + 2 * y + b) as f32;
        }
    }
    node
}

#[test]
fn probabilities_and_moments_are_continuous_at_grid_sector_and_square_edges() {
    let eps: f64 = 1e-8;
    // ERA5 node lines, z9 square edges, and both longitude wrapping seams.
    for (lat, lon) in [
        (50., 14.25),
        (50., 14.0625),
        (0., 0.),
        (0., 180.),
        (-90., 40.),
        (90., 40.),
    ] {
        let a = sample_at((lat - eps).clamp(-90., 90.), lon - eps, field).unwrap();
        let b = sample_at((lat + eps).clamp(-90., 90.), lon + eps, field).unwrap();
        for k in 0..3 {
            for s in 0..SECTORS {
                let angle = s as f64 * 22.5;
                assert!(
                    (a.probability(k, angle - eps).unwrap()
                        - b.probability(k, angle + eps).unwrap())
                    .abs()
                        < 1e-5
                );
            }
            for band in 0..8 {
                assert!((a.alpha_mean[k][band] - b.alpha_mean[k][band]).abs() < 1e-3);
                assert!((a.alpha_variance[k][band] - b.alpha_variance[k][band]).abs() < 1e-3);
            }
        }
    }
    let at = sample_at(50., 14.25, field).unwrap();
    assert_eq!(at.alpha_mean[0][0], field(57, 160).alpha_mean[0][0]);
    assert_eq!(
        at.probability(0, 0.).unwrap(),
        at.probability(0, 360.).unwrap()
    );
    assert!((at.probability(0, 11.25).unwrap() - (at.p[0][0] + at.p[0][1]) / 2.).abs() < 1e-7);
    assert_eq!(
        at.probability(0, -1e-20).unwrap(),
        at.probability(0, 0.).unwrap()
    );
    assert!(at.probability(3, 0.).is_err());
    assert!(at.probability(0, f64::NAN).is_err());
    assert!(sample_at(91., 0., field).is_err());
    assert!(sample_at(0., f64::INFINITY, field).is_err());
    assert_eq!(std::mem::size_of::<MeteorologyNode>(), 240);
}

#[test]
fn missing_and_incomplete_climatologies_are_errors() {
    use arrow::{datatypes::Schema, ipc::writer::FileWriter};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("meteorology.arrow");
    assert!(Meteorology::load(&path).is_err());
    let schema = Schema::empty();
    FileWriter::try_new(File::create(&path).unwrap(), &schema)
        .unwrap()
        .finish()
        .unwrap();
    assert!(Meteorology::load(&path).err().unwrap().contains("metadata"));
    let mut metadata: std::collections::HashMap<String, String> =
        serde_json::from_str(CONTRACT).unwrap();
    metadata.insert("complete".into(), "true".into());
    metadata.insert("source_identity".into(), "synthetic".into());
    let schema = schema.with_metadata(metadata);
    FileWriter::try_new(File::create(&path).unwrap(), &schema)
        .unwrap()
        .finish()
        .unwrap();
    assert!(Meteorology::load(&path)
        .err()
        .unwrap()
        .contains("incomplete global"));
}

#[test]
fn populated_rows_reject_null_nonphysical_and_reordered_values() {
    use arrow::{
        array::ArrayRef,
        datatypes::{Float32Type, UInt8Type},
        ipc::writer::FileWriter,
        record_batch::RecordBatch,
    };
    use std::sync::Arc;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("meteorology.arrow");
    for (fault, message) in [
        ("null", "null child"),
        ("p", "outside 0..100"),
        ("mean", "nonfinite or negative"),
        ("variance", "nonfinite or negative"),
        ("order", "row-major"),
        ("maximum", "p_max disagrees"),
    ] {
        let mut columns: Vec<(String, ArrayRef)> = vec![
            (
                "x".into(),
                Arc::new(UInt16Array::from(vec![u16::from(fault == "order")])),
            ),
            ("y".into(), Arc::new(UInt16Array::from(vec![0u16]))),
        ];
        for period in ["day", "evening", "night"] {
            let mut probabilities = vec![Some(if fault == "p" { 101u8 } else { 50u8 }); SECTORS];
            if fault == "null" {
                probabilities[0] = None;
            }
            columns.push((
                format!("p_{period}"),
                Arc::new(FixedSizeListArray::from_iter_primitive::<UInt8Type, _, _>(
                    [Some(probabilities)],
                    SECTORS as i32,
                )),
            ));
            columns.push((
                format!("p_max_{period}"),
                Arc::new(UInt8Array::from(vec![if fault == "maximum" {
                    49u8
                } else {
                    50u8
                }])),
            ));
            for (prefix, value) in [
                ("alpha_mean", if fault == "mean" { f32::NAN } else { 1. }),
                ("alpha_variance", if fault == "variance" { -1. } else { 0. }),
            ] {
                columns.push((
                    format!("{prefix}_{period}"),
                    Arc::new(
                        FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
                            [Some(vec![Some(value); 8])],
                            8,
                        ),
                    ),
                ));
            }
        }
        let batch = RecordBatch::try_from_iter(columns).unwrap();
        let mut metadata: std::collections::HashMap<String, String> =
            serde_json::from_str(CONTRACT).unwrap();
        metadata.insert("complete".into(), "true".into());
        metadata.insert("source_identity".into(), "synthetic".into());
        let schema = Arc::new(batch.schema().as_ref().clone().with_metadata(metadata));
        let batch = RecordBatch::try_new(schema.clone(), batch.columns().to_vec()).unwrap();
        let mut writer = FileWriter::try_new(File::create(&path).unwrap(), &schema).unwrap();
        writer.write(&batch).unwrap();
        writer.finish().unwrap();
        let error = Meteorology::load(&path).err().unwrap();
        assert!(error.contains(message), "{fault}: {error}");
    }
}

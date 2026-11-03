//! Producer-to-reader regressions for controls, structure extents and new source evidence.

use super::*;
use crate::osm::{
    classify::{FeatureType, Tags},
    model_nodes::{ControlPoints, ModelNode},
    spill::Spiller,
};
use arrow_array::*;
use arrow_ipc::reader::FileReader;

fn tags(pairs: &[(&str, &str)]) -> Tags {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}
fn fixture(
    name: &str,
    write: impl FnOnce(&mut Spiller),
) -> (std::path::PathBuf, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!("osm-evidence-{}-{name}", std::process::id()));
    let spill = root.join("spill");
    let out = root.join("prepared");
    let speeds = serde_json::from_value(
        serde_json::json!({"rows":[{"token":"de:urban","speed":50}],"aliases":{},"zone_units":{}}),
    )
    .unwrap();
    let mut writer = Spiller::new(&spill, 1, speeds, &[]).unwrap();
    write(&mut writer);
    crate::osm::transport::TransportSpill::new(&spill)
        .unwrap()
        .finish()
        .unwrap();
    writer.finish().unwrap();
    drop(writer);
    finalize(&spill, &out, 1, &[]).unwrap();
    (
        root,
        out.join(crate::osm::geometry::square_name(
            crate::osm::geometry::square_of(50.0, 14.0),
        )),
    )
}
fn read(path: &Path) -> Vec<RecordBatch> {
    FileReader::try_new(File::open(path).unwrap(), None)
        .unwrap()
        .map(Result::unwrap)
        .collect()
}
fn evidence_tags<'a>(batch: &'a RecordBatch, family: &str, row: usize) -> Result<&'a str> {
    crate::osm::contracts::validate(&batch.schema(), family).unwrap();
    Ok(batch
        .column_by_name("osm_tags")
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap()
        .value(row))
}

struct ControlPoint<'a> {
    node_id: i64,
    way_id: Option<i64>,
    family: &'a str,
    vertex_index: Option<u32>,
    way_m: Option<f64>,
    tags_json: &'a str,
}

fn control_points(batch: &RecordBatch) -> Result<Vec<ControlPoint<'_>>> {
    use crate::dev4::column;
    let id = column::<Int64Array>(batch, "osm_id").unwrap();
    let way = column::<Int64Array>(batch, "way_id").unwrap();
    let family = column::<StringArray>(batch, "family").unwrap();
    let vertex = column::<UInt32Array>(batch, "vertex_index").unwrap();
    let metres = column::<Float64Array>(batch, "way_m").unwrap();
    let tags = column::<StringArray>(batch, "osm_tags").unwrap();
    Ok((0..batch.num_rows())
        .map(|row| ControlPoint {
            node_id: id.value(row),
            way_id: (!way.is_null(row)).then(|| way.value(row)),
            family: family.value(row),
            vertex_index: (!vertex.is_null(row)).then(|| vertex.value(row)),
            way_m: (!metres.is_null(row)).then(|| metres.value(row)),
            tags_json: tags.value(row),
        })
        .collect())
}

mod sites;
mod transport;

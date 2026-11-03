//! Shared typed Arrow assertions and manifest-driven PBF test execution.

use super::pbf_fixture::NODES;
use arrow_array::*;
use arrow_ipc::reader::FileReader;
use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
};
/// Process-unique temporary directory removed on drop.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        static SEQUENCE: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "qm-source-topology-pbf-{tag}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).expect("create temporary directory");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn all_rows(path: &Path) -> RecordBatch {
    let reader =
        FileReader::try_new(File::open(path).expect("open arrow file"), None).expect("read IPC");
    let schema = reader.schema();
    let batches: Vec<RecordBatch> = reader.map(|batch| batch.expect("decode batch")).collect();
    arrow_select::concat::concat_batches(&schema, &batches).expect("concatenate batches")
}

pub fn column<'a, T: 'static>(batch: &'a RecordBatch, name: &str) -> &'a T {
    batch
        .column_by_name(name)
        .unwrap_or_else(|| panic!("column {name}"))
        .as_any()
        .downcast_ref::<T>()
        .unwrap_or_else(|| panic!("type of {name}"))
}

pub fn list_values<T: Array + Clone + 'static>(batch: &RecordBatch, name: &str, row: usize) -> T {
    let list = column::<ListArray>(batch, name).value(row);
    list.as_any()
        .downcast_ref::<T>()
        .expect("list item type")
        .clone()
}

pub fn e7(degrees: f64) -> i32 {
    (degrees * 1e7).round() as i32
}

/// Expected canonical id and e7 coordinates; only the zero-length hop 3→7 (way 13) unions, so
/// node 7 is written as 3 — the coordinate twin 4 (way 11) never touches 2.
pub fn expected_node(id: i64) -> (i64, Option<(i32, i32)>) {
    let coordinates = NODES
        .iter()
        .find(|&&(node_id, _, _)| node_id == id)
        .map(|&(_, lat, lon)| (e7(lat), e7(lon)));
    (if id == 7 { 3 } else { id }, coordinates)
}

pub fn run_extract(root: &Path, tag: &str, input: &Path) -> anyhow::Result<PathBuf> {
    run_extract_squares(
        root,
        tag,
        input,
        &[
            crate::dev4::Square { x: 275, y: 173 },
            crate::dev4::Square { x: 276, y: 173 },
        ],
    )
}

pub fn run_extract_squares(
    root: &Path,
    tag: &str,
    input: &Path,
    squares: &[crate::dev4::Square],
) -> anyhow::Result<PathBuf> {
    let output = root.join(tag);
    let boundary = root.join("regions.json");
    std::fs::write(
        &boundary,
        r#"{"regions":[{"country":"XY","continent":1,"disputed":false,"polygons":[[[[13,49],[15,49],[15,51],[13,51],[13,49]]]]}]}"#,
    )?;
    let manifest = root.join("manifest.json");
    let source_root = root.join("2000");
    std::fs::create_dir_all(&source_root)?;
    std::fs::copy(input, source_root.join("prepare-osm"))?;
    std::fs::copy(&boundary, source_root.join("prepare-regions"))?;
    let entry = |kind: &str| {
        serde_json::json!({
            "id":kind,"kind":kind,"region":"world","publisher":"fixture","url":"https://example.org",
            "observed":{"start":"2000-01-01","end":"2000-01-01"},"published":null,"depends_on":[],"licence":"test","public_ok":true,"description":"test input"
        })
    };
    std::fs::write(
        &manifest,
        serde_json::to_vec(&serde_json::json!({
            "year":"2000","built":false,"as_of":"2000-01-02","flows":{"start":"2000-01-01","end":"2000-01-02"},
            "inputs":[entry("prepare-osm"),entry("prepare-regions")],
            "unavailable":[{"id":"speeds","kind":"prepare-osm-speeds","region":"world","description":"test explicit OSM only","depends_on":[]}]
        }))?,
    )?;
    let inputs = crate::year::YearInputs::read(&manifest, root).map_err(anyhow::Error::msg)?;
    super::build(
        &inputs,
        squares,
        &output,
        &root.join(format!("scratch-{tag}")),
    )?;
    Ok(output)
}

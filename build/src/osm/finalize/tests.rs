//! End-to-end source spill and Arrow contract regressions.

use super::*;
use arrow_array::{BinaryArray, BooleanArray, Int32Array, Int64Array, StringArray, UInt8Array};
use arrow_ipc::reader::FileReader;

fn prague_ring_text() -> String {
    [
        (14.0, 50.0),
        (14.001_394, 50.0),
        (14.001_394, 50.000_904),
        (14.0, 50.000_904),
    ]
    .iter()
    .map(|&(lon, lat)| {
        let (gx, gy) = crate::osm::geometry::lonlat_to_grid(lon, lat);
        format!("{gx},{gy}")
    })
    .collect::<Vec<_>>()
    .join(";")
}

fn scratch_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("osm-extract-test-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn read_ipc(path: &Path) -> (Schema, Vec<arrow_array::RecordBatch>) {
    let file = File::open(path).unwrap();
    let reader = FileReader::try_new(file, None).unwrap();
    let schema = reader.schema().as_ref().clone();
    let batches: Vec<_> = reader.collect::<Result<_, _>>().unwrap();
    (schema, batches)
}

mod buildings;
mod rows;

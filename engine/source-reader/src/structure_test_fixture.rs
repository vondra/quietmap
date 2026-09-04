//! Test double for `scripts/structures/build-structures.py`: writes the
//! structures_v1 per-cell table (kind-tagged buildings ∪ walls) that the
//! popup readers under test consume, with the contract metadata
//! `hex_store::load_hex` gates on. The writer's schema is mirrored HERE, once,
//! so a column rename breaks every reader test in one place.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{
    ArrayRef, BinaryArray, Float32Array, Float64Array, Int16Array, Int64Array, StringArray,
    UInt16Array, UInt32Array, UInt8Array,
};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::ipc::writer::FileWriter;
use arrow::record_batch::RecordBatch;
use h3o::CellIndex;

/// One structures.arrow row; unset fields write null (the schema's nullable
/// columns) or the zero value (the non-nullable ones), matching the builder.
#[derive(Default, Clone)]
pub struct StructureRow {
    pub kind: u8, // crate::hex_store::STRUCTURE_KIND_*
    pub geometry_wkb: Option<Vec<u8>>,
    pub height_m: f32,
    pub height_tier: u8,
    pub envelope_class: u8,
    pub centroid_lat: f64,
    pub centroid_lon: f64,
    pub osm_id: Option<i64>,
    pub building_type: Option<u8>,
    pub building_use: Option<u8>,
    pub height: Option<f32>,
    pub floors: Option<u8>,
    pub name: Option<String>,
    pub addr_street: Option<String>,
    pub addr_housenumber: Option<String>,
    pub area_m2: Option<f32>,
    pub opening_hours_frac: Option<u8>,
    pub source_id: Option<u16>,
    pub emission_polygon_wkb: Option<Vec<u8>>,
    pub emission_centroid_lat: Option<f64>,
    pub emission_centroid_lon: Option<f64>,
    pub segment_idx: Option<i16>,
    /// Null falls back to the row index — the fixtures don't care about the
    /// specific order, only that geometry rows carry a dense sequence (the
    /// builder's invariant).
    pub screening_ordinal: Option<u32>,
}

fn structure_schema(with_contract: bool) -> Schema {
    let fields = vec![
        Field::new("kind", DataType::UInt8, false),
        Field::new("geometry_wkb", DataType::Binary, true),
        Field::new("height_m", DataType::Float32, false),
        Field::new("height_tier", DataType::UInt8, false),
        Field::new("envelope_class", DataType::UInt8, false),
        Field::new("centroid_lat", DataType::Float64, false),
        Field::new("centroid_lon", DataType::Float64, false),
        Field::new("osm_id", DataType::Int64, true),
        Field::new("building_type", DataType::UInt8, true),
        Field::new("building_use", DataType::UInt8, true),
        Field::new("height", DataType::Float32, true),
        Field::new("floors", DataType::UInt8, true),
        Field::new("name", DataType::Utf8, true),
        Field::new("addr_street", DataType::Utf8, true),
        Field::new("addr_housenumber", DataType::Utf8, true),
        Field::new("area_m2", DataType::Float32, true),
        Field::new("opening_hours_frac", DataType::UInt8, true),
        Field::new("source_id", DataType::UInt16, true),
        Field::new("emission_polygon_wkb", DataType::Binary, true),
        Field::new("emission_centroid_lat", DataType::Float64, true),
        Field::new("emission_centroid_lon", DataType::Float64, true),
        Field::new("segment_idx", DataType::Int16, true),
        Field::new("screening_ordinal", DataType::UInt32, true),
    ];
    let mut metadata = std::collections::HashMap::new();
    if with_contract {
        metadata.insert(
            "structures_contract".to_string(),
            crate::hex_store::STRUCTURES_CONTRACT_V1.to_string(),
        );
    }
    Schema::new(fields).with_metadata(metadata)
}

fn structure_columns(rows: &[StructureRow]) -> Vec<ArrayRef> {
    vec![
        Arc::new(UInt8Array::from_iter_values(rows.iter().map(|r| r.kind))),
        Arc::new(BinaryArray::from_iter(
            rows.iter().map(|r| r.geometry_wkb.as_ref()),
        )),
        Arc::new(Float32Array::from_iter_values(
            rows.iter().map(|r| r.height_m),
        )),
        Arc::new(UInt8Array::from_iter_values(
            rows.iter().map(|r| r.height_tier),
        )),
        Arc::new(UInt8Array::from_iter_values(
            rows.iter().map(|r| r.envelope_class),
        )),
        Arc::new(Float64Array::from_iter_values(
            rows.iter().map(|r| r.centroid_lat),
        )),
        Arc::new(Float64Array::from_iter_values(
            rows.iter().map(|r| r.centroid_lon),
        )),
        Arc::new(Int64Array::from_iter(rows.iter().map(|r| r.osm_id))),
        Arc::new(UInt8Array::from_iter(rows.iter().map(|r| r.building_type))),
        Arc::new(UInt8Array::from_iter(rows.iter().map(|r| r.building_use))),
        Arc::new(Float32Array::from_iter(rows.iter().map(|r| r.height))),
        Arc::new(UInt8Array::from_iter(rows.iter().map(|r| r.floors))),
        Arc::new(StringArray::from_iter(rows.iter().map(|r| r.name.as_ref()))),
        Arc::new(StringArray::from_iter(
            rows.iter().map(|r| r.addr_street.as_ref()),
        )),
        Arc::new(StringArray::from_iter(
            rows.iter().map(|r| r.addr_housenumber.as_ref()),
        )),
        Arc::new(Float32Array::from_iter(rows.iter().map(|r| r.area_m2))),
        Arc::new(UInt8Array::from_iter(
            rows.iter().map(|r| r.opening_hours_frac),
        )),
        Arc::new(UInt16Array::from_iter(rows.iter().map(|r| r.source_id))),
        Arc::new(BinaryArray::from_iter(
            rows.iter().map(|r| r.emission_polygon_wkb.as_ref()),
        )),
        Arc::new(Float64Array::from_iter(
            rows.iter().map(|r| r.emission_centroid_lat),
        )),
        Arc::new(Float64Array::from_iter(
            rows.iter().map(|r| r.emission_centroid_lon),
        )),
        Arc::new(Int16Array::from_iter(rows.iter().map(|r| r.segment_idx))),
        Arc::new(UInt32Array::from_iter(
            rows.iter()
                .enumerate()
                .map(|(i, r)| r.screening_ordinal.or(Some(i as u32))),
        )),
    ]
}

/// One batch in the v1 layout — the readers' own batching is orthogonal to
/// what the tests assert, so a single batch is the honest shape.
pub fn structure_batch(rows: &[StructureRow]) -> RecordBatch {
    RecordBatch::try_new(Arc::new(structure_schema(true)), structure_columns(rows)).unwrap()
}

/// A structures.arrow on disk; `with_contract: false` writes the same rows
/// without the contract stamp (the `load_hex` gate test's case).
pub fn write_structure_file(path: &Path, rows: &[StructureRow], with_contract: bool) {
    let schema = Arc::new(structure_schema(with_contract));
    let batch = RecordBatch::try_new(schema.clone(), structure_columns(rows)).unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut w = FileWriter::try_new(file, &schema).unwrap();
    w.write(&batch).unwrap();
    w.finish().unwrap();
}

/// A cell's `structures.arrow` under the prepared-tree layout.
pub fn write_structure_table(h3r4_dir: &Path, cell: CellIndex, rows: &[StructureRow]) -> PathBuf {
    let dir = h3r4_dir.join(cell.to_string());
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(noise_compute::propagation::structure_cell_file::CELL_STRUCTURE_FILENAME);
    write_structure_file(&path, rows, true);
    path
}

/// Closed square WKB Polygon (little-endian, 1 ring × 5 points) with its
/// south-west corner at (lat, lon) — the ~20 m footprint the store tests
/// build on.
pub fn square_polygon_wkb(lat: f64, lon: f64) -> Vec<u8> {
    let mut wkb = vec![1, 3, 0, 0, 0, 1, 0, 0, 0, 5, 0, 0, 0];
    for (dlon, dlat) in [
        (0.0, 0.0),
        (0.0003, 0.0),
        (0.0003, 0.0002),
        (0.0, 0.0002),
        (0.0, 0.0),
    ] {
        wkb.extend_from_slice(&f64::to_le_bytes(lon + dlon));
        wkb.extend_from_slice(&f64::to_le_bytes(lat + dlat));
    }
    wkb
}

/// Two-point WKB LineString (little-endian), endpoints as (lat, lon) — a wall
/// microsegment.
pub fn wall_linestring_wkb(start: (f64, f64), end: (f64, f64)) -> Vec<u8> {
    let mut wkb = vec![1, 2, 0, 0, 0, 2, 0, 0, 0];
    for (lat, lon) in [start, end] {
        wkb.extend_from_slice(&f64::to_le_bytes(lon));
        wkb.extend_from_slice(&f64::to_le_bytes(lat));
    }
    wkb
}

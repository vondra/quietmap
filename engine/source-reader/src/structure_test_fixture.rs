//! Test double for `scripts/structures/build-structures.py` + the osm-extract
//! finalizers: writes the structures_v4 per-square table (kind-tagged
//! buildings ∪ walls) and tiny road/rail/leisure/industrial arrows that the
//! popup readers under test consume, with the contract metadata
//! `square_store::store::load_square` gates on. Coordinates are lon/lat floats
//! at the fixture boundary and z30 grid cells on disk, like the real writers.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{
    ArrayRef, BinaryArray, BooleanArray, Float32Array, Int16Array, Int32Array, Int64Array,
    StringArray, UInt16Array, UInt32Array, UInt8Array,
};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::ipc::writer::FileWriter;
use arrow::record_batch::RecordBatch;

/// One structures.arrow row; unset fields write null (the schema's nullable
/// columns) or the zero value (the non-nullable ones), matching the builder.
/// Geometry is lon/lat here and snapped to z30 on write.
#[derive(Default, Clone)]
pub struct StructureRow {
    pub kind: u8, // square_store::store::STRUCTURE_KIND_*
    pub ring_lonlat: Option<Vec<(f64, f64)>>,
    pub height_m: i16,
    pub height_tier: u8,
    pub envelope_class: u8,
    pub centroid_lonlat: Option<(f64, f64)>,
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
    pub emission_ring_lonlat: Option<Vec<(f64, f64)>>,
    pub emission_centroid_lonlat: Option<(f64, f64)>,
    pub segment_idx: Option<i16>,
    /// Null falls back to the row index — the fixtures don't care about the
    /// specific order, only that geometry rows carry a dense sequence (the
    /// builder's invariant).
    pub screening_ordinal: Option<u32>,
}

fn grid_of(lon: f64, lat: f64) -> (i32, i32) {
    grid::lonlat_to_grid(lon, lat)
}

fn encode_ring(ring: &[(f64, f64)]) -> Vec<u8> {
    let grid: Vec<(i32, i32)> = ring.iter().map(|&(lon, lat)| grid_of(lon, lat)).collect();
    grid::poly::encode_grid_poly(&grid)
}

fn structure_schema(with_contract: bool) -> Schema {
    let fields = vec![
        Field::new("kind", DataType::UInt8, false),
        Field::new("geom", DataType::Binary, true),
        Field::new("height_m", DataType::Int16, false),
        Field::new("height_tier", DataType::UInt8, false),
        Field::new("envelope_class", DataType::UInt8, false),
        Field::new("centroid_gx", DataType::Int32, false),
        Field::new("centroid_gy", DataType::Int32, false),
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
        Field::new("emission_geom", DataType::Binary, true),
        Field::new("emission_centroid_gx", DataType::Int32, true),
        Field::new("emission_centroid_gy", DataType::Int32, true),
        Field::new("segment_idx", DataType::Int16, true),
        Field::new("screening_ordinal", DataType::UInt32, true),
    ];
    let mut metadata = std::collections::HashMap::new();
    if with_contract {
        metadata.insert(
            "structures_contract".to_string(),
            square_store::structure_contract::CONTRACT.to_string(),
        );
        metadata.insert(
            "grid".to_string(),
            square_store::store::GRID_CONTRACT_Z30.to_string(),
        );
    }
    Schema::new(fields).with_metadata(metadata)
}

fn structure_columns(rows: &[StructureRow]) -> Vec<ArrayRef> {
    let centroids: Vec<(i32, i32)> = rows
        .iter()
        .map(|r| {
            r.centroid_lonlat
                .map(|(lon, lat)| grid_of(lon, lat))
                .unwrap_or((0, 0))
        })
        .collect();
    let emission_centroids: Vec<Option<(i32, i32)>> = rows
        .iter()
        .map(|r| {
            r.emission_centroid_lonlat
                .map(|(lon, lat)| grid_of(lon, lat))
        })
        .collect();
    vec![
        Arc::new(UInt8Array::from_iter_values(rows.iter().map(|r| r.kind))),
        Arc::new(BinaryArray::from_iter(rows.iter().map(|r| {
            r.ring_lonlat.as_ref().map(|ring| {
                if r.kind == square_store::store::STRUCTURE_KIND_BARRIER {
                    encode_ring(ring)
                } else {
                    let ring = ring.iter().map(|&(lon, lat)| grid_of(lon, lat)).collect();
                    grid::poly::encode_grid_polygons(&[vec![ring]])
                }
            })
        }))),
        Arc::new(Int16Array::from_iter_values(
            rows.iter().map(|r| r.height_m),
        )),
        Arc::new(UInt8Array::from_iter_values(
            rows.iter().map(|r| r.height_tier),
        )),
        Arc::new(UInt8Array::from_iter_values(
            rows.iter().map(|r| r.envelope_class),
        )),
        Arc::new(Int32Array::from_iter_values(
            centroids.iter().map(|(gx, _)| *gx),
        )),
        Arc::new(Int32Array::from_iter_values(
            centroids.iter().map(|(_, gy)| *gy),
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
        Arc::new(BinaryArray::from_iter(rows.iter().map(|r| {
            r.emission_ring_lonlat
                .as_ref()
                .or_else(|| r.osm_id.and(r.ring_lonlat.as_ref()))
                .map(|ring| encode_ring(ring))
        }))),
        Arc::new(Int32Array::from_iter(
            emission_centroids.iter().map(|c| c.map(|(gx, _)| gx)),
        )),
        Arc::new(Int32Array::from_iter(
            emission_centroids.iter().map(|c| c.map(|(_, gy)| gy)),
        )),
        Arc::new(Int16Array::from_iter(rows.iter().map(|r| r.segment_idx))),
        Arc::new(UInt32Array::from_iter(
            rows.iter()
                .enumerate()
                .map(|(i, r)| r.screening_ordinal.or(Some(i as u32))),
        )),
    ]
}

/// One batch in the current layout — the readers' own batching is orthogonal to
/// what the tests assert, so a single batch is the honest shape.
pub fn structure_batch(rows: &[StructureRow]) -> RecordBatch {
    RecordBatch::try_new(Arc::new(structure_schema(true)), structure_columns(rows)).unwrap()
}

/// A structures.arrow on disk; `with_contract: false` writes the same rows
/// without the contract stamps (the `load_square` gate test's case).
pub fn write_structure_file(path: &Path, rows: &[StructureRow], with_contract: bool) {
    let schema = Arc::new(structure_schema(with_contract));
    let batch = RecordBatch::try_new(schema.clone(), structure_columns(rows)).unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut w = FileWriter::try_new(file, &schema).unwrap();
    w.write(&batch).unwrap();
    w.finish().unwrap();
}

/// Closed square ring (lon/lat) with its south-west corner at (lat, lon) —
/// the ~20 m footprint the store tests build on.
pub fn square_ring_lonlat(lat: f64, lon: f64) -> Vec<(f64, f64)> {
    vec![
        (lon, lat),
        (lon + 0.0003, lat),
        (lon + 0.0003, lat + 0.0002),
        (lon, lat + 0.0002),
        (lon, lat),
    ]
}

/// One road microsegment row: lon/lat endpoints + classification.
pub struct FixtureRoad {
    pub osm_id: i64,
    pub start: (f64, f64),
    pub end: (f64, f64),
    pub road_class: u8,
    pub speed_limit: u8,
    pub lanes: u8,
    pub name: String,
}

fn roads_schema() -> Schema {
    Schema::new(vec![
        Field::new("osm_id", DataType::Int64, false),
        Field::new("segment_idx", DataType::Int16, false),
        Field::new("start_gx", DataType::Int32, false),
        Field::new("start_gy", DataType::Int32, false),
        Field::new("end_gx", DataType::Int32, false),
        Field::new("end_gy", DataType::Int32, false),
        Field::new("length_m", DataType::Float32, false),
        Field::new("road_class", DataType::UInt8, false),
        Field::new("speed_limit", DataType::UInt8, false),
        Field::new("surface_type", DataType::UInt8, false),
        Field::new("oneway", DataType::Boolean, false),
        Field::new("lanes", DataType::UInt8, false),
        Field::new("name", DataType::Utf8, true),
        Field::new("ref", DataType::Utf8, true),
        Field::new("bridge", DataType::Boolean, false),
        Field::new("tunnel", DataType::Boolean, false),
        Field::new("toll", DataType::Boolean, false),
        Field::new("lit", DataType::UInt8, false),
        Field::new("junction", DataType::UInt8, false),
        Field::new("access", DataType::UInt8, false),
        Field::new("source_id", DataType::UInt16, false),
    ])
}

/// A roads.arrow on disk in the osm-extract v2 (grid) layout.
pub fn write_roads_file(path: &Path, rows: &[FixtureRoad]) {
    let schema = Arc::new(roads_schema());
    let starts: Vec<(i32, i32)> = rows.iter().map(|r| grid_of(r.start.0, r.start.1)).collect();
    let ends: Vec<(i32, i32)> = rows.iter().map(|r| grid_of(r.end.0, r.end.1)).collect();
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(Int64Array::from_iter_values(rows.iter().map(|r| r.osm_id))),
            Arc::new(Int16Array::from_iter_values(rows.iter().map(|_| 0i16))),
            Arc::new(Int32Array::from_iter_values(
                starts.iter().map(|(gx, _)| *gx),
            )),
            Arc::new(Int32Array::from_iter_values(
                starts.iter().map(|(_, gy)| *gy),
            )),
            Arc::new(Int32Array::from_iter_values(ends.iter().map(|(gx, _)| *gx))),
            Arc::new(Int32Array::from_iter_values(ends.iter().map(|(_, gy)| *gy))),
            Arc::new(Float32Array::from_iter_values(rows.iter().map(|_| 0.0f32))),
            Arc::new(UInt8Array::from_iter_values(
                rows.iter().map(|r| r.road_class),
            )),
            Arc::new(UInt8Array::from_iter_values(
                rows.iter().map(|r| r.speed_limit),
            )),
            Arc::new(UInt8Array::from_iter_values(rows.iter().map(|_| 0u8))),
            Arc::new(BooleanArray::from(vec![false; rows.len()])),
            Arc::new(UInt8Array::from_iter_values(rows.iter().map(|r| r.lanes))),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.name.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(rows.iter().map(|_| ""))),
            Arc::new(BooleanArray::from(vec![false; rows.len()])),
            Arc::new(BooleanArray::from(vec![false; rows.len()])),
            Arc::new(BooleanArray::from(vec![false; rows.len()])),
            Arc::new(UInt8Array::from_iter_values(rows.iter().map(|_| 0u8))),
            Arc::new(UInt8Array::from_iter_values(rows.iter().map(|_| 0u8))),
            Arc::new(UInt8Array::from_iter_values(rows.iter().map(|_| 0u8))),
            Arc::new(UInt16Array::from_iter_values(rows.iter().map(|_| 0u16))),
        ],
    )
    .unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut w = FileWriter::try_new(file, &schema).unwrap();
    w.write(&batch).unwrap();
    w.finish().unwrap();
}

/// One rail microsegment row.
pub struct FixtureRail {
    pub osm_id: i64,
    pub start: (f64, f64),
    pub end: (f64, f64),
    pub rail_type: u8,
    pub maxspeed: u16,
}

/// A railways.arrow on disk in the osm-extract v2 (grid) layout.
pub fn write_railways_file(path: &Path, rows: &[FixtureRail]) {
    let schema = Arc::new(Schema::new(vec![
        Field::new("osm_id", DataType::Int64, false),
        Field::new("segment_idx", DataType::Int16, false),
        Field::new("start_gx", DataType::Int32, false),
        Field::new("start_gy", DataType::Int32, false),
        Field::new("end_gx", DataType::Int32, false),
        Field::new("end_gy", DataType::Int32, false),
        Field::new("length_m", DataType::Float32, false),
        Field::new("rail_type", DataType::UInt8, false),
        Field::new("usage", DataType::UInt8, false),
        Field::new("maxspeed", DataType::UInt16, false),
        Field::new("name", DataType::Utf8, true),
        Field::new("ref", DataType::Utf8, true),
        Field::new("electrified", DataType::UInt8, false),
        Field::new("gauge", DataType::UInt16, false),
        Field::new("bridge", DataType::Boolean, false),
        Field::new("tunnel", DataType::Boolean, false),
        Field::new("highspeed", DataType::Boolean, false),
        Field::new("service", DataType::UInt8, false),
        Field::new("source_id", DataType::UInt16, false),
    ]));
    let starts: Vec<(i32, i32)> = rows.iter().map(|r| grid_of(r.start.0, r.start.1)).collect();
    let ends: Vec<(i32, i32)> = rows.iter().map(|r| grid_of(r.end.0, r.end.1)).collect();
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(Int64Array::from_iter_values(rows.iter().map(|r| r.osm_id))),
            Arc::new(Int16Array::from_iter_values(rows.iter().map(|_| 0i16))),
            Arc::new(Int32Array::from_iter_values(
                starts.iter().map(|(gx, _)| *gx),
            )),
            Arc::new(Int32Array::from_iter_values(
                starts.iter().map(|(_, gy)| *gy),
            )),
            Arc::new(Int32Array::from_iter_values(ends.iter().map(|(gx, _)| *gx))),
            Arc::new(Int32Array::from_iter_values(ends.iter().map(|(_, gy)| *gy))),
            Arc::new(Float32Array::from_iter_values(rows.iter().map(|_| 0.0f32))),
            Arc::new(UInt8Array::from_iter_values(
                rows.iter().map(|r| r.rail_type),
            )),
            Arc::new(UInt8Array::from_iter_values(rows.iter().map(|_| 0u8))),
            Arc::new(UInt16Array::from_iter_values(
                rows.iter().map(|r| r.maxspeed),
            )),
            Arc::new(StringArray::from_iter_values(rows.iter().map(|_| ""))),
            Arc::new(StringArray::from_iter_values(rows.iter().map(|_| ""))),
            Arc::new(UInt8Array::from_iter_values(rows.iter().map(|_| 0u8))),
            Arc::new(UInt16Array::from_iter_values(rows.iter().map(|_| 0u16))),
            Arc::new(BooleanArray::from(vec![false; rows.len()])),
            Arc::new(BooleanArray::from(vec![false; rows.len()])),
            Arc::new(BooleanArray::from(vec![false; rows.len()])),
            Arc::new(UInt8Array::from_iter_values(rows.iter().map(|_| 0u8))),
            Arc::new(UInt16Array::from_iter_values(rows.iter().map(|_| 0u16))),
        ],
    )
    .unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut w = FileWriter::try_new(file, &schema).unwrap();
    w.write(&batch).unwrap();
    w.finish().unwrap();
}

/// One leisure row.
pub struct FixtureLeisure {
    pub osm_id: i64,
    pub centroid: (f64, f64),
    pub sport: u8,
    pub name: String,
}

/// A leisure.arrow on disk in the v2 (grid) layout, with the contract stamp.
pub fn write_leisure_file(path: &Path, rows: &[FixtureLeisure]) {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert(
        "leisure_contract".to_string(),
        square_store::store::LEISURE_CONTRACT_V2.to_string(),
    );
    metadata.insert(
        "grid".to_string(),
        square_store::store::GRID_CONTRACT_Z30.to_string(),
    );
    let schema = Arc::new(
        Schema::new(vec![
            Field::new("osm_id", DataType::Int64, false),
            Field::new("centroid_gx", DataType::Int32, false),
            Field::new("centroid_gy", DataType::Int32, false),
            Field::new("sport", DataType::UInt8, false),
            Field::new("opening_hours_frac", DataType::UInt8, false),
            Field::new("name", DataType::Utf8, true),
            Field::new("geom", DataType::Binary, true),
            Field::new("area_m2", DataType::Float32, true),
        ])
        .with_metadata(metadata),
    );
    let centroids: Vec<(i32, i32)> = rows
        .iter()
        .map(|r| grid_of(r.centroid.0, r.centroid.1))
        .collect();
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(Int64Array::from_iter_values(rows.iter().map(|r| r.osm_id))),
            Arc::new(Int32Array::from_iter_values(
                centroids.iter().map(|(gx, _)| *gx),
            )),
            Arc::new(Int32Array::from_iter_values(
                centroids.iter().map(|(_, gy)| *gy),
            )),
            Arc::new(UInt8Array::from_iter_values(rows.iter().map(|r| r.sport))),
            Arc::new(UInt8Array::from_iter_values(rows.iter().map(|_| 0u8))),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.name.as_str()),
            )),
            Arc::new(BinaryArray::from_iter_values(rows.iter().map(|r| {
                encode_ring(&square_ring_lonlat(r.centroid.1, r.centroid.0))
            }))),
            Arc::new(Float32Array::from_iter_values(
                rows.iter().map(|_| 400.0f32),
            )),
        ],
    )
    .unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut w = FileWriter::try_new(file, &schema).unwrap();
    w.write(&batch).unwrap();
    w.finish().unwrap();
}

/// One industrial row.
pub struct FixtureIndustrial {
    pub osm_id: i64,
    pub centroid: (f64, f64),
    pub source_type: u8,
    pub name: String,
}

/// An industrial.arrow on disk in the osm-extract v2 (grid) layout.
pub fn write_industrial_file(path: &Path, rows: &[FixtureIndustrial]) {
    let schema = Arc::new(Schema::new(vec![
        Field::new("osm_id", DataType::Int64, false),
        Field::new("centroid_gx", DataType::Int32, false),
        Field::new("centroid_gy", DataType::Int32, false),
        Field::new("source_type", DataType::UInt8, false),
        Field::new("site_subtype", DataType::UInt8, false),
        Field::new("name", DataType::Utf8, true),
        Field::new("hub_height", DataType::Float32, true),
        Field::new("rated_power_kw", DataType::Float32, true),
        Field::new("geom", DataType::Binary, true),
        Field::new("area_m2", DataType::Float32, true),
        Field::new("source_id", DataType::UInt16, false),
    ]));
    let centroids: Vec<(i32, i32)> = rows
        .iter()
        .map(|r| grid_of(r.centroid.0, r.centroid.1))
        .collect();
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(Int64Array::from_iter_values(rows.iter().map(|r| r.osm_id))),
            Arc::new(Int32Array::from_iter_values(
                centroids.iter().map(|(gx, _)| *gx),
            )),
            Arc::new(Int32Array::from_iter_values(
                centroids.iter().map(|(_, gy)| *gy),
            )),
            Arc::new(UInt8Array::from_iter_values(
                rows.iter().map(|r| r.source_type),
            )),
            Arc::new(UInt8Array::from_iter_values(rows.iter().map(|_| 0u8))),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.name.as_str()),
            )),
            Arc::new(Float32Array::from_iter(rows.iter().map(|_| None::<f32>))),
            Arc::new(Float32Array::from_iter(rows.iter().map(|_| None::<f32>))),
            Arc::new(BinaryArray::from_iter_values(rows.iter().map(|r| {
                encode_ring(&square_ring_lonlat(r.centroid.1, r.centroid.0))
            }))),
            Arc::new(Float32Array::from_iter_values(
                rows.iter().map(|_| 5000.0f32),
            )),
            Arc::new(UInt16Array::from_iter_values(rows.iter().map(|_| 0u16))),
        ],
    )
    .unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut w = FileWriter::try_new(file, &schema).unwrap();
    w.write(&batch).unwrap();
    w.finish().unwrap();
}

/// A square's directory under a prepared-year tree: `<year>/z9/<x>/<y>/`.
pub fn square_dir(year_dir: &Path, square: grid::Square) -> PathBuf {
    year_dir
        .join("z9")
        .join(square.x.to_string())
        .join(square.y.to_string())
}

/// A square's `structures.arrow` under the prepared-tree layout.
pub fn write_square_structures(
    year_dir: &Path,
    square: grid::Square,
    rows: &[StructureRow],
) -> PathBuf {
    let dir = square_dir(year_dir, square);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("structures.arrow");
    write_structure_file(&path, rows, true);
    path
}

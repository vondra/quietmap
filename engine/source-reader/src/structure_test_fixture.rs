//! Test double for `scripts/structures/build-structures.py` + the osm-extract
//! finalizers: writes the structures_v5 per-square table (kind-tagged
//! buildings ∪ walls) and tiny road/rail/leisure/industrial arrows that the
//! popup readers under test consume, with the contract metadata
//! `square_store::store::load_square` gates on. Coordinates are lon/lat floats
//! at the fixture boundary and z30 grid cells on disk, like the real writers.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{
    Array, ArrayRef, BinaryArray, BooleanArray, Float32Array, Float64Array, Int16Array, Int32Array,
    Int64Array, StringArray, UInt16Array, UInt32Array, UInt8Array,
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
    pub height_source: u8,
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

pub(crate) fn grid_of(lon: f64, lat: f64) -> (i32, i32) {
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
        Field::new("height_source", DataType::UInt8, false),
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
            rows.iter().map(|r| r.height_source),
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

/// One batch, as the merge's plain chunks before the finalize step blocks
/// them; `with_contract: false` carries the same rows without the contract
/// stamps (the `load_square` gate test's case).
pub fn structure_batch(rows: &[StructureRow], with_contract: bool) -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(structure_schema(with_contract)),
        structure_columns(rows),
    )
    .unwrap()
}

/// A merged (not yet finalized) structures.arrow on disk.
pub fn write_structure_file(path: &Path, rows: &[StructureRow], with_contract: bool) {
    let batch = structure_batch(rows, with_contract);
    let file = std::fs::File::create(path).unwrap();
    let mut w = FileWriter::try_new(file, &batch.schema()).unwrap();
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

/// One road microsegment row: lon/lat endpoints + classification. Defaults
/// carry an audible all-estimated prior block (the shape `roads-finalize`
/// publishes for an unobserved section), so `..Default::default()` rows are
/// collected like real prepared traffic.
pub struct FixtureRoad {
    pub osm_id: i64,
    pub start: (f64, f64),
    pub end: (f64, f64),
    pub road_class: u8,
    pub speed_limit: u8,
    pub lanes: u8,
    pub name: String,
    pub aadt_light: f64,
    pub aadt_medium: f64,
    pub aadt_heavy: f64,
    pub aadt_moto: f64,
    /// Per-category estimated bitmask (light 1, medium 2, heavy 4, moto 8).
    pub traffic_estimated: u8,
    /// 1-based index into the file's `roads_time_profiles` dictionary;
    /// 0 (the default) = no observed profile.
    pub traffic_profile_id: u16,
    /// Written but unread at runtime: the producer already resolved closures.
    pub access: u8,
}

impl Default for FixtureRoad {
    fn default() -> Self {
        Self {
            osm_id: 0,
            start: (0.0, 0.0),
            end: (0.0, 0.0),
            road_class: 2,
            speed_limit: 50,
            lanes: 2,
            name: String::new(),
            aadt_light: 3_000.0,
            aadt_medium: 200.0,
            aadt_heavy: 400.0,
            aadt_moto: 100.0,
            traffic_estimated: 15,
            traffic_profile_id: 0,
            access: 0,
        }
    }
}

fn roads_schema(profiles: Option<&str>) -> Schema {
    let mut fields = vec![
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
        Field::new("oneway", DataType::UInt8, false),
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
        Field::new("aadt_light", DataType::Float64, false),
        Field::new("aadt_medium", DataType::Float64, false),
        Field::new("aadt_heavy", DataType::Float64, false),
        Field::new("aadt_moto", DataType::Float64, false),
        Field::new("traffic_estimated", DataType::UInt8, false),
        Field::new("cross_section_aadt", DataType::Float64, false),
    ];
    let mut metadata =
        std::collections::HashMap::from([("road_traffic_contract".to_owned(), "1".to_owned())]);
    metadata.insert("osm_roads_contract".into(), square_store::osm_contract::ROADS_CONTRACT.into());
    if let Some(dictionary) = profiles {
        fields.push(Field::new("traffic_profile_id", DataType::UInt16, false));
        metadata.insert(
            crate::road_traffic::ROAD_PROFILES_METADATA_KEY.to_owned(),
            dictionary.to_owned(),
        );
    }
    Schema::new(fields).with_metadata(metadata)
}

/// A final roads.arrow on disk: osm-extract grid layout plus the finalized
/// traffic columns (`road_traffic_contract=1`).
pub fn write_roads_file(path: &Path, rows: &[FixtureRoad]) {
    write_roads_file_opts(path, rows, None);
}

/// `write_roads_file` with an optional `roads_time_profiles` dictionary:
/// `rows` reference entries 1-based via `traffic_profile_id`.
pub fn write_roads_file_opts(path: &Path, rows: &[FixtureRoad], profiles: Option<&str>) {
    let schema = Arc::new(roads_schema(profiles));
    let starts: Vec<(i32, i32)> = rows.iter().map(|r| grid_of(r.start.0, r.start.1)).collect();
    let ends: Vec<(i32, i32)> = rows.iter().map(|r| grid_of(r.end.0, r.end.1)).collect();
    let batch = RecordBatch::try_new(schema.clone(), {
        let mut columns: Vec<Arc<dyn Array>> = vec![
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
            Arc::new(UInt8Array::from(vec![0u8; rows.len()])),
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
            Arc::new(UInt8Array::from_iter_values(rows.iter().map(|r| r.access))),
            Arc::new(UInt16Array::from_iter_values(rows.iter().map(|_| 0u16))),
            Arc::new(Float64Array::from_iter_values(
                rows.iter().map(|r| r.aadt_light),
            )),
            Arc::new(Float64Array::from_iter_values(
                rows.iter().map(|r| r.aadt_medium),
            )),
            Arc::new(Float64Array::from_iter_values(
                rows.iter().map(|r| r.aadt_heavy),
            )),
            Arc::new(Float64Array::from_iter_values(
                rows.iter().map(|r| r.aadt_moto),
            )),
            Arc::new(UInt8Array::from_iter_values(
                rows.iter().map(|r| r.traffic_estimated),
            )),
            Arc::new(Float64Array::from_iter_values(
                rows.iter().map(|r| r.aadt_light + r.aadt_medium + r.aadt_heavy + r.aadt_moto),
            )),
        ];
        if profiles.is_some() {
            columns.push(Arc::new(UInt16Array::from_iter_values(
                rows.iter().map(|r| r.traffic_profile_id),
            )));
        }
        columns
    })
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
    let schema = Arc::new(
        Schema::new(vec![
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
            Field::new("trains_passenger_day", DataType::Float64, false),
            Field::new("trains_passenger_evening", DataType::Float64, false),
            Field::new("trains_passenger_night", DataType::Float64, false),
            Field::new("trains_freight_day", DataType::Float64, false),
            Field::new("trains_freight_evening", DataType::Float64, false),
            Field::new("trains_freight_night", DataType::Float64, false),
            Field::new("passenger_status", DataType::UInt8, false),
            Field::new("passenger_source_id", DataType::UInt16, false),
            Field::new("passenger_matching", DataType::UInt8, false),
            Field::new("freight_status", DataType::UInt8, false),
            Field::new("freight_source_id", DataType::UInt16, false),
            Field::new("freight_matching", DataType::UInt8, false),
        ])
        .with_metadata(std::collections::HashMap::from([
            ("rail_traffic_contract".to_owned(), "1".to_owned()),
            ("osm_railways_contract".into(), square_store::osm_contract::RAILWAYS_CONTRACT.into()),
        ])),
    );
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
            Arc::new(Float64Array::from(vec![56.0; rows.len()])),
            Arc::new(Float64Array::from(vec![16.0; rows.len()])),
            Arc::new(Float64Array::from(vec![8.0; rows.len()])),
            Arc::new(Float64Array::from(vec![10.0; rows.len()])),
            Arc::new(Float64Array::from(vec![10.0 / 3.0; rows.len()])),
            Arc::new(Float64Array::from(vec![20.0 / 3.0; rows.len()])),
            Arc::new(UInt8Array::from(vec![2u8; rows.len()])),
            Arc::new(UInt16Array::from(vec![0u16; rows.len()])),
            Arc::new(UInt8Array::from(vec![0u8; rows.len()])),
            Arc::new(UInt8Array::from(vec![2u8; rows.len()])),
            Arc::new(UInt16Array::from(vec![0u16; rows.len()])),
            Arc::new(UInt8Array::from(vec![0u8; rows.len()])),
        ],
    )
    .unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut w = FileWriter::try_new(file, &schema).unwrap();
    w.write(&batch).unwrap();
    w.finish().unwrap();
}

/// One leisure row in the `leisure_v4` layout. `chain_lonlat` is `None` for a
/// point row, an open chain for a raceway/track line, a closed ring for an
/// area; `tags` are the retained OSM tags, written as stable sorted JSON
/// like the extractor writes them.
pub struct FixtureLeisure {
    pub osm_id: i64,
    pub centroid: (f64, f64),
    pub sport: u8,
    pub name: String,
    pub chain_lonlat: Option<Vec<(f64, f64)>>,
    pub tags: &'static [(&'static str, &'static str)],
}

fn tags_json(tags: &[(&str, &str)]) -> String {
    serde_json::to_string(
        &tags
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect::<std::collections::BTreeMap<String, String>>(),
    )
    .unwrap()
}

/// A leisure.arrow on disk in the v2 (grid) layout, stamped `leisure_v4`.
pub fn write_leisure_file(path: &Path, rows: &[FixtureLeisure]) {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert(
        "leisure_contract".to_string(),
        square_store::osm_contract::LEISURE_CONTRACT_V4.to_string(),
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
            Field::new("geometry_kind", DataType::UInt8, false),
            Field::new("length_m", DataType::Float32, true),
            Field::new("osm_tags", DataType::Utf8, false),
            Field::new("osm_kind", DataType::Utf8, false),
        ])
        .with_metadata(metadata),
    );
    let centroids: Vec<(i32, i32)> = rows
        .iter()
        .map(|r| grid_of(r.centroid.0, r.centroid.1))
        .collect();
    let chains: Vec<Option<Vec<(i32, i32)>>> = rows
        .iter()
        .map(|r| {
            r.chain_lonlat.as_ref().map(|chain| {
                chain.iter().map(|&(lon, lat)| grid_of(lon, lat)).collect()
            })
        })
        .collect();
    let kinds: Vec<u8> = rows
        .iter()
        .map(|r| match &r.chain_lonlat {
            None => 0,
            Some(chain) if chain.len() >= 4 && chain.first() == chain.last() => 1,
            Some(_) => 2,
        })
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
            Arc::new(BinaryArray::from_iter(chains.iter().map(|chain| {
                chain.as_ref().map(|chain| grid::poly::encode_grid_poly(chain))
            }))),
            Arc::new(Float32Array::from_iter(rows.iter().zip(&chains).map(
                |(r, chain)| match (r.chain_lonlat.as_ref(), chain.as_ref()) {
                    (Some(lonlat), Some(grid))
                        if lonlat.len() >= 4 && lonlat.first() == lonlat.last() =>
                    {
                        grid::poly::ring_area_m2(grid).map(|area| area as f32)
                    }
                    _ => None,
                },
            ))),
            Arc::new(UInt8Array::from_iter_values(kinds)),
            Arc::new(Float32Array::from_iter(rows.iter().map(|r| match &r.chain_lonlat {
                Some(chain)
                    if !(chain.len() >= 4 && chain.first() == chain.last()) =>
                {
                    Some(
                        chain.windows(2).map(|leg| grid::geo::flat_dist(leg[0].1, leg[0].0, leg[1].1, leg[1].0)).sum::<f64>() as f32,
                    )
                }
                _ => None,
            }))),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| tags_json(r.tags)),
            )),
            Arc::new(StringArray::from_iter_values(rows.iter().map(|r| {
                if r.chain_lonlat.is_some() {
                    "way"
                } else {
                    "node"
                }
            }))),
        ],
    )
    .unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut w = FileWriter::try_new(file, &schema).unwrap();
    w.write(&batch).unwrap();
    w.finish().unwrap();
}

/// One industrial row in the `osm_industrial_contract = 2` layout.
/// `ring_lonlat` is `None` for a point row (turbines, node substations,
/// transformers); `suppressed` marks enrichment-retired rows the readers must
/// skip; `tags` are the retained OSM tags; `rated_power_kw` covers
/// generator-unit rows (turbines, solar units).
pub struct FixtureIndustrial {
    pub osm_id: i64,
    pub centroid: (f64, f64),
    pub source_type: u8,
    pub name: String,
    pub ring_lonlat: Option<Vec<(f64, f64)>>,
    pub suppressed: bool,
    pub tags: &'static [(&'static str, &'static str)],
    pub rated_power_kw: Option<f32>,
}

/// An industrial.arrow on disk in the osm-extract v2 (grid) layout.
pub fn write_industrial_file(path: &Path, rows: &[FixtureIndustrial]) {
    let schema = Arc::new(
        Schema::new(vec![
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
            Field::new("suppressed", DataType::UInt8, false),
            Field::new("osm_tags", DataType::Utf8, false),
            Field::new("osm_kind", DataType::Utf8, false),
        ])
        .with_metadata(std::collections::HashMap::from([
            (
                "osm_industrial_contract".into(),
                square_store::osm_contract::INDUSTRIAL_CONTRACT.into(),
            ),
            (
                "grid".into(),
                square_store::store::GRID_CONTRACT_Z30.into(),
            ),
        ])),
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
            Arc::new(UInt8Array::from_iter_values(
                rows.iter().map(|r| r.source_type),
            )),
            Arc::new(UInt8Array::from_iter_values(rows.iter().map(|_| 0u8))),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.name.as_str()),
            )),
            Arc::new(Float32Array::from_iter(rows.iter().map(|_| None::<f32>))),
            Arc::new(Float32Array::from_iter(
                rows.iter().map(|r| r.rated_power_kw),
            )),
            Arc::new(BinaryArray::from_iter(rows.iter().map(|r| {
                r.ring_lonlat.as_ref().map(|ring| {
                    let grid: Vec<(i32, i32)> =
                        ring.iter().map(|&(lon, lat)| grid_of(lon, lat)).collect();
                    grid::poly::encode_grid_poly(&grid)
                })
            }))),
            Arc::new(Float32Array::from_iter(rows.iter().map(|r| {
                r.ring_lonlat.as_ref().and_then(|ring| {
                    let grid: Vec<(i32, i32)> =
                        ring.iter().map(|&(lon, lat)| grid_of(lon, lat)).collect();
                    grid::poly::ring_area_m2(&grid).map(|area| area as f32)
                })
            }))),
            Arc::new(UInt16Array::from_iter_values(rows.iter().map(|_| 0u16))),
            Arc::new(UInt8Array::from_iter_values(
                rows.iter().map(|r| u8::from(r.suppressed)),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| tags_json(r.tags)),
            )),
            Arc::new(StringArray::from_iter_values(rows.iter().map(|r| {
                if r.ring_lonlat.is_some() {
                    "way"
                } else {
                    "node"
                }
            }))),
        ],
    )
    .unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut w = FileWriter::try_new(file, &schema).unwrap();
    w.write(&batch).unwrap();
    w.finish().unwrap();
}

/// One ship traffic cell: centre (lon, lat) and hours per month by class.
pub struct FixtureShipCell {
    pub centroid: (f64, f64),
    pub hours: [f32; 3],
}

/// A ships.arrow on disk as `scripts/ships/build_ships.py` writes it; `contract`
/// lets a test stamp a stale version.
pub fn write_ships_file(path: &Path, rows: &[FixtureShipCell], contract: &str) {
    let metadata = std::collections::HashMap::from([
        ("grid".to_string(), "z30".to_string()),
        ("ships_contract".to_string(), contract.to_string()),
    ]);
    let schema = Arc::new(Schema::new_with_metadata(
        vec![
            Field::new("centroid_gx", DataType::Int32, false),
            Field::new("centroid_gy", DataType::Int32, false),
            Field::new("area_m2", DataType::Float32, false),
            Field::new("hours_large", DataType::Float32, false),
            Field::new("hours_work", DataType::Float32, false),
            Field::new("hours_leisure", DataType::Float32, false),
            Field::new("source_id", DataType::UInt16, false),
        ],
        metadata,
    ));
    let centroids: Vec<(i32, i32)> = rows
        .iter()
        .map(|r| grid_of(r.centroid.0, r.centroid.1))
        .collect();
    let hours = |class: usize| {
        Arc::new(Float32Array::from_iter_values(
            rows.iter().map(move |r| r.hours[class]),
        ))
    };
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(Int32Array::from_iter_values(
                centroids.iter().map(|(gx, _)| *gx),
            )),
            Arc::new(Int32Array::from_iter_values(
                centroids.iter().map(|(_, gy)| *gy),
            )),
            Arc::new(Float32Array::from_iter_values(
                rows.iter().map(|_| 1_000_000.0f32),
            )),
            hours(0),
            hours(1),
            hours(2),
            Arc::new(UInt16Array::from_iter_values(rows.iter().map(|_| 9901u16))),
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

/// A square's `structures.arrow` under the prepared-tree layout, finalized
/// like the real files: z14-blocked, with `structures.qoix` beside it.
pub fn write_square_structures(
    year_dir: &Path,
    square: grid::Square,
    rows: &[StructureRow],
) -> PathBuf {
    let dir = square_dir(year_dir, square);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("structures.arrow");
    write_structure_file(&path, rows, true);
    crate::structures_finalize::finalize_square_structures(&dir, square).unwrap();
    path
}

/// An aircraft sampling window with fixed day-list hashes.
pub(crate) fn sampling_window(
    baseline_days: u16,
    increment_days: u16,
) -> noise_compute::emission::aircraft::SamplingWindow {
    noise_compute::emission::aircraft::SamplingWindow {
        baseline_days,
        increment_days,
        baseline_days_sha256: "baseline".into(),
        increment_days_sha256: "increment".into(),
    }
}

/// A square's `facade_exposure.arrow` for the rows given, stamped with the
/// square's current `structures.arrow` length as the stage would stamp it.
pub fn write_facade_exposure(
    square_dir: &Path,
    rows: &[square_store::facade_exposure_contract::FacadeExposureRow],
) {
    use square_store::facade_exposure_contract as contract;
    let structures_bytes = std::fs::metadata(square_dir.join("structures.arrow"))
        .unwrap()
        .len();
    let metadata = std::collections::HashMap::from([
        (contract::CONTRACT_KEY.to_string(), contract::CONTRACT.to_string()),
        (
            "grid".to_string(),
            square_store::store::GRID_CONTRACT_Z30.to_string(),
        ),
        (contract::LAYER_ORDER_KEY.to_string(), "road".to_string()),
        (
            contract::STRUCTURES_BYTES_KEY.to_string(),
            structures_bytes.to_string(),
        ),
    ]);
    let schema = std::sync::Arc::new(contract::schema(1, metadata));
    let batch = RecordBatch::try_new(schema.clone(), contract::columns(rows, 1).unwrap()).unwrap();
    let file = std::fs::File::create(square_dir.join(contract::FACADE_EXPOSURE_ARROW)).unwrap();
    let mut writer = FileWriter::try_new(file, &schema).unwrap();
    writer.write(&batch).unwrap();
    writer.finish().unwrap();
}

/// The stage's row for a building whose exposed receivers are `receivers`,
/// choosing `chosen` (no layer powers: the popup does not read them).
pub fn facade_exposure_row(
    footprint_id: u32,
    receivers: &[noise_compute::facade_receivers::FacadeReceiverPosition],
    chosen: Option<usize>,
) -> square_store::facade_exposure_contract::FacadeExposureRow {
    square_store::facade_exposure_contract::FacadeExposureRow {
        footprint_id,
        facade_points: receivers.len() as u32,
        chosen: chosen.map(|i| square_store::facade_exposure_contract::ChosenFacadeReceiver {
            canonical_index: i as u32,
            gx: receivers[i].gx,
            gy: receivers[i].gy,
            ground_altitude_m: 0.0,
            outward_bearing_deg: receivers[i].outward_bearing_deg,
            layer_period_power: vec![0.0; 3],
            total_lden_db: 0.0,
            runner_up_lden_db: None,
        }),
    }
}

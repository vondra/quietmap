//! OSM building rows and the consumed structures Arrow schema, without the retired runtime index.

use anyhow::{Result, anyhow, ensure};
use arrow_array::{
    Array, ArrayRef, BinaryArray, BooleanArray, Float32Array, Int16Array, Int32Array, Int64Array,
    RecordBatch, StringArray, UInt8Array, UInt16Array, UInt32Array,
};
use arrow_ipc::{reader::FileReader, writer::FileWriter};
use arrow_schema::{DataType, Field, Schema};
use std::{fs::File, path::Path, sync::Arc};

#[derive(Default, Debug)]
pub struct StructureRow {
    pub kind: u8,
    pub geometry: Option<Vec<u8>>,
    pub height_m: i16,
    pub height_source: u8,
    pub envelope: u8,
    pub centroid: (i32, i32),
    pub osm_id: Option<i64>,
    pub building_type: Option<u8>,
    pub height: Option<f32>,
    pub floors: Option<u8>,
    pub name: Option<String>,
    pub street: Option<String>,
    pub house_number: Option<String>,
    pub area_m2: Option<f32>,
    pub emission_geometry: Option<Vec<u8>>,
    pub emission_centroid: Option<(i32, i32)>,
    pub storeys: Option<u8>,
    pub ordinal: Option<u32>,
}

pub struct OsmBuilding {
    pub row: StructureRow,
    pub building_use: u8,
    pub ground_activity: bool,
}

pub fn column<'a, T: 'static>(batch: &'a RecordBatch, name: &str) -> Result<&'a T> {
    batch
        .column_by_name(name)
        .and_then(|a| a.as_any().downcast_ref())
        .ok_or_else(|| anyhow!("invalid {name}"))
}

pub fn read_osm(path: &Path) -> Result<Vec<OsmBuilding>> {
    if !path.exists() {
        return Ok(vec![]);
    }
    let reader = FileReader::try_new(File::open(path)?, None)?;
    ensure!(
        reader.schema().metadata().get("grid").map(String::as_str) == Some("z30")
            && reader
                .schema()
                .metadata()
                .get("buildings_contract")
                .map(String::as_str)
                == Some("buildings_v5"),
        "expected buildings_v5/z30"
    );
    let mut rows = Vec::new();
    for batch in reader {
        let batch = batch?;
        let geometry: &BinaryArray = column(&batch, "geom")?;
        let ids: &Int64Array = column(&batch, "osm_id")?;
        let x: &Int32Array = column(&batch, "centroid_gx")?;
        let y: &Int32Array = column(&batch, "centroid_gy")?;
        let classes: &UInt8Array = column(&batch, "building_type")?;
        let uses: &UInt8Array = column(&batch, "building_use")?;
        let floors: &UInt8Array = column(&batch, "floors")?;
        let heights: &Float32Array = column(&batch, "height")?;
        let areas: &Float32Array = column(&batch, "area_m2")?;
        let names: &StringArray = column(&batch, "name")?;
        let streets: &StringArray = column(&batch, "addr_street")?;
        let numbers: &StringArray = column(&batch, "addr_housenumber")?;
        let ground: &BooleanArray = column(&batch, "area_source")?;
        for array in [ids as &dyn Array, x, y, classes, uses, floors, ground] {
            ensure!(
                array.null_count() == 0,
                "null OSM identity or classification"
            );
        }
        for i in 0..batch.num_rows() {
            let text = |a: &StringArray| (!a.is_null(i)).then(|| a.value(i).to_owned());
            rows.push(OsmBuilding {
                building_use: uses.value(i),
                ground_activity: ground.value(i),
                row: StructureRow {
                    osm_id: Some(ids.value(i)),
                    centroid: (x.value(i), y.value(i)),
                    building_type: Some(classes.value(i)),
                    floors: Some(floors.value(i)),
                    height: (!heights.is_null(i)).then(|| heights.value(i)),
                    area_m2: (!areas.is_null(i)).then(|| areas.value(i)),
                    name: text(names),
                    street: text(streets),
                    house_number: text(numbers),
                    emission_geometry: (!geometry.is_null(i)).then(|| geometry.value(i).to_vec()),
                    ..Default::default()
                },
            });
        }
    }
    Ok(rows)
}

pub fn write(path: &Path, rows: &[StructureRow]) -> Result<()> {
    let fields = vec![
        ("kind", DataType::UInt8, false),
        ("geom", DataType::Binary, true),
        ("height_m", DataType::Int16, false),
        ("height_source", DataType::UInt8, false),
        ("envelope_class", DataType::UInt8, false),
        ("centroid_gx", DataType::Int32, false),
        ("centroid_gy", DataType::Int32, false),
        ("osm_id", DataType::Int64, true),
        ("building_type", DataType::UInt8, true),
        ("height", DataType::Float32, true),
        ("floors", DataType::UInt8, true),
        ("name", DataType::Utf8, true),
        ("addr_street", DataType::Utf8, true),
        ("addr_housenumber", DataType::Utf8, true),
        ("area_m2", DataType::Float32, true),
        ("emission_geom", DataType::Binary, true),
        ("emission_centroid_gx", DataType::Int32, true),
        ("emission_centroid_gy", DataType::Int32, true),
        ("storeys", DataType::UInt8, true),
        ("screening_ordinal", DataType::UInt32, true),
    ]
    .into_iter()
    .map(|(n, t, o)| Field::new(n, t, o))
    .collect::<Vec<_>>();
    let schema = Arc::new(Schema::new_with_metadata(
        fields,
        std::collections::HashMap::from([
            ("grid".to_owned(), "z30".to_owned()),
            ("structures_contract".to_owned(), "structures_v5".to_owned()),
        ]),
    ));
    std::fs::create_dir_all(path.parent().unwrap())?;
    let mut writer = FileWriter::try_new(
        File::options().write(true).create_new(true).open(path)?,
        &schema,
    )?;
    for rows in rows.chunks(8192) {
        macro_rules! required {
            ($ty:ident, $f:expr) => {
                Arc::new($ty::from_iter_values(rows.iter().map($f))) as ArrayRef
            };
        }
        macro_rules! optional {
            ($ty:ident, $f:expr) => {
                Arc::new($ty::from_iter(rows.iter().map($f))) as ArrayRef
            };
        }
        let columns = vec![
            required!(UInt8Array, |r| r.kind),
            optional!(BinaryArray, |r| r.geometry.as_deref()),
            required!(Int16Array, |r| r.height_m),
            required!(UInt8Array, |r| r.height_source),
            required!(UInt8Array, |r| r.envelope),
            required!(Int32Array, |r| r.centroid.0),
            required!(Int32Array, |r| r.centroid.1),
            optional!(Int64Array, |r| r.osm_id),
            optional!(UInt8Array, |r| r.building_type),
            optional!(Float32Array, |r| r.height),
            optional!(UInt8Array, |r| r.floors),
            optional!(StringArray, |r| r.name.as_deref()),
            optional!(StringArray, |r| r.street.as_deref()),
            optional!(StringArray, |r| r.house_number.as_deref()),
            optional!(Float32Array, |r| r.area_m2),
            optional!(BinaryArray, |r| r.emission_geometry.as_deref()),
            optional!(Int32Array, |r| r.emission_centroid.map(|p| p.0)),
            optional!(Int32Array, |r| r.emission_centroid.map(|p| p.1)),
            optional!(UInt8Array, |r| r.storeys),
            optional!(UInt32Array, |r| r.ordinal),
        ];
        writer.write(&RecordBatch::try_new(schema.clone(), columns)?)?;
    }
    writer.finish()?;
    Ok(())
}

pub struct OsmWall {
    pub osm_id: i64,
    pub start: (i32, i32),
    pub end: (i32, i32),
    pub height: f32,
    pub mapped: bool,
    pub country: u16,
}

pub fn read_walls(path: &Path) -> Result<Vec<OsmWall>> {
    if !path.exists() {
        return Ok(vec![]);
    }
    let reader = FileReader::try_new(File::open(path)?, None)?;
    ensure!(
        reader.schema().metadata().get("grid").map(String::as_str) == Some("z30")
            && reader
                .schema()
                .metadata()
                .get("barriers_contract")
                .map(String::as_str)
                == Some("country_baked_v1"),
        "walls need country_baked_v1/z30"
    );
    let mut rows = Vec::new();
    for batch in reader {
        let b = batch?;
        let ids: &Int64Array = column(&b, "osm_id")?;
        let sx: &Int32Array = column(&b, "start_gx")?;
        let sy: &Int32Array = column(&b, "start_gy")?;
        let ex: &Int32Array = column(&b, "end_gx")?;
        let ey: &Int32Array = column(&b, "end_gy")?;
        let height: &Float32Array = column(&b, "height")?;
        let tier: &UInt8Array = column(&b, "height_tier")?;
        let country: &UInt16Array = column(&b, "country_iso")?;
        for a in [ids as &dyn Array, sx, sy, ex, ey, height, tier, country] {
            ensure!(a.null_count() == 0, "null OSM wall");
        }
        for i in 0..b.num_rows() {
            rows.push(OsmWall {
                osm_id: ids.value(i),
                start: (sx.value(i), sy.value(i)),
                end: (ex.value(i), ey.value(i)),
                height: height.value(i),
                mapped: tier.value(i) == 0,
                country: country.value(i),
            });
        }
    }
    Ok(rows)
}

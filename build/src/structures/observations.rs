//! One normalized vector-observation format for footprint, height, wall and national-attribute evidence.

use anyhow::{Result, ensure};
use arrow_array::{
    Array, ArrayRef, BinaryArray, BooleanArray, Float64Array, RecordBatch, UInt8Array,
};
use arrow_ipc::{reader::FileReader, writer::FileWriter};
use arrow_schema::{DataType, Field, Schema};
use geos::Geometry;
use std::{fs::File, path::Path, sync::Arc};

pub const FOOTPRINT: u8 = 0;
pub const HEIGHT: u8 = 1;
pub const WALL: u8 = 2;
pub const ATTRIBUTE: u8 = 3;

#[derive(Clone, Debug)]
pub struct Observation {
    pub kind: u8,
    pub geometry: Option<Vec<u8>>,
    pub longitude: f64,
    pub latitude: f64,
    pub height_m: Option<f64>,
    pub floors: u8,
    pub building_type: Option<u8>,
    pub envelope: u8,
    pub open_roof: bool,
    pub owned: bool,
}

impl Observation {
    pub fn geometry(&self) -> Result<Geometry> {
        Ok(Geometry::new_from_wkb(
            self.geometry
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("observation has no geometry"))?,
        )?)
    }
}

pub fn schema(year: &str) -> Schema {
    Schema::new_with_metadata(
        vec![
            Field::new("kind", DataType::UInt8, false),
            Field::new("geometry", DataType::Binary, true),
            Field::new("longitude", DataType::Float64, false),
            Field::new("latitude", DataType::Float64, false),
            Field::new("height_m", DataType::Float64, true),
            Field::new("floors", DataType::UInt8, false),
            Field::new("building_type", DataType::UInt8, true),
            Field::new("envelope", DataType::UInt8, false),
            Field::new("open_roof", DataType::Boolean, false),
            Field::new("owned", DataType::Boolean, false),
        ],
        std::collections::HashMap::from([
            ("structure-observations".to_owned(), "1".to_owned()),
            ("year".to_owned(), year.to_owned()),
        ]),
    )
}

pub fn write(path: &Path, rows: &[Observation], year: &str) -> Result<()> {
    std::fs::create_dir_all(path.parent().unwrap())?;
    let schema = Arc::new(schema(year));
    let mut writer = FileWriter::try_new(
        File::options().write(true).create_new(true).open(path)?,
        &schema,
    )?;
    for rows in rows.chunks(8192) {
        let columns: Vec<ArrayRef> = vec![
            Arc::new(UInt8Array::from_iter_values(rows.iter().map(|r| r.kind))),
            Arc::new(BinaryArray::from_iter(
                rows.iter().map(|r| r.geometry.as_deref()),
            )),
            Arc::new(Float64Array::from_iter_values(
                rows.iter().map(|r| r.longitude),
            )),
            Arc::new(Float64Array::from_iter_values(
                rows.iter().map(|r| r.latitude),
            )),
            Arc::new(Float64Array::from_iter(rows.iter().map(|r| r.height_m))),
            Arc::new(UInt8Array::from_iter_values(rows.iter().map(|r| r.floors))),
            Arc::new(UInt8Array::from_iter(rows.iter().map(|r| r.building_type))),
            Arc::new(UInt8Array::from_iter_values(
                rows.iter().map(|r| r.envelope),
            )),
            Arc::new(BooleanArray::from_iter(
                rows.iter().map(|r| Some(r.open_roof)),
            )),
            Arc::new(BooleanArray::from_iter(rows.iter().map(|r| Some(r.owned)))),
        ];
        writer.write(&RecordBatch::try_new(schema.clone(), columns)?)?;
    }
    writer.finish()?;
    Ok(())
}

pub fn read(path: &Path, year: &str) -> Result<Vec<Observation>> {
    let reader = FileReader::try_new(File::open(path)?, None)?;
    ensure!(
        *reader.schema() == schema(year),
        "{}: wrong structure observation schema or year",
        path.display()
    );
    let mut rows = Vec::new();
    for batch in reader {
        let batch = batch?;
        for (field, column) in batch.schema().fields().iter().zip(batch.columns()) {
            ensure!(
                field.is_nullable() || column.null_count() == 0,
                "null required observation {}",
                field.name()
            );
        }
        let c = |i: usize| batch.column(i);
        let kinds = c(0).as_any().downcast_ref::<UInt8Array>().unwrap();
        let geometry = c(1).as_any().downcast_ref::<BinaryArray>().unwrap();
        let lon = c(2).as_any().downcast_ref::<Float64Array>().unwrap();
        let lat = c(3).as_any().downcast_ref::<Float64Array>().unwrap();
        let height = c(4).as_any().downcast_ref::<Float64Array>().unwrap();
        let floors = c(5).as_any().downcast_ref::<UInt8Array>().unwrap();
        let class = c(6).as_any().downcast_ref::<UInt8Array>().unwrap();
        let envelope = c(7).as_any().downcast_ref::<UInt8Array>().unwrap();
        let open = c(8).as_any().downcast_ref::<BooleanArray>().unwrap();
        let owned = c(9).as_any().downcast_ref::<BooleanArray>().unwrap();
        for i in 0..batch.num_rows() {
            ensure!(
                kinds.value(i) <= ATTRIBUTE
                    && lon.value(i).is_finite()
                    && lat.value(i).is_finite()
                    && lon.value(i).abs() <= 180.0
                    && lat.value(i).abs() <= 90.0,
                "invalid structure observation"
            );
            ensure!(
                !matches!(kinds.value(i), HEIGHT | WALL)
                    || (!height.is_null(i) && height.value(i).is_finite()),
                "survey observation needs finite metres"
            );
            rows.push(Observation {
                kind: kinds.value(i),
                geometry: (!geometry.is_null(i)).then(|| geometry.value(i).to_vec()),
                longitude: lon.value(i),
                latitude: lat.value(i),
                height_m: (!height.is_null(i)).then(|| height.value(i)),
                floors: floors.value(i),
                building_type: (!class.is_null(i)).then(|| class.value(i)),
                envelope: envelope.value(i),
                open_roof: open.value(i),
                owned: owned.value(i),
            });
        }
    }
    Ok(rows)
}

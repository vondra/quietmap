//! Original mapped road pieces with attached observations, before physical carriageway allocation.

use crate::osm::geometry;
use anyhow::{Result, ensure};
use arrow_array::{
    Array, BooleanArray, Float32Array, Int16Array, Int32Array, Int64Array, RecordBatch,
    StringArray, UInt8Array, UInt16Array, UInt32Array,
};
use std::path::Path;

pub const COUNTS: [&str; 4] = ["aadt_light", "aadt_medium", "aadt_heavy", "aadt_moto"];
pub const CONTRACT: &str = "road_traffic_contract";
pub const CROSS_SECTION_AADT: &str = "cross_section_aadt";

#[derive(Clone, Debug)]
pub struct Road {
    pub way_id: i64,
    pub segment_idx: i16,
    pub ends: [(i32, i32); 2],
    pub length_m: f64,
    pub name: String,
    pub speed_limit: u8,
    pub speed_taper: u8,
    pub start: (f64, f64),
    pub end: (f64, f64),
    pub direction: u8,
    pub class: u8,
    pub lanes: u8,
    pub access: u8,
    pub tunnel: bool,
    pub built_up: u8,
    pub roundabout: bool,
    pub country: (u16, u16),
    pub source_id: u16,
    pub observation_source_id: u16,
    pub count_origin_measured: bool,
    pub counts: [f64; 4],
    pub basis: u8,
    pub estimated: u8,
    pub observation: String,
    pub reference: String,
    pub corridor: String,
}

impl Road {
    pub fn midpoint(&self) -> (f64, f64) {
        (
            (self.start.0 + self.end.0) / 2.0,
            (self.start.1 + self.end.1) / 2.0,
        )
    }
    pub fn mercator_scale(&self) -> f64 {
        1.0 / (self.midpoint().1 / geometry::WEB_MERCATOR_RADIUS_M).cosh()
    }
    pub fn travel_vector(&self) -> (f64, f64) {
        let sign = if self.direction == 2 { -1.0 } else { 1.0 };
        (
            (self.end.0 - self.start.0) * sign,
            (self.end.1 - self.start.1) * sign,
        )
    }
}

pub fn column<'a, T: Array + 'static>(batch: &'a RecordBatch, name: &str) -> Result<&'a T> {
    let value = crate::dev4::column::<T>(batch, name).map_err(anyhow::Error::msg)?;
    ensure!(value.null_count() == 0, "null road column {name}");
    Ok(value)
}

pub fn load(path: &Path) -> Result<Vec<RecordBatch>> {
    let reader = arrow_ipc::reader::FileReader::try_new(std::fs::File::open(path)?, None)?;
    let schema = reader.schema();
    ensure!(
        schema.metadata().get("grid").map(String::as_str) == Some("z30")
            && schema.metadata().get("roads_contract").map(String::as_str)
                == Some("country_baked_v1"),
        "expected mapped country-baked z30 roads"
    );
    ensure!(
        !schema.metadata().contains_key(CONTRACT),
        "road preparation requires original mapped pieces"
    );
    let mut batches = reader.collect::<std::result::Result<Vec<_>, _>>()?;
    if batches.is_empty() {
        batches.push(RecordBatch::new_empty(schema.clone()));
    }
    let batch = arrow_select::concat::concat_batches(&schema, &batches)?;
    let ids = column::<Int64Array>(&batch, "osm_id")?;
    let segments = column::<Int16Array>(&batch, "segment_idx")?;
    let mut order = (0..batch.num_rows()).collect::<Vec<_>>();
    // Building frontage and equal-distance routes must not change when Arrow files are rebatched or reordered.
    order.sort_by_key(|&i| (ids.value(i), segments.value(i)));
    ensure!(
        order
            .windows(2)
            .all(|pair| (ids.value(pair[0]), segments.value(pair[0]))
                != (ids.value(pair[1]), segments.value(pair[1]))),
        "duplicate original road piece"
    );
    let indices = UInt32Array::from(
        order
            .into_iter()
            .map(u32::try_from)
            .collect::<std::result::Result<Vec<_>, _>>()?,
    );
    let columns = batch
        .columns()
        .iter()
        .map(|column| arrow_select::take::take(column.as_ref(), &indices, None))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(vec![RecordBatch::try_new(schema, columns)?])
}

pub fn roads(batch: &RecordBatch) -> Result<Vec<Road>> {
    let ids = column::<Int64Array>(batch, "osm_id")?;
    let segments = column::<Int16Array>(batch, "segment_idx")?;
    let lengths = column::<Float32Array>(batch, "length_m")?;
    let speeds = column::<UInt8Array>(batch, "speed_limit")?;
    let sx = column::<Int32Array>(batch, "start_gx")?;
    let sy = column::<Int32Array>(batch, "start_gy")?;
    let ex = column::<Int32Array>(batch, "end_gx")?;
    let ey = column::<Int32Array>(batch, "end_gy")?;
    let direction = column::<UInt8Array>(batch, "oneway")?;
    let class = column::<UInt8Array>(batch, "road_class")?;
    let lanes = column::<UInt8Array>(batch, "lanes")?;
    let access = column::<UInt8Array>(batch, "access")?;
    let tunnel = column::<BooleanArray>(batch, "tunnel")?;
    let junction = column::<UInt8Array>(batch, "junction")?;
    let country = column::<UInt16Array>(batch, "country_iso")?;
    let built_up = batch
        .column_by_name("built_up")
        .map(|_| column::<UInt8Array>(batch, "built_up"))
        .transpose()?;
    let city = batch
        .column_by_name("city_id")
        .map(|_| column::<UInt16Array>(batch, "city_id"))
        .transpose()?;
    let text = |name, i| {
        batch
            .column_by_name(name)
            .and_then(|v| v.as_any().downcast_ref::<StringArray>())
            .filter(|v| !v.is_null(i))
            .map(|v| v.value(i).trim())
            .unwrap_or("")
    };
    (0..batch.num_rows())
        .map(|i| {
            ensure!(
                direction.value(i) <= 4 && class.value(i) <= 12 && access.value(i) <= 8,
                "invalid road attributes at {}:{}",
                ids.value(i),
                segments.value(i)
            );
            let reference = text("ref", i);
            let start = geometry::grid_to_meters(sx.value(i), sy.value(i));
            let mut end = geometry::grid_to_meters(ex.value(i), ey.value(i));
            end.0 += ((start.0 - end.0) / geometry::EARTH_CIRCUMFERENCE_M).round()
                * geometry::EARTH_CIRCUMFERENCE_M;
            Ok(Road {
                way_id: ids.value(i),
                segment_idx: segments.value(i),
                start,
                end,
                ends: [(sx.value(i), sy.value(i)), (ex.value(i), ey.value(i))],
                length_m: f64::from(lengths.value(i)),
                name: text("name", i).into(),
                speed_limit: speeds.value(i),
                speed_taper: 0,
                direction: direction.value(i),
                class: class.value(i),
                lanes: lanes.value(i),
                access: access.value(i),
                tunnel: tunnel.value(i),
                built_up: built_up.map_or(0, |v| v.value(i)),
                roundabout: junction.value(i) != 0,
                country: (country.value(i), city.map_or(0, |v| v.value(i))),
                source_id: 0,
                observation_source_id: 0,
                count_origin_measured: false,
                counts: [0.0; 4],
                basis: 0,
                estimated: 15,
                observation: String::new(),
                reference: reference.into(),
                corridor: if reference.is_empty() {
                    text("name", i)
                } else {
                    reference
                }
                .to_uppercase(),
            })
        })
        .collect()
}

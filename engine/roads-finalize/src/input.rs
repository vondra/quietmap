//! Read native road identities and explicit source observations for allocation.

use arrow::array::{Array, BooleanArray, Float64Array, Int16Array, Int32Array, Int64Array, StringArray, UInt16Array, UInt8Array};
use arrow::ipc::reader::FileReader;
use arrow::record_batch::RecordBatch;
use noise_compute::defaults::baked_square_country_city;
use noise_compute::sources::{provenance_of, Provenance};
use noise_compute::square_country_city::SquareCountryCity;
use std::path::Path;

pub const CONTRACT: &str = "road_traffic_contract";
pub const COUNTS: [&str; 4] = ["aadt_light", "aadt_medium", "aadt_heavy", "aadt_moto"];
/// Whole-road vehicles per day at a finalized piece; 0 where only its own direction is known.
pub const CROSS_SECTION_AADT: &str = "cross_section_aadt";

#[derive(Clone, Debug)]
pub struct Road {
    pub way_id: i64,
    pub segment_idx: i16,
    pub start: (f64, f64),
    pub end: (f64, f64),
    pub direction: u8,
    pub class: u8,
    pub lanes: u8,
    pub access: u8,
    pub tunnel: bool,
    /// 0 unknown, 1 rural, 2 urban (`noise_compute::defaults::BUILT_UP_*`).
    pub built_up: u8,
    /// A roundabout ring is one-way by OSM tagging, yet each point of it carries the circulating flow.
    pub roundabout: bool,
    pub country: SquareCountryCity,
    pub source_id: u16,
    pub observation_source_id: u16,
    pub provenance: Provenance,
    pub counts: [f64; 4],
    /// `pipeline/lib/road-observation.ts` order: 0 unknown, 1 directional, 2 both-directions,
    /// 3 allocated, 4 street-cross-section.
    pub basis: u8,
    pub estimated: u8,
    pub observation: String,
    pub corridor: String,
}

impl Road {
    pub fn midpoint(&self) -> (f64, f64) {
        ((self.start.0 + self.end.0) / 2.0, (self.start.1 + self.end.1) / 2.0)
    }
    pub fn mercator_scale(&self) -> f64 {
        1.0 / (self.midpoint().1 / grid::WEB_MERCATOR_RADIUS_M).cosh()
    }
    pub fn travel_vector(&self) -> (f64, f64) {
        let sign = if self.direction == 2 { -1.0 } else { 1.0 };
        ((self.end.0 - self.start.0) * sign, (self.end.1 - self.start.1) * sign)
    }
}

pub fn column<'a, T: Array + 'static>(batch: &'a RecordBatch, name: &str) -> Result<&'a T, String> {
    let column = batch.column_by_name(name).and_then(|v| v.as_any().downcast_ref::<T>())
        .ok_or_else(|| format!("missing or invalid road column {name}"))?;
    if column.null_count() != 0 { return Err(format!("null road column {name}")); }
    Ok(column)
}

pub fn load(path: &Path) -> Result<Vec<RecordBatch>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let reader = FileReader::try_new(file, None).map_err(|e| e.to_string())?;
    let schema = reader.schema();
    let mut batches = reader.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
    if batches.is_empty() { batches.push(RecordBatch::new_empty(schema)); }
    Ok(batches)
}

pub fn roads(batch: &RecordBatch) -> Result<Vec<Road>, String> {
    let finalized = batch.schema().metadata().get(CONTRACT).map(String::as_str) == Some("1");
    let ids = column::<Int64Array>(batch, "osm_id")?;
    let segments = column::<Int16Array>(batch, "segment_idx")?;
    let sx = column::<Int32Array>(batch, "start_gx")?;
    let sy = column::<Int32Array>(batch, "start_gy")?;
    let ex = column::<Int32Array>(batch, "end_gx")?;
    let ey = column::<Int32Array>(batch, "end_gy")?;
    let direction = column::<UInt8Array>(batch, "oneway")?;
    let class = column::<UInt8Array>(batch, "road_class")?;
    let lanes = column::<UInt8Array>(batch, "lanes")?;
    let access = column::<UInt8Array>(batch, "access")?;
    let tunnel = column::<BooleanArray>(batch, "tunnel")?;
    let built_up = column::<UInt8Array>(batch, "built_up")?;
    let junction = column::<UInt8Array>(batch, "junction")?;
    let country = column::<UInt16Array>(batch, "country_iso")?;
    let city = column::<UInt16Array>(batch, "city_id")?;
    let continent = column::<UInt8Array>(batch, "continent")?;
    let source = column::<UInt16Array>(batch, "source_id")?;
    let has_counts = COUNTS.iter().any(|name| batch.column_by_name(name).is_some());
    let counts = if has_counts || finalized {
        Some(COUNTS.map(|name| column::<Float64Array>(batch, name)).into_iter()
            .collect::<Result<Vec<_>, _>>()?)
    } else { None };
    let basis = if finalized { None } else if has_counts {
        Some(column::<UInt8Array>(batch, "traffic_count_basis")?)
    } else { None };
    let observations = if !finalized && has_counts {
        Some(column::<StringArray>(batch, "traffic_observation_id")?)
    } else { None };
    let origins = if !finalized && has_counts {
        Some(column::<UInt16Array>(batch, "traffic_observation_source")?)
    } else { None };
    let estimated = if has_counts || finalized {
        Some(column::<UInt8Array>(batch, "traffic_estimated")?)
    } else { None };
    let text = |name, i| batch.column_by_name(name).and_then(|v| v.as_any().downcast_ref::<StringArray>())
        .filter(|v| !v.is_null(i)).map(|v| v.value(i).trim()).unwrap_or("");
    (0..batch.num_rows()).map(|i| {
        let count_values = std::array::from_fn(|n| counts.as_ref().map(|v| v[n].value(i)).unwrap_or(0.0));
        let count_basis = basis.map(|v| v.value(i)).unwrap_or(if finalized { 3 } else { 0 });
        let observation = observations.map(|v| v.value(i)).unwrap_or("").to_owned();
        let status = estimated.map(|v| v.value(i)).unwrap_or(15);
        if direction.value(i) > 2 || class.value(i) > 12 || count_basis > 4 || status > 15
            || count_values.iter().any(|v| !v.is_finite() || *v < 0.0)
            || (source.value(i) != 0 && count_basis != 3 && observation.is_empty()) {
            return Err(format!("invalid road traffic at {}:{}", ids.value(i), segments.value(i)));
        }
        let reference = text("ref", i);
        let start = grid::grid_to_meters(sx.value(i), sy.value(i));
        let mut end = grid::grid_to_meters(ex.value(i), ey.value(i));
        end.0 += ((start.0 - end.0) / grid::EARTH_CIRCUMFERENCE_M).round()
            * grid::EARTH_CIRCUMFERENCE_M;
        Ok(Road {
            way_id: ids.value(i), segment_idx: segments.value(i),
            start, end,
            direction: direction.value(i), class: class.value(i), lanes: lanes.value(i),
            access: access.value(i), tunnel: tunnel.value(i), built_up: built_up.value(i),
            roundabout: junction.value(i) != 0,
            country: baked_square_country_city(country.value(i), city.value(i), continent.value(i)),
            source_id: source.value(i),
            observation_source_id: origins.map(|v| v.value(i)).unwrap_or(source.value(i)),
            provenance: provenance_of(origins.map(|v| v.value(i)).unwrap_or(source.value(i))),
            counts: count_values, basis: count_basis, estimated: status, observation,
            corridor: if reference.is_empty() { text("name", i) } else { reference }.to_uppercase(),
        })
    }).collect()
}

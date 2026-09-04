//! Load typed z30 airport geometry and resolve observed airport identity.

use crate::spatial::square_directories;
use anyhow::{Context, Result};
use arrow::{array::*, ipc::reader::FileReader, record_batch::RecordBatch};
use noise_compute::{propagation::geo::flat_dist, types::AirportArea};
use std::{fs::File, io::BufReader, path::Path};

/// `airport_areas.arrow` aeroway_type for an `aeroway = aerodrome`
/// polygon (the only value that anchors traffic identity). Apron /
/// taxi polygons live in the same arrow but match other aeroway_type
/// values.
pub(crate) const AERODROME_AEROWAY_TYPE: u8 = 5;

pub(crate) const NEAREST_AERODROME_FLOOR_M: f64 = 6000.0;

/// Multiplier on the polygon's equivalent radius (√area/π) for the
/// nearest-aerodrome snap window. 1.5 gives a small buffer past the
/// painted polygon for taxiway / runway approach segments.
pub(crate) const NEAREST_AERODROME_RADIUS_MULT: f64 = 1.5;

pub(crate) fn nearest_aerodrome_within(
    lat: f64,
    lon: f64,
    areas: &[AirportArea],
) -> Option<&AirportArea> {
    let mut best: Option<(&AirportArea, f64)> = None;
    for area in areas {
        if area.aeroway_type != AERODROME_AEROWAY_TYPE {
            continue;
        }
        if area.airport_key.is_empty() && area.name.is_empty() {
            continue;
        }
        let radius = aerodrome_radius_m(area);
        let dist = flat_dist(lat, lon, area.centroid_lat, area.centroid_lon);
        if dist > radius {
            continue;
        }
        if best.map(|(_, d)| dist < d).unwrap_or(true) {
            best = Some((area, dist));
        }
    }
    best.map(|(a, _)| a)
}

/// Centroid-radius used by the aerodrome gates: `max(6 km floor, √(area/π) × 1.5)`,
/// with a 500 m fallback when `area_m2` is unknown. The single source of this
/// formula so the spatial index (`airport_index`) and the naive gates can't drift.
pub(crate) fn aerodrome_radius_m(area: &AirportArea) -> f64 {
    let area_radius = if area.area_m2 > 0.0 {
        (area.area_m2 as f64 / std::f64::consts::PI).sqrt()
    } else {
        500.0
    };
    NEAREST_AERODROME_FLOOR_M.max(area_radius * NEAREST_AERODROME_RADIUS_MULT)
}

fn batches(path: &Path) -> Result<Vec<RecordBatch>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("open {}", path.display())),
    };
    let reader = FileReader::try_new(BufReader::new(file), None)?;
    let schema = reader.schema();
    let mut batches = reader
        .map(|batch| batch.map_err(Into::into))
        .collect::<Result<Vec<_>>>()?;
    if batches.is_empty() {
        batches.push(RecordBatch::new_empty(schema));
    }
    Ok(batches)
}

pub(crate) fn column<'a, T: Array + 'static>(batch: &'a RecordBatch, name: &str) -> Result<&'a T> {
    batch
        .column_by_name(name)
        .and_then(|array| array.as_any().downcast_ref())
        .ok_or_else(|| anyhow::anyhow!("missing or wrong-typed airport column {name}"))
}

fn grid_lonlat(gx: i32, gy: i32) -> (f64, f64) {
    let (x, y) = grid::grid_to_meters(gx, gy);
    grid::poly::meters_to_lonlat(x, y)
}

pub fn read_airport_areas(path: &Path) -> Result<Vec<AirportArea>> {
    let mut out = Vec::new();
    for batch in batches(path)? {
        let ids = crate::arrow_io::required_column::<Int64Array>(&batch, "osm_id")?;
        let gx = crate::arrow_io::required_column::<Int32Array>(&batch, "centroid_gx")?;
        let gy = crate::arrow_io::required_column::<Int32Array>(&batch, "centroid_gy")?;
        let kind = crate::arrow_io::required_column::<UInt8Array>(&batch, "aeroway_type")?;
        let name = column::<StringArray>(&batch, "name")?;
        let icao = column::<StringArray>(&batch, "icao")?;
        let iata = column::<StringArray>(&batch, "iata")?;
        let geometry = column::<BinaryArray>(&batch, "geom")?;
        let area = column::<Float32Array>(&batch, "area_m2")?;
        for row in 0..batch.num_rows() {
            let label = |values: &StringArray| {
                if values.is_null(row) {
                    String::new()
                } else {
                    values.value(row).trim().to_owned()
                }
            };
            let name = label(name);
            let key = [label(icao), label(iata), name.clone()]
                .into_iter()
                .find(|value| !value.is_empty())
                .unwrap_or_default();
            let (lon, lat) = grid_lonlat(gx.value(row), gy.value(row));
            let ring = if geometry.is_null(row) {
                Vec::new()
            } else {
                grid::poly::decode_grid_poly(geometry.value(row)).ok_or_else(|| {
                    anyhow::anyhow!("invalid airport polygon in {} row {row}", path.display())
                })?
            };
            out.push(AirportArea::new(
                ids.value(row),
                kind.value(row),
                name,
                key,
                lat,
                lon,
                ring,
                if area.is_null(row) {
                    0.0
                } else {
                    area.value(row)
                },
            ));
        }
    }
    Ok(out)
}

pub struct AirportLineRow {
    pub osm_id: u64,
    pub segment_idx: u16,
    pub start_lat: f32,
    pub start_lon: f32,
    pub end_lat: f32,
    pub end_lon: f32,
    pub grid: ((i32, i32), (i32, i32)),
    pub length_m: f32,
    pub aeroway_type: u8,
}

pub fn read_airport_lines(path: &Path) -> Result<Vec<AirportLineRow>> {
    let mut out = Vec::new();
    for batch in batches(path)? {
        let ids = crate::arrow_io::required_column::<Int64Array>(&batch, "osm_id")?;
        let indices = crate::arrow_io::required_column::<Int16Array>(&batch, "segment_idx")?;
        let sx = crate::arrow_io::required_column::<Int32Array>(&batch, "start_gx")?;
        let sy = crate::arrow_io::required_column::<Int32Array>(&batch, "start_gy")?;
        let ex = crate::arrow_io::required_column::<Int32Array>(&batch, "end_gx")?;
        let ey = crate::arrow_io::required_column::<Int32Array>(&batch, "end_gy")?;
        let length = crate::arrow_io::required_column::<Float32Array>(&batch, "length_m")?;
        let kind = crate::arrow_io::required_column::<UInt8Array>(&batch, "aeroway_type")?;
        for row in 0..batch.num_rows() {
            let (start_lon, start_lat) = grid_lonlat(sx.value(row), sy.value(row));
            let (end_lon, end_lat) = grid_lonlat(ex.value(row), ey.value(row));
            anyhow::ensure!(
                indices.value(row) >= 0,
                "negative airport segment index in {}",
                path.display()
            );
            out.push(AirportLineRow {
                osm_id: ids.value(row) as u64,
                segment_idx: indices.value(row) as u16,
                start_lat: start_lat as f32,
                start_lon: start_lon as f32,
                end_lat: end_lat as f32,
                end_lon: end_lon as f32,
                grid: (
                    (sx.value(row), sy.value(row)),
                    (ex.value(row), ey.value(row)),
                ),
                length_m: length.value(row),
                aeroway_type: kind.value(row),
            });
        }
    }
    Ok(out)
}

pub fn read_global_airports(root: &Path) -> Result<Vec<AirportArea>> {
    let mut areas = Vec::new();
    for (_, path) in square_directories(root)? {
        areas.extend(
            read_airport_areas(&path.join("airport_areas.arrow"))?
                .into_iter()
                .filter(|area| area.aeroway_type == AERODROME_AEROWAY_TYPE),
        );
    }
    areas.sort_by_key(|area| area.osm_id);
    areas.dedup_by_key(|area| area.osm_id);
    Ok(areas)
}

pub fn read_global_airport_lines(root: &Path) -> Result<Vec<AirportLineRow>> {
    let mut lines = Vec::new();
    for (_, path) in square_directories(root)? {
        lines.extend(read_airport_lines(&path.join("airport_lines.arrow"))?);
    }
    lines.sort_by_key(|line| (line.osm_id, line.segment_idx));
    lines.dedup_by_key(|line| (line.osm_id, line.segment_idx));
    Ok(lines)
}

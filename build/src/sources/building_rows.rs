//! dev4's `structures.arrow` as the building conversion reads it: its OSM buildings as rows, and
//! the places mapped inside its footprints placed outside them and hosted by their buildings.

use super::cells::{Z30Ring, ring_cell};
use super::outside::OutsidePlacer;
use super::venues::VenueSite;
use crate::dev4::{Square, cell, column, degrees_to_z30, positive, text};
use crate::structures::{decode_parts, footprint_id};
use arrow_array::{
    Array, BinaryArray, Float32Array, Int32Array, Int64Array, RecordBatch, StringArray, UInt8Array,
    UInt32Array,
};
use std::collections::HashMap;

const KIND_BUILDING: u8 = 0;
/// dev4 `height_source` of an emission-only ground activity (no structure stands there).
const HEIGHT_SOURCE_GROUND_ACTIVITY: u8 = 7;

/// One building row as the conversion reads it.
pub struct BuildingRow {
    pub osm_id: i64,
    /// The emission centroid, else the screening centroid.
    pub centroid: (i32, i32),
    pub ring: Z30Ring,
    /// The mapped height tag, 0 when none.
    pub height_tag_m: f64,
    /// The mapped floor count, 0 when none.
    pub floors: u8,
    /// dev4's storeys for the dwellings (its demand storeys, else the floors), 0 when none.
    pub storeys: u8,
    pub area_m2: Option<f64>,
    pub class: u8,
    pub ground_activity: bool,
    pub name: String,
    pub address: String,
    pub footprint_id: u64,
}

/// The places mapped in a square, each placed outside the footprint it stands in (OpenStreetMap's
/// and Overture's alike: both screen) and hosted by the bar's, restaurant's or café's building it
/// stands in (`hospitality`: footprint id to that building's key): what the sources hear and the
/// venue-seating fit counts.
pub fn host_places(
    batches: &[RecordBatch],
    (square, hospitality): (Square, &HashMap<u64, u64>),
    venues: &[VenueSite],
) -> Result<OutsidePlacer, String> {
    let mut placer = OutsidePlacer::new(
        venues
            .iter()
            .map(|site| degrees_to_z30(site.lat, site.lon))
            .collect(),
        venues.first().map_or(0.0, |site| site.lat),
    );
    if venues.is_empty() {
        return Ok(placer);
    }
    for batch in batches {
        let kinds = column::<UInt8Array>(batch, "kind")?;
        let outlines = column::<BinaryArray>(batch, "geom")?;
        let ordinals = column::<UInt32Array>(batch, "screening_ordinal")?;
        let centroid = [
            column::<Int32Array>(batch, "centroid_gx")?,
            column::<Int32Array>(batch, "centroid_gy")?,
        ];
        for row in 0..batch.num_rows() {
            if kinds.value(row) != KIND_BUILDING
                || !outlines.is_valid(row)
                || !centroid.iter().all(|values| values.is_valid(row))
                || !placer.near((centroid[0].value(row), centroid[1].value(row)))
            {
                continue;
            }
            let host = cell(ordinals, row)
                .and_then(|ordinal| hospitality.get(&footprint_id(square, ordinal)).copied());
            for part in decode_parts(outlines.value(row)).unwrap_or_default() {
                placer.offer(part, host);
            }
        }
    }
    Ok(placer)
}

pub fn read_batch(batch: &RecordBatch, square: Square) -> Result<Vec<BuildingRow>, String> {
    let bytes = |name: &str| column::<UInt8Array>(batch, name);
    let (kinds, floors, storeys) = (bytes("kind")?, bytes("floors")?, bytes("storeys")?);
    let (classes, height_sources) = (bytes("building_type")?, bytes("height_source")?);
    let integers = |name: &str| column::<Int32Array>(batch, name);
    let centroid = [integers("centroid_gx")?, integers("centroid_gy")?];
    let emission_centroid = [
        integers("emission_centroid_gx")?,
        integers("emission_centroid_gy")?,
    ];
    let floats = |name: &str| column::<Float32Array>(batch, name);
    let (heights, areas) = (floats("height")?, floats("area_m2")?);
    let binaries = |name: &str| column::<BinaryArray>(batch, name);
    let (rings, geometry) = (binaries("emission_geom")?, binaries("geom")?);
    let texts = |name: &str| column::<StringArray>(batch, name);
    let (names, streets, numbers) = (
        texts("name")?,
        texts("addr_street")?,
        texts("addr_housenumber")?,
    );
    let (ids, ordinals) = (
        column::<Int64Array>(batch, "osm_id")?,
        column::<UInt32Array>(batch, "screening_ordinal")?,
    );
    let mut rows = Vec::new();
    for row in 0..batch.num_rows() {
        let Some(osm_id) = cell(ids, row) else {
            continue;
        };
        if kinds.value(row) != KIND_BUILDING {
            continue;
        }
        let emission_centroid_known = emission_centroid.iter().all(|values| values.is_valid(row));
        let [x, y] = if emission_centroid_known {
            emission_centroid
        } else {
            centroid
        };
        let footprint_id = match (geometry.is_valid(row), cell(ordinals, row)) {
            (false, _) => 0,
            (true, Some(ordinal)) => footprint_id(square, ordinal),
            (true, None) => return Err(format!("row {row} has geometry but no screening_ordinal")),
        };
        rows.push(BuildingRow {
            osm_id,
            centroid: (x.value(row), y.value(row)),
            ring: ring_cell(rings, row),
            height_tag_m: positive(heights, row).unwrap_or(0.0),
            floors: cell(floors, row).unwrap_or(0),
            storeys: cell(storeys, row)
                .filter(|&count| count > 0)
                .or_else(|| cell(floors, row))
                .unwrap_or(0),
            area_m2: positive(areas, row),
            class: cell(classes, row).unwrap_or(0),
            ground_activity: cell(height_sources, row) == Some(HEIGHT_SOURCE_GROUND_ACTIVITY),
            name: text(names, row).to_string(),
            address: match text(streets, row) {
                "" => String::new(),
                street => format!("{street} {}", text(numbers, row))
                    .trim_end()
                    .to_string(),
            },
            footprint_id,
        });
    }
    Ok(rows)
}

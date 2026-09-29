//! Buildings of dev4's `structures.arrow` (kind 0 rows with an OSM id) as sources: the settlement
//! area law over the gross floor area, at half the building's height (its height tag, else floors
//! x 3 m, else 8 m) or 1.5 m for a ground activity (an emission-only area); cells of 30 m above
//! 2,000 m2. Each carries the id of its screening footprint, so it never screens itself.

use super::cells::{
    AUDIBILITY_FLOOR_DBA, Site, Z30Ring, decode_z30_ring, push_site_points, resolve_area_m2,
    site_points,
};
use super::{Converted, column, group_key};
use crate::dev4::{Dev4, Square, require_stamp, z30_corner_degrees};
use crate::structures::footprint_id;
use arrow_array::{
    Array, BinaryArray, Float32Array, Int32Array, Int64Array, RecordBatch, StringArray, UInt8Array,
    UInt32Array,
};
use physics::emission::settlement::building_sound_power;
use physics::emission::spectrum::SoundPower;
use serde_json::json;
use tiles::sources::{Attribute, GROUND_FROM_TERRAIN, Layer};

const KIND_BUILDING: u8 = 0;
/// dev4 `height_source` of an emission-only ground activity (no structure stands there).
const HEIGHT_SOURCE_GROUND_ACTIVITY: u8 = 7;
const GROUND_ACTIVITY_SOURCE_HEIGHT_M: f64 = 1.5;
/// Storey height and the height of a building with neither a height nor a floor count (dev4).
const FLOOR_HEIGHT_M: f64 = 3.0;
const DEFAULT_HEIGHT_M: f64 = 8.0;
/// The tallest building on Earth (Burj Khalifa); a taller mapped value is a tag error.
const TALLEST_BUILDING_M: f64 = 828.0;
/// The footprint of a building with neither a stored area nor a ring.
const DEFAULT_FOOTPRINT_M2: f64 = 100.0;
const SINGLE_POINT_UP_TO_M2: f64 = 2_000.0;
const CELL_M: f64 = 30.0;

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
    pub area_m2: Option<f64>,
    pub class: u8,
    pub ground_activity: bool,
    pub name: String,
    pub address: String,
    pub footprint_id: u64,
}

/// What a building emits and the geometry it was computed for.
#[derive(Debug, PartialEq)]
pub struct BuildingEmission {
    pub sound: SoundPower,
    pub source_height_m: f64,
    pub height_m: f64,
    pub floors: u8,
    pub area_m2: f64,
}

/// A building's emission; `None` for silent classes and inaudible levels.
pub fn building_emission(row: &BuildingRow) -> Option<BuildingEmission> {
    let (height_m, floors) = if row.ground_activity {
        (0.0, 0)
    } else {
        let height_m = if row.height_tag_m > 0.0 {
            row.height_tag_m
        } else if row.floors > 0 {
            f64::from(row.floors) * FLOOR_HEIGHT_M
        } else {
            DEFAULT_HEIGHT_M
        }
        .min(TALLEST_BUILDING_M);
        let floors = if row.floors > 0 {
            row.floors
        } else {
            (height_m / FLOOR_HEIGHT_M).ceil() as u8
        };
        (height_m, floors)
    };
    let area_m2 = resolve_area_m2(row.area_m2, &row.ring, DEFAULT_FOOTPRINT_M2);
    let sound = building_sound_power(row.class, area_m2, floors)?;
    let source_height_m = if row.ground_activity {
        GROUND_ACTIVITY_SOURCE_HEIGHT_M
    } else {
        height_m / 2.0
    };
    (sound.day_dba >= AUDIBILITY_FLOOR_DBA).then_some(BuildingEmission {
        sound,
        source_height_m,
        height_m,
        floors,
        area_m2,
    })
}

fn class_label(class: u8) -> &'static str {
    const LABELS: [&str; 14] = [
        "residential_multi",
        "commercial",
        "warehouse",
        "education",
        "healthcare",
        "worship",
        "hotel",
        "garage",
        "farm",
        "public",
        "silent",
        "residential_house",
        "food_retail",
        "restaurant_bar",
    ];
    LABELS.get(usize::from(class)).copied().unwrap_or("default")
}

/// Converts the building rows of one dev4 square; returns how many rows emit.
pub fn convert(dev4: &Dev4, square: Square, out: &mut Vec<Converted>) -> Result<usize, String> {
    let Some(table) = dev4.table(square, "structures.arrow")? else {
        return Ok(0);
    };
    let context = |error: String| format!("structures.arrow of {square:?}: {error}");
    for (key, value) in [("grid", "z30"), ("structures_contract", "structures_v5")] {
        require_stamp(&table, key, value).map_err(context)?;
    }
    let mut emitting = 0;
    for batch in &table.batches {
        for row in read_batch(batch, square).map_err(context)? {
            let Some(emission) = building_emission(&row) else {
                continue;
            };
            let site = Site {
                centroid: z30_corner_degrees(row.centroid.0, row.centroid.1),
                ring: &row.ring,
                area_m2: emission.area_m2,
                single_point_up_to_m2: SINGLE_POINT_UP_TO_M2,
                cell_m: CELL_M,
            };
            let decibels = |level: f64| (level * 10.0).round() / 10.0;
            let attribute = Attribute {
                layer: Layer::Building,
                height_m: emission.source_height_m,
                ground_percent: GROUND_FROM_TERRAIN,
                platform_half_width_m: 0.0,
                exclusion_radius_m: 0.0,
                footprint_id: row.footprint_id,
                group_key: group_key(&["building", &row.osm_id.to_string()]),
                emission: emission.sound.band_levels_db(),
                display: json!([
                    row.name,
                    class_label(row.class),
                    decibels(emission.height_m),
                    emission.floors,
                    emission.area_m2.round(),
                    row.address,
                    decibels(emission.sound.day_dba)
                ])
                .to_string(),
            };
            push_site_points(&site_points(&site), emission.area_m2, &attribute, out);
            emitting += 1;
        }
    }
    Ok(emitting)
}

fn read_batch(batch: &RecordBatch, square: Square) -> Result<Vec<BuildingRow>, String> {
    let (kinds, ids) = (
        column::<UInt8Array>(batch, "kind")?,
        column::<Int64Array>(batch, "osm_id")?,
    );
    let centroid = [
        column::<Int32Array>(batch, "centroid_gx")?,
        column::<Int32Array>(batch, "centroid_gy")?,
    ];
    let emission_centroid = [
        column::<Int32Array>(batch, "emission_centroid_gx")?,
        column::<Int32Array>(batch, "emission_centroid_gy")?,
    ];
    let (heights, floors) = (
        column::<Float32Array>(batch, "height")?,
        column::<UInt8Array>(batch, "floors")?,
    );
    let (areas, classes) = (
        column::<Float32Array>(batch, "area_m2")?,
        column::<UInt8Array>(batch, "building_type")?,
    );
    let (height_sources, rings) = (
        column::<UInt8Array>(batch, "height_source")?,
        column::<BinaryArray>(batch, "emission_geom")?,
    );
    let (geometry, ordinals) = (
        column::<BinaryArray>(batch, "geom")?,
        column::<UInt32Array>(batch, "screening_ordinal")?,
    );
    let text = |name: &str| column::<StringArray>(batch, name);
    let (names, streets, numbers) = (
        text("name")?,
        text("addr_street")?,
        text("addr_housenumber")?,
    );
    fn string(values: &StringArray, row: usize) -> &str {
        if values.is_valid(row) {
            values.value(row)
        } else {
            ""
        }
    }
    let mut rows = Vec::new();
    for row in
        (0..batch.num_rows()).filter(|&row| kinds.value(row) == KIND_BUILDING && ids.is_valid(row))
    {
        let [x, y] = if emission_centroid.iter().all(|values| values.is_valid(row)) {
            emission_centroid
        } else {
            centroid
        };
        let footprint_id = if geometry.is_valid(row) {
            if !ordinals.is_valid(row) {
                return Err(format!("row {row} has geometry but no screening_ordinal"));
            }
            footprint_id(square, ordinals.value(row))
        } else {
            0
        };
        let street = string(streets, row);
        let address = if street.is_empty() {
            String::new()
        } else {
            format!("{street} {}", string(numbers, row))
        };
        rows.push(BuildingRow {
            osm_id: ids.value(row),
            centroid: (x.value(row), y.value(row)),
            ring: rings
                .is_valid(row)
                .then(|| decode_z30_ring(rings.value(row)))
                .flatten()
                .unwrap_or_default(),
            height_tag_m: if heights.is_valid(row) {
                f64::from(heights.value(row))
            } else {
                0.0
            },
            floors: if floors.is_valid(row) {
                floors.value(row)
            } else {
                0
            },
            area_m2: (areas.is_valid(row) && areas.value(row) > 0.0)
                .then(|| f64::from(areas.value(row))),
            class: if classes.is_valid(row) {
                classes.value(row)
            } else {
                0
            },
            ground_activity: height_sources.is_valid(row)
                && height_sources.value(row) == HEIGHT_SOURCE_GROUND_ACTIVITY,
            name: string(names, row).to_string(),
            address: address.trim_end().to_string(),
            footprint_id,
        });
    }
    Ok(rows)
}

#[cfg(test)]
#[path = "building_tests.rs"]
mod tests;

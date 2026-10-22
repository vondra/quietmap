//! AIS vessel-density cells of dev4's `ships.arrow` as area sources: each cell's ships spread over
//! the water of its square footprint in sub-cells of about 250 m (a quay sees the nearest water at
//! its distance, not the whole 1 km cell at its centre), each a share of the cell by its water, at
//! a wet sample (ESA WorldCover's water, `water`): a cell over a coast spread over its land put
//! ships in the streets (a beach: the marina's cell 185 m from the promenade on
//! its houses, 40 % of the place's loudness). A cell all on land has none. At the height of the
//! class carrying most of the energy.

use super::cells::{
    AUDIBILITY_FLOOR_DBA, M_PER_DEG_LAT, M_PER_DEG_LON_EQ, SitePoint, push_site_points,
};
use super::water::WaterTiles;
use super::{Converted, fields, group_key};
use crate::dev4::{Dev4, Square, column, require_stamp, z30_corner_degrees};
use arrow_array::{Float32Array, Int32Array, UInt16Array};
use physics::emission::ships::ship_cell_sound_power;
use serde_json::json;
use tiles::sources::{Attribute, GROUND_FROM_TERRAIN, Layer};

/// A cell's sub-cells are as near this side (m) as a whole number of them along its side makes them.
const SUB_CELL_M: f64 = 250.0;
/// Water samples along a sub-cell's side (50 m apart; WorldCover's cells are 90 m).
const WATER_SAMPLES: usize = 5;

/// Where a cell's ships sail: its square of `side_m` around `centre` (latitude, longitude) cut into
/// equal sub-cells of about 250 m, each with the area of its water (`share` of each sample) at its
/// wet sample nearest its water's centre (a centre between two piers is on land); the dry ones out.
fn water_points(
    centre: (f64, f64),
    side_m: f64,
    mut share: impl FnMut(f64, f64) -> Result<f64, String>,
) -> Result<Vec<SitePoint>, String> {
    let sub_cells = (side_m / SUB_CELL_M).round().max(1.0) as usize;
    let samples = sub_cells * WATER_SAMPLES;
    let step_m = side_m / samples as f64;
    let metres_per_lon = M_PER_DEG_LON_EQ * centre.0.to_radians().cos().max(0.01);
    let offset = |index: usize| (index as f64 + 0.5 - samples as f64 / 2.0) * step_m;
    let mut points = Vec::new();
    for sub_row in 0..sub_cells {
        for sub_column in 0..sub_cells {
            let mut wet = Vec::new();
            for row in sub_row * WATER_SAMPLES..(sub_row + 1) * WATER_SAMPLES {
                for column in sub_column * WATER_SAMPLES..(sub_column + 1) * WATER_SAMPLES {
                    let (north_m, east_m) = (offset(row), offset(column));
                    let (lat, lon) = (
                        centre.0 + north_m / M_PER_DEG_LAT,
                        centre.1 + east_m / metres_per_lon,
                    );
                    let water = share(lat, lon)?;
                    if water > 0.0 {
                        wet.push((north_m, east_m, lat, lon, water));
                    }
                }
            }
            let total: f64 = wet.iter().map(|sample| sample.4).sum();
            if total == 0.0 {
                continue;
            }
            let mean = |of: fn(&(f64, f64, f64, f64, f64)) -> f64| {
                wet.iter().map(|sample| of(sample) * sample.4).sum::<f64>() / total
            };
            let (north_m, east_m) = (mean(|sample| sample.0), mean(|sample| sample.1));
            let distance =
                |sample: &(f64, f64, f64, f64, f64)| (sample.0 - north_m).hypot(sample.1 - east_m);
            let nearest = wet
                .iter()
                .min_by(|a, b| distance(a).total_cmp(&distance(b)))
                .expect("a wet sub-cell has a wet sample");
            points.push(SitePoint {
                lat: nearest.2,
                lon: (nearest.3 + 180.0).rem_euclid(360.0) - 180.0,
                area_m2: total * step_m * step_m,
            });
        }
    }
    Ok(points)
}

/// Converts the ship cells of one dev4 square; returns how many cells emit.
pub fn convert(
    dev4: &Dev4,
    square: Square,
    water: &mut WaterTiles,
    out: &mut Vec<Converted>,
) -> Result<usize, String> {
    let Some(table) = dev4.table(square, "ships.arrow")? else {
        return Ok(0);
    };
    let context = |error: String| format!("ships.arrow of {square:?}: {error}");
    for (key, value) in [("grid", "z30"), ("ships_contract", "ships_v1")] {
        require_stamp(&table, key, value).map_err(context)?;
    }
    let mut emitting = 0;
    for batch in &table.batches {
        let floats = |name: &str| column::<Float32Array>(batch, name).map_err(context);
        let (large, work, leisure) = (
            floats("hours_large")?,
            floats("hours_work")?,
            floats("hours_leisure")?,
        );
        let areas = floats("area_m2")?;
        let integers = |name: &str| column::<Int32Array>(batch, name).map_err(context);
        let (gx, gy) = (integers("centroid_gx")?, integers("centroid_gy")?);
        let sources = column::<UInt16Array>(batch, "source_id").map_err(context)?;
        for row in 0..batch.num_rows() {
            let hours_per_month = [large.value(row), work.value(row), leisure.value(row)];
            let Some((sound, class)) = ship_cell_sound_power(hours_per_month) else {
                continue;
            };
            let area_m2 = f64::from(areas.value(row));
            if sound.day_dba < AUDIBILITY_FLOOR_DBA || area_m2.is_nan() || area_m2 <= 0.0 {
                continue;
            }
            let centroid = z30_corner_degrees(gx.value(row), gy.value(row));
            // All sub-cells share the identity of their cell, as dev4 grouped them.
            let cell_id = (i64::from(gx.value(row)) << 32) | i64::from(gy.value(row) as u32);
            let attribute = Attribute {
                layer: Layer::Ship,
                height_m: class.source_height_m(),
                ground_percent: GROUND_FROM_TERRAIN,
                platform_half_width_m: 0.0,
                exclusion_radius_m: 0.0,
                footprint_id: 0,
                group_key: group_key(&["ship", &cell_id.to_string()]),
                emission: sound.band_levels_db(),
                display: fields(json!([
                    class.name(),
                    area_m2.round(),
                    hours_per_month.map(|hours| (f64::from(hours) * 100.0).round() / 100.0),
                    (sound.day_dba * 10.0).round() / 10.0,
                    sources.value(row)
                ])),
            };
            let on_water =
                water_points(centroid, area_m2.sqrt(), |lat, lon| water.share(lat, lon))?;
            let water_m2: f64 = on_water.iter().map(|point| point.area_m2).sum();
            if water_m2 > 0.0 {
                push_site_points(&on_water, water_m2, &attribute, out);
                emitting += 1;
            }
        }
    }
    Ok(emitting)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1 km cell is sixteen 250 m sub-cells of equal water share in the open sea; over a coast
    /// running north-south through its middle only its eight eastern ones, on the water; between
    /// two piers a sub-cell's point stays on a pier's water, not on the land between them.
    #[test]
    fn a_cell_sails_on_its_water_alone() {
        let centre = (54.0, 10.0);
        let east_of = |lon: f64| move |_: f64, at: f64| Ok(if at > lon { 1.0 } else { 0.0 });
        let sea = water_points(centre, 1_000.0, |_, _| Ok(1.0)).unwrap();
        assert_eq!(sea.len(), 16);
        assert!(
            sea.iter()
                .all(|point| (point.area_m2 - 62_500.0).abs() < 1e-6)
        );
        let coast = water_points(centre, 1_000.0, east_of(centre.1)).unwrap();
        assert_eq!(coast.len(), 8);
        assert!(coast.iter().all(|point| point.lon > centre.1));
        let total: f64 = coast.iter().map(|point| point.area_m2).sum();
        assert!((total - 500_000.0).abs() < 1e-6, "{total}");
        assert!(
            water_points(centre, 1_000.0, |_, _| Ok(0.0))
                .unwrap()
                .is_empty()
        );
        // Water only in the first and last sample column of every sub-cell: its water's centre is
        // its middle column, dry; the point goes to a wet sample.
        let step_lon = 50.0 / (M_PER_DEG_LON_EQ * 54f64.to_radians().cos());
        let west_edge = centre.1 - 500.0 / (M_PER_DEG_LON_EQ * 54f64.to_radians().cos());
        let piers = |_: f64, lon: f64| {
            let column = ((lon - west_edge) / step_lon).floor() as i64 % 5;
            Ok(if column == 0 || column == 4 { 1.0 } else { 0.0 })
        };
        for point in water_points(centre, 1_000.0, piers).unwrap() {
            assert_eq!(piers(point.lat, point.lon).unwrap(), 1.0, "{point:?}");
        }
    }
}

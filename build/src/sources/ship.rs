//! AIS vessel-density cells distributed over water along their native grid axes.
//!
//! A cell's tent reaches its neighbours' centres; on open water, neighbouring tents interpolate
//! density without seams. About 250 m sub-cells sample ESA WorldCover and place each source on a
//! wet sample. Each cell retains its total power over its wet support, which can include water
//! outside its original footprint. A support with no water emits nothing. Sources use the height
//! of the class carrying most energy.

use super::cells::{
    AUDIBILITY_FLOOR_DBA, M_PER_DEG_LAT, M_PER_DEG_LON_EQ, SitePoint, push_site_points,
};
use super::water::WaterTiles;
use super::{Converted, fields, group_key};
use crate::dev4::{Square, column, require_stamp, z30_corner_degrees};
use arrow_array::{Float32Array, Int32Array, UInt16Array};
use physics::emission::ships::ship_cell_sound_power;
use serde_json::json;
use tiles::sources::{Attribute, GROUND_FROM_TERRAIN, Layer};

/// A cell's sub-cells are as near this side (m) as a whole number of them along its span makes them.
const SUB_CELL_M: f64 = 250.0;
/// Water samples along a sub-cell's side (50 m apart; WorldCover's cells are 90 m).
const WATER_SAMPLES: usize = 5;
/// A water sample of a cell's span: where it lies (m north and east of the centre, degrees), its water
/// share and its tent weight.
struct Sample {
    north_m: f64,
    east_m: f64,
    lat: f64,
    lon: f64,
    water: f64,
    tent: f64,
}

/// Where a cell's ships sail: the span of one step (`axes`) each way around `centre` (latitude,
/// longitude) cut into equal sub-cells of about 250 m, each with its water (`share` of each sample)
/// times the tent, as an area, at its wet sample nearest that area's centre (a centre between two
/// piers is on land), and the area its wet samples span (m2), its extent near it; the dry ones out.
/// Over open sea the weighted areas add up to the cell's.
fn water_points(
    centre: (f64, f64),
    axes: [[f64; 2]; 2],
    mut share: impl FnMut(f64, f64) -> Result<f64, String>,
) -> Result<Vec<(SitePoint, f64)>, String> {
    let sub_cells =
        axes.map(|axis| (2.0 * axis[0].hypot(axis[1]) / SUB_CELL_M).round().max(2.0) as usize);
    let samples = sub_cells.map(|count| count * WATER_SAMPLES);
    let cell_m2 = (axes[0][0] * axes[1][1] - axes[0][1] * axes[1][0]).abs();
    let sample_m2 = 4.0 * cell_m2 / (samples[0] * samples[1]) as f64;
    let metres_per_lon = M_PER_DEG_LON_EQ * centre.0.to_radians().cos();
    // A sample's place along an axis, in steps from the centre.
    let along = |index: usize, count: usize| 2.0 * (index as f64 + 0.5) / count as f64 - 1.0;
    let mut points = Vec::new();
    for sub_row in 0..sub_cells[1] {
        for sub_column in 0..sub_cells[0] {
            let mut wet = Vec::new();
            for row in sub_row * WATER_SAMPLES..(sub_row + 1) * WATER_SAMPLES {
                for column in sub_column * WATER_SAMPLES..(sub_column + 1) * WATER_SAMPLES {
                    let (east, north) = (along(column, samples[0]), along(row, samples[1]));
                    let east_m = east * axes[0][0] + north * axes[1][0];
                    let north_m = east * axes[0][1] + north * axes[1][1];
                    let (lat, lon) = (
                        centre.0 + north_m / M_PER_DEG_LAT,
                        centre.1 + east_m / metres_per_lon,
                    );
                    let water = share(lat, lon)?;
                    if water > 0.0 {
                        let tent = (1.0 - east.abs()) * (1.0 - north.abs());
                        wet.push(Sample {
                            north_m,
                            east_m,
                            lat,
                            lon,
                            water,
                            tent,
                        });
                    }
                }
            }
            let weight = |sample: &Sample| sample.water * sample.tent;
            let total: f64 = wet.iter().map(weight).sum();
            if total == 0.0 {
                continue;
            }
            let mean = |of: fn(&Sample) -> f64| {
                wet.iter()
                    .map(|sample| of(sample) * weight(sample))
                    .sum::<f64>()
                    / total
            };
            let (north_m, east_m) = (mean(|sample| sample.north_m), mean(|sample| sample.east_m));
            let distance =
                |sample: &Sample| (sample.north_m - north_m).hypot(sample.east_m - east_m);
            let nearest = wet
                .iter()
                .min_by(|a, b| distance(a).total_cmp(&distance(b)))
                .expect("a wet sub-cell has a wet sample");
            let point = SitePoint {
                lat: nearest.lat,
                lon: (nearest.lon + 180.0).rem_euclid(360.0) - 180.0,
                area_m2: wet.iter().map(weight).sum::<f64>() * sample_m2,
            };
            points.push((point, wet.len() as f64 * sample_m2));
        }
    }
    Ok(points)
}

/// Pushes a cell's points (`water_points`), its ships shared by their weighted water, each heard as
/// near as its wet samples' extent, not a disk of its water (a speck of water carrying a cell's ships
/// is heard from a sample's distance, not 3 m); false for a cell without water.
fn push_cell(
    points: Vec<(SitePoint, f64)>,
    cell_m2: f64,
    attribute: &Attribute,
    out: &mut Vec<Converted>,
) -> bool {
    let water_m2: f64 = points.iter().map(|(point, _)| point.area_m2).sum();
    if water_m2 <= 0.0 {
        return false;
    }
    // Each point's area as its share of the cell's: `push_site_points` counts no area under 1 m2,
    // which the tent's corner of a speck of water is (0.016 m2), and gave such a cell 64 times its
    // ships.
    let (on_water, extents): (Vec<SitePoint>, Vec<f64>) = (points.into_iter())
        .map(|(point, extent_m2)| {
            let area_m2 = point.area_m2 * cell_m2 / water_m2;
            (SitePoint { area_m2, ..point }, extent_m2)
        })
        .unzip();
    let first = out.len();
    push_site_points(&on_water, cell_m2, attribute, out);
    for (item, extent_m2) in out[first..].iter_mut().zip(extents) {
        item.attribute.exclusion_radius_m = (extent_m2 / std::f64::consts::PI).sqrt();
    }
    true
}

/// Converts the normalized occupancy cells of one square; returns how many cells emit.
pub fn convert(
    ships: &std::path::Path,
    square: Square,
    water: &mut WaterTiles,
    out: &mut Vec<Converted>,
) -> Result<usize, String> {
    let table = crate::ships::table(ships, square)?;
    let grids = crate::ships::grids(&table)?;
    let context = |error: String| format!("ship-occupancy.arrow of {square:?}: {error}");
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
            let grid = grids
                .get(&sources.value(row))
                .ok_or("ship cell has no native grid")?;
            let axes = grid.axes(centroid, area_m2);
            let points = water_points(centroid, axes, |lat, lon| water.share(lat, lon))?;
            if push_cell(points, area_m2, &attribute, out) {
                emitting += 1;
            }
        }
    }
    Ok(emitting)
}

#[cfg(test)]
#[path = "ship_tests.rs"]
mod tests;

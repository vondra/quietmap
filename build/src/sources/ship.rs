//! AIS vessel-density cells of dev4's `ships.arrow` as area sources: each cell's ships spread over
//! the water of its square footprint in 250 m sub-cells (a quay sees the nearest water at its
//! distance, not the whole 1 km cell at its centre), each a share of the cell by its water, at its
//! water's centre (ESA WorldCover's, `water`): a cell over a coast spread over its land put ships in
//! the streets (a beach: the marina's cell 185 m from the promenade on its
//! houses, 40 % of the place's loudness). A cell all on land has none. At the height of the class
//! carrying most of the energy.

use super::cells::{
    AUDIBILITY_FLOOR_DBA, M_PER_DEG_LAT, M_PER_DEG_LON_EQ, Site, SitePoint, push_site_points,
    site_points, square_ring,
};
use super::water::WaterTiles;
use super::{Converted, fields, group_key};
use crate::dev4::{Dev4, Square, column, require_stamp, z30_corner_degrees};
use arrow_array::{Float32Array, Int32Array, UInt16Array};
use physics::emission::ships::ship_cell_sound_power;
use serde_json::json;
use tiles::sources::{Attribute, GROUND_FROM_TERRAIN, Layer};

const SUB_CELL_M: f64 = 250.0;
/// Samples along a side of a sub-cell weighing its water (50 m apart, WorldCover's cells 90 m).
const WATER_SAMPLES: u32 = 5;

/// A sub-cell's water: its area times its share of water, at the water's centre; `None` on land.
fn water_part(point: &SitePoint, water: &mut WaterTiles) -> Result<Option<SitePoint>, String> {
    let step_m = point.area_m2.sqrt() / f64::from(WATER_SAMPLES);
    let lat_step = step_m / M_PER_DEG_LAT;
    let lon_step = step_m / (M_PER_DEG_LON_EQ * point.lat.to_radians().cos().max(0.01));
    let middle = f64::from(WATER_SAMPLES - 1) / 2.0;
    let (mut total, mut lat_sum, mut lon_sum) = (0.0, 0.0, 0.0);
    for row in 0..WATER_SAMPLES {
        for column in 0..WATER_SAMPLES {
            let lat = point.lat + (f64::from(row) - middle) * lat_step;
            let lon = point.lon + (f64::from(column) - middle) * lon_step;
            let share = water.share(lat, lon)?;
            (total, lat_sum, lon_sum) =
                (total + share, lat_sum + share * lat, lon_sum + share * lon);
        }
    }
    Ok((total > 0.0).then(|| SitePoint {
        lat: lat_sum / total,
        lon: lon_sum / total,
        area_m2: point.area_m2 * total / f64::from(WATER_SAMPLES * WATER_SAMPLES),
    }))
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
            let ring = square_ring(centroid, area_m2.sqrt());
            let site = Site {
                centroid,
                ring: &ring,
                area_m2,
                single_point_up_to_m2: 0.0,
                cell_m: SUB_CELL_M,
            };
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
            let mut on_water = Vec::new();
            for point in site_points(&site) {
                on_water.extend(water_part(&point, water)?);
            }
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
    use super::super::cells::SitePoint;
    use super::*;

    /// A 1 km cell spreads over sixteen 250 m sub-cells of equal share, each 141 m in radius.
    #[test]
    fn a_square_kilometre_cell_spreads_over_sixteen_sub_cells() {
        let centroid = (54.0, 10.0);
        let ring = square_ring(centroid, 1_000.0);
        let site = Site {
            centroid,
            ring: &ring,
            area_m2: 1e6,
            single_point_up_to_m2: 0.0,
            cell_m: SUB_CELL_M,
        };
        let points: Vec<SitePoint> = site_points(&site);
        assert!((16..=25).contains(&points.len()), "{}", points.len());
        let total: f64 = points.iter().map(|point| point.area_m2).sum();
        assert!((total - 1e6).abs() < 1e-3);
        let largest = points.iter().map(|point| point.area_m2).fold(0.0, f64::max);
        assert!(largest <= 62_500.0 * 1.1, "{largest}");
    }
}

//! AIS vessel-density cells of dev4's `ships.arrow` as area sources: each cell's ships spread over
//! its square footprint in 250 m sub-cells (a quay sees the nearest water at its distance, not the
//! whole 1 km cell at its centre), at the height of the class carrying most of the energy.

use super::cells::{AUDIBILITY_FLOOR_DBA, Site, push_site_points, site_points, square_ring};
use super::{Converted, group_key};
use crate::dev4::{Dev4, Square, column, require_stamp, z30_corner_degrees};
use arrow_array::{Float32Array, Int32Array, UInt16Array};
use physics::emission::ships::ship_cell_sound_power;
use serde_json::json;
use tiles::sources::{Attribute, GROUND_FROM_TERRAIN, Layer};

const SUB_CELL_M: f64 = 250.0;

/// Converts the ship cells of one dev4 square; returns how many cells emit.
pub fn convert(dev4: &Dev4, square: Square, out: &mut Vec<Converted>) -> Result<usize, String> {
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
                display: json!([
                    class.name(),
                    area_m2.round(),
                    hours_per_month.map(|hours| (f64::from(hours) * 100.0).round() / 100.0),
                    (sound.day_dba * 10.0).round() / 10.0,
                    sources.value(row)
                ])
                .to_string(),
            };
            push_site_points(&site_points(&site), area_m2, &attribute, out);
            emitting += 1;
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

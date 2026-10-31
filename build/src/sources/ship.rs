//! AIS vessel-density cells of dev4's `ships.arrow` as area sources: each cell's ships spread over
//! the water within one of its grid steps of its centre each way, on its grid's own axes, weighted by
//! a tent that falls to nothing at its neighbours' centres: neighbouring cells add up to the density
//! bilinear between their centres, with no edges, each cell keeping its ships (1 km squares aligned
//! to north overlapped on EMODnet's grid, turned 15° at the Canary Islands, and lay side by side as
//! squares; owner 2026-10-31). Sub-cells of about 250 m (a quay sees the nearest water
//! at its distance, not the whole cell at its centre), each a share of the cell by its weighted
//! water, at a wet sample (ESA WorldCover's water, `water`): a cell over a coast spread over its land
//! put ships in the streets (a beach: the marina's cell 185 m from the promenade on its houses, 40 %
//! of the place's loudness). A cell all on land has none. At the height of the class carrying most
//! of the energy.

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

/// A cell's sub-cells are as near this side (m) as a whole number of them along its span makes them.
const SUB_CELL_M: f64 = 250.0;
/// Water samples along a sub-cell's side (50 m apart; WorldCover's cells are 90 m).
const WATER_SAMPLES: usize = 5;
/// dev4's vessel grids by `source_id`: EMODnet's 1 km cells of ETRS89-LAEA (EPSG:3035, centred at
/// 52°N 10°E) in European seas; Global Fishing Watch's 0.01° cells elsewhere.
const EMODNET_SOURCE: u16 = 9901;
const LAEA_CENTRE_DEG: (f64, f64) = (52.0, 10.0);
const GLOBAL_FISHING_WATCH_CELL_DEG: f64 = 0.01;

/// The LAEA grid's steps east and north on the ground at a point, each as metres east and north,
/// spanning `area_m2`: the projection's inverse Jacobian on the sphere. It keeps areas, not angles: at
/// the Canary Islands a step east leans 19° north and a step north 15° west, as the cells' centres in
/// `ships.arrow` lie (neighbours at bearings 71.6° and -14.5°).
fn laea_steps(centre: (f64, f64), area_m2: f64) -> [[f64; 2]; 2] {
    let (phi0, lon0) = (LAEA_CENTRE_DEG.0.to_radians(), LAEA_CENTRE_DEG.1);
    let project = |lat: f64, lon: f64| {
        let (phi, lambda) = (lat.to_radians(), (lon - lon0).to_radians());
        let cos_c = phi0.sin() * phi.sin() + phi0.cos() * phi.cos() * lambda.cos();
        let k = (2.0 / (1.0 + cos_c)).sqrt();
        let y = phi0.cos() * phi.sin() - phi0.sin() * phi.cos() * lambda.cos();
        [k * phi.cos() * lambda.sin(), k * y]
    };
    // The projection's change for 10 m east and 10 m north, its inverse the ground under a grid step.
    let metres_per_lon = M_PER_DEG_LON_EQ * centre.0.to_radians().cos();
    let at = project(centre.0, centre.1);
    let [east, north] = [
        project(centre.0, centre.1 + 10.0 / metres_per_lon),
        project(centre.0 + 10.0 / M_PER_DEG_LAT, centre.1),
    ]
    .map(|moved| [moved[0] - at[0], moved[1] - at[1]]);
    let det = east[0] * north[1] - north[0] * east[1];
    let steps = [
        [north[1] / det, -east[1] / det],
        [-north[0] / det, east[0] / det],
    ];
    let scale = (area_m2 * det.abs() / 100.0).sqrt();
    steps.map(|step| step.map(|metres| metres * 10.0 * scale))
}

/// A cell's grid steps east and north, each as metres east and north: EMODnet's on its LAEA grid,
/// Global Fishing Watch's a hundredth of a degree each way.
fn cell_axes(source: u16, centre: (f64, f64), area_m2: f64) -> [[f64; 2]; 2] {
    if source == EMODNET_SOURCE {
        return laea_steps(centre, area_m2);
    }
    let degree_m = [
        M_PER_DEG_LON_EQ * centre.0.to_radians().cos(),
        M_PER_DEG_LAT,
    ];
    [
        [GLOBAL_FISHING_WATCH_CELL_DEG * degree_m[0], 0.0],
        [0.0, GLOBAL_FISHING_WATCH_CELL_DEG * degree_m[1]],
    ]
}

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
            let axes = cell_axes(sources.value(row), centroid, area_m2);
            let points = water_points(centroid, axes, |lat, lon| water.share(lat, lon))?;
            if push_cell(points, area_m2, &attribute, out) {
                emitting += 1;
            }
        }
    }
    Ok(emitting)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NORTH_ALIGNED_KM: [[f64; 2]; 2] = [[1_000.0, 0.0], [0.0, 1_000.0]];

    /// Over open sea a 1 km cell is sixty-four 250 m sub-cells over 2 km, their weighted areas adding
    /// up to its 1 km2, most at its centre; the tents of a block of cells add up to the same water
    /// everywhere inside it (no edges); over a coast running north-south through its middle only
    /// the eastern half, on the water.
    #[test]
    fn a_cell_fades_into_its_neighbours_on_its_water_alone() {
        let centre = (54.0, 10.0);
        let sea = water_points(centre, NORTH_ALIGNED_KM, |_, _| Ok(1.0)).unwrap();
        assert_eq!(sea.len(), 64);
        let total: f64 = sea.iter().map(|(point, _)| point.area_m2).sum();
        assert!((total - 1e6).abs() < 1e-3, "{total}");
        let (low, high) = sea
            .iter()
            .fold((f64::INFINITY, 0.0f64), |(low, high), (point, _)| {
                (low.min(point.area_m2), high.max(point.area_m2))
            });
        assert!((high - 62_500.0 * 0.875f64.powi(2)).abs() < 1.0, "{high}");
        assert!((low - 62_500.0 * 0.125f64.powi(2)).abs() < 1.0, "{low}");
        assert!(
            sea.iter()
                .all(|(_, extent_m2)| (extent_m2 - 62_500.0).abs() < 1e-6)
        );
        // Nine cells 1 km apart: each 250 m of the middle cell holds 62,500 m2 of their water.
        let metres_per_lon = M_PER_DEG_LON_EQ * centre.0.to_radians().cos();
        let mut summed = std::collections::HashMap::new();
        for east in -1..=1 {
            for north in -1..=1 {
                let cell = (
                    centre.0 + f64::from(north) * 1_000.0 / M_PER_DEG_LAT,
                    centre.1 + f64::from(east) * 1_000.0 / metres_per_lon,
                );
                for (point, _) in water_points(cell, NORTH_ALIGNED_KM, |_, _| Ok(1.0)).unwrap() {
                    // Its 250 m sub-cell of the middle cell (a point leans to its own cell's centre).
                    let north_m = ((point.lat - centre.0) * M_PER_DEG_LAT).round() as i64;
                    let east_m = ((point.lon - centre.1) * metres_per_lon).round() as i64;
                    let sub_cell = (north_m.div_euclid(250), east_m.div_euclid(250));
                    *summed.entry(sub_cell).or_insert(0.0) += point.area_m2;
                }
            }
        }
        let inside: Vec<f64> = (summed.iter())
            .filter(|(sub_cell, _)| (-2..2).contains(&sub_cell.0) && (-2..2).contains(&sub_cell.1))
            .map(|(_, area)| *area)
            .collect();
        assert_eq!(inside.len(), 16);
        assert!(
            inside.iter().all(|area| (area - 62_500.0).abs() < 1.0),
            "{inside:?}"
        );
        let east_of = |lon: f64| move |_: f64, at: f64| Ok(if at > lon { 1.0 } else { 0.0 });
        let coast = water_points(centre, NORTH_ALIGNED_KM, east_of(centre.1)).unwrap();
        assert_eq!(coast.len(), 32);
        assert!(coast.iter().all(|(point, _)| point.lon > centre.1));
        let total: f64 = coast.iter().map(|(point, _)| point.area_m2).sum();
        assert!((total - 500_000.0).abs() < 1e-3, "{total}");
        assert!(
            water_points(centre, NORTH_ALIGNED_KM, |_, _| Ok(0.0))
                .unwrap()
                .is_empty()
        );
        // Water only in the first and last sample column of every sub-cell: its water's centre is
        // its middle column, dry; the point goes to a wet sample. Ten of its 25 samples are wet.
        let step_lon = 50.0 / metres_per_lon;
        let west_edge = centre.1 - 1_000.0 / metres_per_lon;
        let piers = |_: f64, lon: f64| {
            let column = ((lon - west_edge) / step_lon).floor() as i64 % 5;
            Ok(if column == 0 || column == 4 { 1.0 } else { 0.0 })
        };
        for (point, extent_m2) in water_points(centre, NORTH_ALIGNED_KM, piers).unwrap() {
            assert_eq!(piers(point.lat, point.lon).unwrap(), 1.0, "{point:?}");
            assert!((extent_m2 - 25_000.0).abs() < 1e-6 && point.area_m2 < 25_000.0);
        }
        // One wet sample at 1 % (the sample north-east of the centre): 25 m2 of water at its tent,
        // heard as near as its 50 m sample, not as a 25 m2 disk.
        let speck = water_points(centre, NORTH_ALIGNED_KM, |lat, lon| {
            let near =
                lat > centre.0 && lat - centre.0 < 3e-4 && lon > centre.1 && lon - centre.1 < 6e-4;
            Ok(if near { 0.01 } else { 0.0 })
        })
        .unwrap();
        assert_eq!(speck.len(), 1);
        assert!((speck[0].0.area_m2 - 25.0 * 0.975f64.powi(2)).abs() < 1e-6);
        assert!((speck[0].1 - 2_500.0).abs() < 1e-6);
        // Two wet samples of the outer north-east sub-cell, half wet at 775 m north and east and all
        // at 975 m: its point stands where the tent puts its ships, at the inner one (97.6 % of them).
        let two = water_points(centre, NORTH_ALIGNED_KM, |lat, lon| {
            let north_m = (lat - centre.0) * M_PER_DEG_LAT;
            let east_m = (lon - centre.1) * metres_per_lon;
            let at = |metres: f64| (north_m - metres).abs() < 1.0 && (east_m - metres).abs() < 1.0;
            Ok(if at(775.0) {
                0.5
            } else if at(975.0) {
                1.0
            } else {
                0.0
            })
        })
        .unwrap();
        assert_eq!(two.len(), 1);
        assert!(((two[0].0.lat - centre.0) * M_PER_DEG_LAT - 775.0).abs() < 1.0);
    }

    /// A cell gives off its ships whole however its water lies: over open sea, and when its only
    /// water is 1 % of the tent's outer corner sample (0.016 m2, which `push_site_points` alone
    /// would have counted as 1 m2: 64 times the cell).
    #[test]
    fn a_cell_gives_off_its_ships_whole() {
        let attribute = Attribute {
            layer: Layer::Ship,
            height_m: 5.0,
            ground_percent: GROUND_FROM_TERRAIN,
            platform_half_width_m: 0.0,
            exclusion_radius_m: 0.0,
            footprint_id: 0,
            group_key: 1,
            emission: [[90.0; tiles::sources::BANDS]; tiles::sources::PERIODS],
            display: Vec::new(),
        };
        let centre: (f64, f64) = (54.0, 10.0);
        let metres_per_lon = M_PER_DEG_LON_EQ * centre.0.to_radians().cos();
        let corner = |lat: f64, lon: f64| {
            let far = (lat - centre.0) * M_PER_DEG_LAT > 950.0
                && (lon - centre.1) * metres_per_lon > 950.0;
            Ok(if far { 0.01 } else { 0.0 })
        };
        let sea = |_: f64, _: f64| Ok(1.0);
        for (points, count) in [
            (water_points(centre, NORTH_ALIGNED_KM, sea).unwrap(), 64),
            (water_points(centre, NORTH_ALIGNED_KM, corner).unwrap(), 1),
        ] {
            let mut out = Vec::new();
            assert!(push_cell(points, 1e6, &attribute, &mut out));
            let energy: f64 = (out.iter())
                .map(|item| 10f64.powf(item.attribute.emission[0][0] / 10.0))
                .sum();
            assert_eq!(out.len(), count);
            assert!((10.0 * energy.log10() - 90.0).abs() < 1e-6, "{energy}");
        }
    }

    /// EMODnet's cells lie on their grid: at the Canary Islands their neighbours at bearings 71.6°
    /// (1,005 m) and -14.5° (996 m) in `ships.arrow`, square at its centre; Global Fishing Watch's
    /// are a hundredth of a degree each way, narrower east-west at 60°N.
    #[test]
    fn a_cell_lies_on_its_own_grid() {
        let bearing = |step: [f64; 2]| step[0].atan2(step[1]).to_degrees();
        let length = |step: [f64; 2]| step[0].hypot(step[1]);
        let [east, north] = cell_axes(EMODNET_SOURCE, (28.1, -15.4), 1e6);
        assert!((bearing(east) - 71.6).abs() < 0.5, "{east:?}");
        assert!((bearing(north) + 14.5).abs() < 0.5, "{north:?}");
        assert!((length(east) - 1_005.0).abs() < 10.0 && (length(north) - 996.0).abs() < 10.0);
        assert!((east[0] * north[1] - east[1] * north[0] - 1e6).abs() < 1e-3);
        let [east, north] = cell_axes(EMODNET_SOURCE, (52.0, 10.0), 1e6);
        assert!(
            east[1].abs() < 0.01 && north[0].abs() < 0.01,
            "{east:?} {north:?}"
        );
        assert!((east[0] - 1_000.0).abs() < 5.0 && (north[1] - 1_000.0).abs() < 5.0);
        let [east, north] = cell_axes(9902, (60.0, 25.0), 6e5);
        assert!((east[0] - 556.6).abs() < 0.1 && east[1] == 0.0, "{east:?}");
        assert!(
            (north[1] - 1_105.4).abs() < 0.1 && north[0] == 0.0,
            "{north:?}"
        );
    }
}

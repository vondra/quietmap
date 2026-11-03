//! Assign original OSM building trip ends once to their nearest eligible street in the original stable order.

use super::preparation_model::DemandModel;
use crate::{
    dev4::{column, read_table, require_stamp},
    roads::input::Road,
};
use anyhow::{Result, ensure};
use arrow_array::{Array, Float32Array, Int32Array, Int64Array, UInt8Array};
use std::{collections::HashMap, path::Path};

pub(super) fn lonlat(point: (f64, f64)) -> [f64; 2] {
    // The original frontage matcher multiplies and then divides; reassociation changes exact nearest-road ties.
    let radius = crate::osm::geometry::WEB_MERCATOR_RADIUS_M;
    [
        point.0 / radius * 180.0 / std::f64::consts::PI,
        (2.0 * (point.1 / radius).exp().atan() - std::f64::consts::FRAC_PI_2) * 180.0
            / std::f64::consts::PI,
    ]
}

pub(super) fn assign(
    path: &Path,
    roads: &[Road],
    eligible: &[usize],
    model: &DemandModel,
) -> Result<(Vec<[f64; 2]>, bool)> {
    let mut buildings = Vec::new();
    if let Some(table) = read_table(path).map_err(anyhow::Error::msg)? {
        require_stamp(&table, "structures_contract", "structures_v5")
            .map_err(anyhow::Error::msg)?;
        for batch in table.batches {
            macro_rules! c {
                ($ty:ident, $name:literal) => {
                    column::<$ty>(&batch, $name).map_err(anyhow::Error::msg)?
                };
            }
            let kinds = c!(UInt8Array, "kind");
            let ids = c!(Int64Array, "osm_id");
            let types = c!(UInt8Array, "building_type");
            let storeys = c!(UInt8Array, "storeys");
            let areas = c!(Float32Array, "area_m2");
            let x = c!(Int32Array, "centroid_gx");
            let y = c!(Int32Array, "centroid_gy");
            let ex = c!(Int32Array, "emission_centroid_gx");
            let ey = c!(Int32Array, "emission_centroid_gy");
            for i in 0..batch.num_rows() {
                if kinds.value(i) != 0 || ids.is_null(i) {
                    continue;
                }
                ensure!(
                    types.is_valid(i)
                        && types.value(i) <= 13
                        && storeys.is_valid(i)
                        && storeys.value(i) > 0,
                    "invalid building trip attributes"
                );
                ensure!(
                    ex.is_valid(i) == ey.is_valid(i),
                    "partial building emission centroid"
                );
                ensure!(
                    areas.is_null(i) || (areas.value(i).is_finite() && areas.value(i) >= 0.0),
                    "invalid building area"
                );
                let gx = if ex.is_valid(i) {
                    ex.value(i)
                } else {
                    x.value(i)
                };
                let gy = if ey.is_valid(i) {
                    ey.value(i)
                } else {
                    y.value(i)
                };
                let point = lonlat(crate::osm::geometry::grid_to_meters(gx, gy));
                buildings.push((
                    ids.value(i),
                    gx,
                    gy,
                    point,
                    model.building(
                        types.value(i),
                        storeys.value(i),
                        areas.is_valid(i).then(|| f64::from(areas.value(i))),
                    ),
                ));
            }
        }
    }
    buildings.sort_by_key(|b| (b.0, b.1, b.2));
    let mut loads = vec![[0.0; 2]; roads.len()];
    if buildings.is_empty() || eligible.is_empty() {
        return Ok((loads, !buildings.is_empty()));
    }
    let origin = buildings[0].3[0];
    let longitude_scale = 111_320.0
        * (buildings.iter().map(|b| b.3[1]).sum::<f64>() / buildings.len() as f64
            * std::f64::consts::PI
            / 180.0)
            .cos();
    let project = |point: [f64; 2]| {
        [
            if (point[0] - origin).abs() > 180.0 {
                origin + (point[0] - origin + 180.0).rem_euclid(360.0) - 180.0
            } else {
                point[0]
            } * longitude_scale,
            point[1] * 110_540.0,
        ]
    };
    let mut cells: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    let segments = eligible
        .iter()
        .map(|&i| {
            let points = roads[i]
                .ends
                .map(|(x, y)| project(lonlat(crate::osm::geometry::grid_to_meters(x, y))));
            (i, points[0], points[1])
        })
        .collect::<Vec<_>>();
    for (index, (_, a, b)) in segments.iter().enumerate() {
        for y in ((a[1].min(b[1]) - 50.0) / 50.0).floor() as i64
            ..=((a[1].max(b[1]) + 50.0) / 50.0).floor() as i64
        {
            for x in ((a[0].min(b[0]) - 50.0) / 50.0).floor() as i64
                ..=((a[0].max(b[0]) + 50.0) / 50.0).floor() as i64
            {
                cells.entry((x, y)).or_default().push(index);
            }
        }
    }
    for (_, _, _, point, load) in buildings {
        let p = project(point);
        let mut best = None;
        let mut distance = f64::INFINITY;
        for &candidate in cells
            .get(&((p[0] / 50.0).floor() as i64, (p[1] / 50.0).floor() as i64))
            .into_iter()
            .flatten()
        {
            let (index, a, b) = segments[candidate];
            let dx = b[0] - a[0];
            let dy = b[1] - a[1];
            let squared = dx * dx + dy * dy;
            let t = if squared < 1e-6 {
                0.0
            } else {
                (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / squared).clamp(0.0, 1.0)
            };
            let ex = p[0] - (a[0] + t * dx);
            let ey = p[1] - (a[1] + t * dy);
            let next = (ex * ex + ey * ey).sqrt();
            if next <= 50.0 && next < distance {
                distance = next;
                best = Some(index);
            }
        }
        if let Some(index) = best {
            loads[index][0] += load[0];
            loads[index][1] += load[1];
        }
    }
    Ok((loads, true))
}

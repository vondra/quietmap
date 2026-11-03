//! Road settlement density from screening footprints, with unknown coverage retained at missing halo cells.

use super::input::Road;
use crate::dev4::{Square, column, read_table, require_stamp};
use anyhow::{Result, anyhow, ensure};
use arrow_array::{Array, BinaryArray, Int32Array, Int64Array, UInt8Array};
use rstar::{RTree, primitives::GeomWithData};
use std::{collections::BTreeSet, path::Path};

// Original vector-density calibration of the 17 by 17 one-arcsecond window.
const HALF_WINDOW: f64 = 8.5 / 3600.0;
const BUILT_PIXELS: f64 = 8.0;
const METRES_LATITUDE: f64 = 111_132.0;
const METRES_LONGITUDE: f64 = 111_320.0;

fn lonlat(grid: (i32, i32)) -> [f64; 2] {
    // The structures producer derives its quantum; the OSM extractor rounds that constant.
    let circumference = 2.0 * std::f64::consts::PI * 6_378_137.0;
    let quantum = circumference / (1u64 << 30) as f64;
    let x = f64::from(grid.0 - (1 << 29)) * quantum;
    let y = f64::from(grid.1 - (1 << 29)) * quantum;
    [
        x / 6_378_137.0 * 180.0 / std::f64::consts::PI,
        (2.0 * (y / 6_378_137.0).exp().atan() - std::f64::consts::FRAC_PI_2) * 180.0
            / std::f64::consts::PI,
    ]
}

fn footprint_area(bytes: &[u8]) -> Result<f64> {
    let parts = crate::structures::decode_parts(bytes)
        .ok_or_else(|| anyhow!("invalid density footprint"))?;
    let mut area = 0.0;
    for rings in parts {
        for (index, ring) in rings.into_iter().enumerate() {
            let coordinates = ring.into_iter().map(lonlat).collect::<Vec<_>>();
            let origin = coordinates[0];
            let metres_lon = METRES_LONGITUDE * origin[1].to_radians().cos();
            let projected = coordinates
                .iter()
                .map(|p| {
                    [
                        ((p[0] - origin[0] + 180.0).rem_euclid(360.0) - 180.0) * metres_lon,
                        (p[1] - origin[1]) * METRES_LATITUDE,
                    ]
                })
                .collect::<Vec<_>>();
            let mut left = 0.0;
            let mut right = 0.0;
            for (a, b) in projected.iter().zip(projected.iter().cycle().skip(1)) {
                left += a[0] * b[1];
                right += a[1] * b[0];
            }
            let ring_area = (left - right).abs() / 2.0;
            area += if index == 0 { ring_area } else { -ring_area };
        }
    }
    Ok(area.max(0.0))
}

pub fn classify(root: &Path, roads: &mut [Road]) -> Result<()> {
    if roads.is_empty() {
        return Ok(());
    }
    let points = roads
        .iter()
        .map(|road| {
            let a = lonlat(road.ends[0]);
            let b = lonlat(road.ends[1]);
            [
                (a[0] + ((b[0] - a[0] + 180.0).rem_euclid(360.0) - 180.0) / 2.0 + 180.0)
                    .rem_euclid(360.0)
                    - 180.0,
                (a[1] + b[1]) / 2.0,
            ]
        })
        .collect::<Vec<_>>();
    let corners = points
        .iter()
        .map(|p| {
            [-HALF_WINDOW, HALF_WINDOW]
                .into_iter()
                .flat_map(|x| {
                    [-HALF_WINDOW, HALF_WINDOW].map(|y| {
                        let square = crate::osm::geometry::square_of(p[1] + y, p[0] + x);
                        Square {
                            x: u32::from(square.x),
                            y: u32::from(square.y),
                        }
                    })
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let mut missing = BTreeSet::new();
    let mut footprints = Vec::new();
    for square in corners.iter().flatten().copied().collect::<BTreeSet<_>>() {
        let path = root
            .join("z9")
            .join(square.x.to_string())
            .join(square.y.to_string())
            .join("structures.arrow");
        let Some(table) = read_table(&path).map_err(anyhow::Error::msg)? else {
            missing.insert(square);
            continue;
        };
        require_stamp(&table, "structures_contract", "structures_v5")
            .map_err(anyhow::Error::msg)?;
        require_stamp(&table, "grid", "z30").map_err(anyhow::Error::msg)?;
        for batch in table.batches {
            macro_rules! column {
                ($ty:ident, $name:literal) => {
                    column::<$ty>(&batch, $name).map_err(anyhow::Error::msg)?
                };
            }
            let kinds = column!(UInt8Array, "kind");
            let ids = column!(Int64Array, "osm_id");
            let gx = column!(Int32Array, "centroid_gx");
            let gy = column!(Int32Array, "centroid_gy");
            let emission_x = column!(Int32Array, "emission_centroid_gx");
            let emission_y = column!(Int32Array, "emission_centroid_gy");
            let geometry = column!(BinaryArray, "geom");
            for i in 0..batch.num_rows() {
                if kinds.value(i) != 0 {
                    continue;
                }
                ensure!(
                    emission_x.is_valid(i) == emission_y.is_valid(i),
                    "partial emission centroid"
                );
                if ids.is_valid(i) && emission_x.is_null(i) {
                    continue;
                }
                ensure!(geometry.is_valid(i), "missing density geometry");
                let p = lonlat((gx.value(i), gy.value(i)));
                let area = footprint_area(geometry.value(i))?;
                footprints.push(GeomWithData::new(p, area));
                if p[0] < -180.0 + 2.0 * HALF_WINDOW {
                    footprints.push(GeomWithData::new([p[0] + 360.0, p[1]], area));
                }
                if p[0] > 180.0 - 2.0 * HALF_WINDOW {
                    footprints.push(GeomWithData::new([p[0] - 360.0, p[1]], area));
                }
            }
        }
    }
    let index = RTree::bulk_load(footprints);
    for ((road, point), corners) in roads.iter_mut().zip(points).zip(corners) {
        road.built_up = if corners.iter().any(|s| missing.contains(s)) {
            0
        } else {
            let window = rstar::AABB::from_corners(
                [point[0] - HALF_WINDOW, point[1] - HALF_WINDOW],
                [point[0] + HALF_WINDOW, point[1] + HALF_WINDOW],
            );
            let area: f64 = index
                .locate_in_envelope_intersecting(&window)
                .map(|p| p.data)
                .sum();
            let pixel_area =
                METRES_LATITUDE * METRES_LONGITUDE * point[1].to_radians().cos() / 3600f64.powi(2);
            if area / pixel_area >= BUILT_PIXELS {
                2
            } else {
                1
            }
        };
    }
    Ok(())
}

//! RÚIAN and Catastro publisher attributes become common dated building observations.

use super::{Objects, Observation, Square, selected};
use crate::structures::observations::ATTRIBUTE;
use anyhow::{Result, ensure};
use serde::Deserialize;
use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};

#[derive(Deserialize)]
struct Point {
    lat: f64,
    lon: f64,
    floors: u32,
    #[serde(rename = "useCode")]
    use_code: Option<u32>,
}

fn ruian_class(code: u32) -> Option<u8> {
    match code {
        2 | 13 | 21 => Some(8),
        6 | 7 | 8 | 19 => Some(0),
        9 | 15 => Some(9),
        10 | 14 | 20 => Some(1),
        11 => Some(6),
        12 | 16 => Some(2),
        17 | 18 => Some(7),
        _ => None,
    }
}

pub fn convert(
    source: &Objects,
    root: &Path,
    squares: &[Square],
    rows: &mut [Vec<Observation>],
) -> Result<()> {
    let coverage = if source.format == "ruian" {
        [11.7, 48.2, 19.2, 51.4]
    } else {
        [-19.0, 27.0, 5.0, 44.0]
    };
    if !squares.iter().any(|&square| {
        super::touched_degrees(square, 31.0)
            .iter()
            .any(|&[lat, lon]| {
                f64::from(lon) <= coverage[2]
                    && f64::from(lon + 1) >= coverage[0]
                    && f64::from(lat) <= coverage[3]
                    && f64::from(lat + 1) >= coverage[1]
            })
    }) {
        return Ok(());
    }
    let mut add = |point: Point| -> Result<()> {
        ensure!(
            point.lat.is_finite()
                && point.lon.is_finite()
                && point.lat.abs() <= 90.0
                && point.lon.abs() <= 180.0,
            "invalid national building coordinate"
        );
        ensure!(
            (coverage[1]..=coverage[3]).contains(&point.lat)
                && (coverage[0]..=coverage[2]).contains(&point.lon),
            "national building outside publisher coverage"
        );
        let class = if source.format == "ruian" {
            ruian_class(
                point
                    .use_code
                    .ok_or_else(|| anyhow::anyhow!("RÚIAN useCode missing"))?,
            )
        } else {
            ensure!(
                (1..=255).contains(&point.floors),
                "invalid Catastro point or floors"
            );
            None
        };
        for (square, rows) in squares.iter().zip(rows.iter_mut()) {
            if !selected(point.lon, point.lat, *square, 31.0).0 {
                continue;
            }
            rows.push(Observation {
                kind: ATTRIBUTE,
                geometry: None,
                longitude: point.lon,
                latitude: point.lat,
                height_m: None,
                floors: point.floors.min(255) as u8,
                building_type: class,
                envelope: 5,
                open_roof: false,
                owned: true,
            });
        }
        Ok(())
    };
    for object in &source.objects {
        let input = BufReader::new(File::open(root.join(&object.path))?);
        if source.format == "ruian" {
            let points: Vec<Point> = serde_json::from_reader(input)?;
            ensure!(!points.is_empty(), "empty RÚIAN inventory");
            for point in points {
                add(point)?;
            }
        } else {
            let mut count = 0;
            for line in input.lines() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                add(serde_json::from_str(&line)?)?;
                count += 1;
            }
            ensure!(count > 0, "empty Catastro inventory");
        }
    }
    Ok(())
}

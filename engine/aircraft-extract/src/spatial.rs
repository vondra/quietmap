//! Canonical z9 partitions and exact transit fractions through z15 cruise cells.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use grid::cruise::{cell_axes, cruise_cell_id, CRUISE_AXIS};
use grid::{poly::meters_to_lonlat, Square, EARTH_CIRCUMFERENCE_M};

pub fn square_id(lat: f64, lon: f64) -> Option<u64> {
    (lat.is_finite() && lon.is_finite() && (-90.0..=90.0).contains(&lat))
        .then(|| grid::square_id(grid::square_of(lat, lon)) as u64)
}

pub fn square_path(id: u64) -> String {
    let square = i64::try_from(id)
        .ok()
        .and_then(grid::square_from_id)
        .expect("aircraft partition id must be a valid z9 square");
    grid::square_name(square)
}

pub fn square_bounds(id: u64) -> (f64, f64, f64, f64) {
    let square = grid::square_from_id(id as i64).expect("valid z9 square");
    let west = f64::from(square.x) * grid::Z9_SPAN_DEG - 180.0;
    let east = west + grid::Z9_SPAN_DEG;
    (
        tile_latitude(u32::from(square.y) + 1, 512),
        west,
        tile_latitude(u32::from(square.y), 512),
        east,
    )
}

/// Only canonical, real directories are visited; symlinks cannot redirect
/// a subsequent scoped write or stale-file removal outside the prepared tree.
pub fn square_directories(root: &Path) -> Result<Vec<(u64, PathBuf)>> {
    let z9 = root.join("z9");
    match std::fs::symlink_metadata(&z9) {
        Ok(metadata) => anyhow::ensure!(
            metadata.is_dir(),
            "{} must be a real directory",
            z9.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("stat {}", z9.display())),
    }
    let columns = match std::fs::read_dir(&z9) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("read {}", z9.display())),
    };
    let mut out = Vec::new();
    for column in columns {
        let column = column?;
        let Some(x) = directory_axis(&column)? else {
            continue;
        };
        for row in std::fs::read_dir(column.path())? {
            let row = row?;
            let Some(y) = directory_axis(&row)? else {
                continue;
            };
            out.push((grid::square_id(Square { x, y }) as u64, row.path()));
        }
    }
    out.sort_unstable_by_key(|(id, _)| *id);
    Ok(out)
}

fn directory_axis(entry: &std::fs::DirEntry) -> Result<Option<u16>> {
    if !entry.file_type()?.is_dir() {
        return Ok(None);
    }
    let name = entry.file_name();
    let Some(name) = name.to_str() else {
        return Ok(None);
    };
    Ok(name
        .parse::<u16>()
        .ok()
        .filter(|value| *value < 512 && value.to_string() == name))
}

fn tile_latitude(y: u32, axis: u32) -> f64 {
    let northing = (0.5 - f64::from(y) / f64::from(axis)) * EARTH_CIRCUMFERENCE_M;
    meters_to_lonlat(0.0, northing).1
}

/// Each interval is bounded by the actual longitude/latitude tile edges.
/// Fractions telescope to one, including dateline crossings and tiny corner
/// intersections; sampling at fixed distances cannot provide this guarantee.
pub fn cruise_transits(lat0: f32, lon0: f32, lat1: f32, lon1: f32) -> Vec<(u64, f32)> {
    let (lat0, lat1, lon0) = (f64::from(lat0), f64::from(lat1), f64::from(lon0));
    let dlat = lat1 - lat0;
    let dlon = grid::geo::wrapped_longitude_delta(lon0, f64::from(lon1));
    let total = crate::geo::flat_dist(lat0 as f32, lon0 as f32, lat1 as f32, lon1);
    if !total.is_finite() || total <= 0.0 {
        return Vec::new();
    }
    let mut cuts = vec![0.0, 1.0];
    let span = 360.0 / f64::from(CRUISE_AXIS);
    if dlon != 0.0 {
        let start = ((lon0.min(lon0 + dlon) + 180.0) / span).floor() as i64 + 1;
        let end = ((lon0.max(lon0 + dlon) + 180.0) / span).ceil() as i64;
        for column in start..end {
            cuts.push((column as f64 * span - 180.0 - lon0) / dlon);
        }
    }
    if dlat != 0.0 {
        let (_, y0) = cell_axes(lat0, lon0);
        let (_, y1) = cell_axes(lat1, lon0 + dlon);
        for row in y0.min(y1) + 1..=y0.max(y1) {
            cuts.push((tile_latitude(row, CRUISE_AXIS) - lat0) / dlat);
        }
    }
    cuts.retain(|fraction| (0.0..=1.0).contains(fraction));
    cuts.sort_unstable_by(f64::total_cmp);
    cuts.dedup();
    cuts.windows(2)
        .filter_map(|interval| {
            let fraction = interval[1] - interval[0];
            if fraction <= 0.0 {
                return None;
            }
            let middle = (interval[0] + interval[1]) * 0.5;
            Some((
                cruise_cell_id(lat0 + middle * dlat, lon0 + middle * dlon),
                (f64::from(total) * fraction) as f32,
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use grid::cruise::{cruise_centroid, cruise_parent};

    #[test]
    fn transit_conserves_distance_and_routes_centroids_worldwide() {
        for (lat0, lon0, lat1, lon1) in [
            (50.0, 14.0, 50.2, 14.3),
            (0.0, 179.97, 0.03, -179.97),
            (-80.0, -179.98, -80.01, 179.98),
            (84.99, 40.0, 85.05, 40.1),
            (-90.0, 0.0, -89.0, 0.1),
            (89.0, 0.0, 90.0, 0.1),
            (-40.0, 10.0, -40.0, 10.5),
        ] {
            let cells = cruise_transits(lat0, lon0, lat1, lon1);
            let total = crate::geo::flat_dist(lat0, lon0, lat1, lon1);
            let sum: f32 = cells.iter().map(|(_, length)| length).sum();
            assert!((sum - total).abs() < total * 1e-6, "{sum} vs {total}");
            for (id, length) in cells {
                assert!(length > 0.0);
                let (lon, lat) = cruise_centroid(id);
                assert_eq!(cruise_parent(id), square_id(lat, lon).unwrap());
            }
        }
    }

    #[test]
    fn directory_walk_rejects_noncanonical_names_and_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["z9/276/173", "z9/0276/174", "z9/512/0", "z9/1/051"] {
            std::fs::create_dir_all(dir.path().join(name)).unwrap();
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path().join("z9/276"), dir.path().join("z9/277")).unwrap();
        let found = square_directories(dir.path()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(square_path(found[0].0), "z9/276/173");
    }
}

#[cfg(test)]
mod cruise_precision_tests {
    #[test]
    fn finer_cruise_centroids_reduce_npd_displacement_error() {
        use noise_compute::emission::aircraft::{NpdLuts, FT_PER_M, NUM_CLASSES};
        let luts = NpdLuts::shared();
        let radius = grid::EARTH_CIRCUMFERENCE_M / f64::from(super::CRUISE_AXIS) / 2.0_f64.sqrt();
        // The corresponding dev1 schema documented its maximum centroid offset as 1.44 km.
        let max_error = |offset: f64| {
            let mut error: f64 = 0.0;
            for class in 0..NUM_CLASSES {
                for altitude in [7200.0_f64, 11000.0, 15000.0] {
                    for distance in (0..=15000).step_by(1000) {
                        let distance = f64::from(distance);
                        let baseline = luts.lookup_lmax(
                            class,
                            true,
                            (distance.hypot(altitude) * FT_PER_M).log10(),
                        );
                        for angle in 0..16 {
                            let theta = f64::from(angle) * std::f64::consts::TAU / 16.0;
                            let shifted = (distance + offset * theta.cos())
                                .hypot(offset * theta.sin())
                                .hypot(altitude);
                            error = error.max(
                                (luts.lookup_lmax(class, true, (shifted * FT_PER_M).log10())
                                    - baseline)
                                    .abs(),
                            );
                        }
                    }
                }
            }
            error
        };
        let current = max_error(radius);
        let prior_bound = max_error(1440.0);
        eprintln!("cruise centroid radius={radius:.3}m, sampled max NPD Lmax shift={current:.6}dB versus prior radius bound={prior_bound:.6}dB");
        assert!(
            current < prior_bound,
            "finer centroid cannot worsen displacement in the same NPD samples"
        );
    }
}

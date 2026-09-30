//! Altitude of every sample above EGM2008, the terrain's datum. Measured on three days (160,021
//! lift-offs and touchdowns on runways, 7.2 M samples of 5 % of the aircraft): readsb's geometric
//! altitude is ellipsoidal for 94 % of aircraft (minus the geoid it lands p5 -4.6, median +3.9,
//! p95 +17.8 m on the runway, the aircraft being a few metres up), the rest report mean sea level.
//! The rule per sample: barometric altitude plus an offset, the offset being (1) the sample's own
//! geometric minus geoid minus barometric where plausible (within 300 m + 6 % of the pressure
//! altitude, and within 100 m of the running median of +-5 geometric samples); (2) between a flight's plausible offsets, linear in time (a 60 s gap
//! errs p5 -16, p95 +15 m below 1,000 m); (3) for a flight without geometric altitude, the median
//! offset of at least two other aircraft in the same 1 degree cell, UTC hour and 500 m pressure band
//! (p5 -23, p95 +30 m below 1,000 m); (4) else none (barometric alone errs p5 -96, p95 +145 m
//! below 10,000 ft). A surface report sits on the terrain.

use super::dem::{LastSquare, TerrainHeights};
use super::filters::point_is_sane;
use super::geoid::Geoid;
use super::trace::TracePoint;
use rayon::prelude::*;
use std::collections::HashMap;

pub const M_PER_FT: f32 = 0.3048;
/// A geometric sample further than this from the running median offset of its neighbours is a
/// bad velocity-message delta (0.05 % of samples).
const PLAUSIBLE_OFFSET_DEVIATION_M: f32 = 100.0;
/// True minus pressure altitude beyond 300 m + 6 % of the pressure altitude is no weather: the
/// measured p0.1-p99.9 are -200..+158 m below 500 m, -316..+302 m at 2-4 km, -597..+696 m at
/// 7-10 km; beyond lie clusters at +-760 and -960 m (a saturated GNSS-minus-baro field) and zeros.
const OFFSET_BOUND_AT_ZERO_M: f32 = 300.0;
const OFFSET_BOUND_PER_PRESSURE_M: f32 = 0.06;
const OFFSET_MEDIAN_HALF_WINDOW: usize = 5;
/// A surface report and an airborne sample this close are a lift-off or a touchdown.
const TRANSITION_MAX_GAP_S: f64 = 3.0;
/// The geometric sample of a transition may lie this far into the air, carried by the barometric
/// difference.
const TRANSITION_GEOMETRIC_REACH_S: f64 = 30.0;
/// Where the geoid is flatter than this, the two datums differ too little to tell (or to matter).
const DATUM_DECISIVE_UNDULATION_M: f64 = 10.0;
/// Mean sea level needs two transitions: one can be a stale reading. Held out on 100,649
/// transitions of three days, deciding from the aircraft's other transitions cuts the share of
/// runway residuals beyond 20 m from 4.8 % (always ellipsoid) to 3.7 % (p95 19.1 to 16.4 m).
const DATUM_MIN_TRANSITIONS: usize = 2;
const REGIONAL_BAND_M: f32 = 500.0;
const REGIONAL_MIN_AIRCRAFT: usize = 2;

/// A trace point with the terrain under it and its altitude above EGM2008.
#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub point: TracePoint,
    pub terrain_m: f32,
    pub altitude_m: f32,
    /// `altitude_m - terrain_m`; 0 on a surface report.
    pub height_m: f32,
}

/// How an aircraft's geometric altitude is referenced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeometricDatum {
    Ellipsoid,
    MeanSeaLevel,
}

/// What a flight's altitudes rest on (the flight table's `altitude_source`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AltitudeSource {
    Geometric = 0,
    Regional = 1,
    Barometric = 2,
}

/// Terrain height under a trace point.
pub fn terrain_m(
    terrain: &TerrainHeights,
    point: &TracePoint,
    last: &mut LastSquare,
) -> Result<f32, String> {
    terrain.height_m(f64::from(point.lat), f64::from(point.lon), last)
}

/// True minus pressure altitude of an airborne sample with a geometric altitude, metres.
fn own_offset_m(point: &TracePoint, datum: GeometricDatum, geoid: &Geoid) -> Option<f32> {
    let barometric = point.airborne_altitude_ft()? * M_PER_FT;
    let geometric = point.geometric_altitude_ft * M_PER_FT;
    if !geometric.is_finite() || !barometric.is_finite() {
        return None;
    }
    let undulation = match datum {
        GeometricDatum::Ellipsoid => {
            geoid.undulation_m(f64::from(point.lat), f64::from(point.lon)) as f32
        }
        GeometricDatum::MeanSeaLevel => 0.0,
    };
    let offset = geometric - undulation - barometric;
    let bound = OFFSET_BOUND_AT_ZERO_M + OFFSET_BOUND_PER_PRESSURE_M * barometric.max(0.0);
    (offset.abs() <= bound).then_some(offset)
}

/// The aircraft's datum from its lift-offs and touchdowns: mean sea level where that explains the
/// geometric altitude on the ground better than the ellipsoid does, else the ellipsoid.
pub fn geometric_datum(
    points: &[TracePoint],
    geoid: &Geoid,
    mut terrain_at: impl FnMut(&TracePoint) -> Result<f32, String>,
) -> Result<GeometricDatum, String> {
    let (mut ellipsoid, mut sea_level, mut undulations) = (Vec::new(), Vec::new(), Vec::new());
    for index in 1..points.len() {
        let (a, b) = (&points[index - 1], &points[index]);
        if b.timestamp - a.timestamp > TRANSITION_MAX_GAP_S
            || a.is_surface_report() == b.is_surface_report()
        {
            continue;
        }
        let (edge, step): (usize, isize) = if a.is_surface_report() {
            (index, 1)
        } else {
            (index - 1, -1)
        };
        let edge_point = &points[edge];
        let mut cursor = edge as isize;
        let geometric = loop {
            let Some(point) = points
                .get(cursor as usize)
                .filter(|p| !p.is_surface_report())
            else {
                break None;
            };
            if (point.timestamp - edge_point.timestamp).abs() > TRANSITION_GEOMETRIC_REACH_S {
                break None;
            }
            if point.geometric_altitude_ft.is_finite() {
                break Some(
                    point.geometric_altitude_ft - (point.altitude_ft - edge_point.altitude_ft),
                );
            }
            cursor += step;
        };
        let Some(geometric_ft) = geometric.filter(|g| g.is_finite()) else {
            continue;
        };
        let terrain = f64::from(terrain_at(edge_point)?);
        let undulation = geoid.undulation_m(f64::from(edge_point.lat), f64::from(edge_point.lon));
        let geometric_m = f64::from(geometric_ft * M_PER_FT);
        ellipsoid.push((geometric_m - undulation - terrain).abs());
        sea_level.push((geometric_m - terrain).abs());
        undulations.push(undulation.abs());
    }
    if undulations.len() < DATUM_MIN_TRANSITIONS
        || median(&mut undulations) < DATUM_DECISIVE_UNDULATION_M
    {
        return Ok(GeometricDatum::Ellipsoid);
    }
    Ok(if median(&mut sea_level) < median(&mut ellipsoid) {
        GeometricDatum::MeanSeaLevel
    } else {
        GeometricDatum::Ellipsoid
    })
}

fn median<T: Copy + PartialOrd>(values: &mut [T]) -> T {
    let middle = values.len() / 2;
    *values
        .select_nth_unstable_by(middle, |a, b| {
            a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
        })
        .1
}

/// The median true-minus-pressure offset of the day's aircraft per 1 degree cell, UTC hour and
/// 500 m pressure band, each aircraft counted once per cell.
pub struct RegionalOffsets {
    offset_m: HashMap<u64, f32>,
}

fn regional_key(point: &TracePoint, barometric_m: f32) -> u64 {
    let hour = (point.timestamp / 3600.0).floor() as u64;
    let lat = (f64::from(point.lat).floor() + 90.0).clamp(0.0, 180.0) as u64;
    let lon = (f64::from(point.lon).floor() + 180.0).clamp(0.0, 360.0) as u64;
    let band = ((barometric_m / REGIONAL_BAND_M).floor() + 8.0).clamp(0.0, 255.0) as u64;
    (hour << 32) | (lat << 20) | (lon << 8) | band
}

impl RegionalOffsets {
    /// From every (trace points, datum) of the day.
    pub fn build(traces: &[(&[TracePoint], GeometricDatum)], geoid: &Geoid) -> Self {
        let mut per_aircraft: Vec<(u64, f32)> = traces
            .par_iter()
            .flat_map_iter(|&(points, datum)| {
                let mut offsets: Vec<(u64, f32)> = points
                    .iter()
                    .filter(|point| point_is_sane(point))
                    .filter_map(|point| {
                        let offset = own_offset_m(point, datum, geoid)?;
                        Some((regional_key(point, point.altitude_ft * M_PER_FT), offset))
                    })
                    .collect();
                offsets.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
                offsets
                    .chunk_by(|a, b| a.0 == b.0)
                    .map(|cell| cell[cell.len() / 2])
                    .collect::<Vec<_>>()
            })
            .collect();
        per_aircraft.par_sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
        let offset_m = per_aircraft
            .chunk_by(|a, b| a.0 == b.0)
            .filter(|cell| cell.len() >= REGIONAL_MIN_AIRCRAFT)
            .map(|cell| (cell[0].0, cell[cell.len() / 2].1))
            .collect();
        RegionalOffsets { offset_m }
    }

    fn offset_m(&self, point: &TracePoint, barometric_m: f32) -> Option<f32> {
        self.offset_m
            .get(&regional_key(point, barometric_m))
            .copied()
    }

    pub fn cells(&self) -> usize {
        self.offset_m.len()
    }
}

/// The samples of one flight and what their altitudes rest on. `terrain_m` is parallel to `points`.
pub fn samples(
    points: &[TracePoint],
    terrain_m: &[f32],
    datum: GeometricDatum,
    geoid: &Geoid,
    regional: &RegionalOffsets,
) -> (Vec<Sample>, AltitudeSource) {
    let own: Vec<Option<f32>> = points
        .iter()
        .map(|p| own_offset_m(p, datum, geoid))
        .collect();
    let with_geometric: Vec<usize> = (0..points.len()).filter(|&i| own[i].is_some()).collect();
    let mut plausible: Vec<(f64, f32)> = Vec::with_capacity(with_geometric.len());
    let mut window = Vec::with_capacity(2 * OFFSET_MEDIAN_HALF_WINDOW + 1);
    for (k, &index) in with_geometric.iter().enumerate() {
        let low = k.saturating_sub(OFFSET_MEDIAN_HALF_WINDOW);
        let high = (k + OFFSET_MEDIAN_HALF_WINDOW + 1).min(with_geometric.len());
        window.clear();
        window.extend(with_geometric[low..high].iter().filter_map(|&i| own[i]));
        let offset = own[index].unwrap_or(0.0);
        if (offset - median(&mut window)).abs() <= PLAUSIBLE_OFFSET_DEVIATION_M {
            plausible.push((points[index].timestamp, offset));
        }
    }
    let (mut regional_hits, mut airborne) = (0usize, 0usize);
    let samples = points
        .iter()
        .zip(terrain_m)
        .map(|(point, &terrain)| {
            let Some(barometric_ft) = point.airborne_altitude_ft() else {
                return Sample {
                    point: *point,
                    terrain_m: terrain,
                    altitude_m: terrain,
                    height_m: 0.0,
                };
            };
            airborne += 1;
            let barometric = barometric_ft * M_PER_FT;
            let offset = if plausible.is_empty() {
                let regional = regional.offset_m(point, barometric);
                regional_hits += usize::from(regional.is_some());
                regional.unwrap_or(0.0)
            } else {
                interpolated_offset(&plausible, point.timestamp)
            };
            let altitude = barometric + offset;
            Sample {
                point: *point,
                terrain_m: terrain,
                altitude_m: altitude,
                height_m: altitude - terrain,
            }
        })
        .collect();
    let source = if !plausible.is_empty() {
        AltitudeSource::Geometric
    } else if airborne > 0 && 2 * regional_hits >= airborne {
        AltitudeSource::Regional
    } else {
        AltitudeSource::Barometric
    };
    (samples, source)
}

/// The offset at `timestamp`: exact at a plausible sample, linear in time between two, the nearest
/// beyond the ends. `plausible` is in time order and not empty.
fn interpolated_offset(plausible: &[(f64, f32)], timestamp: f64) -> f32 {
    let after = plausible.partition_point(|&(t, _)| t < timestamp);
    match (
        after.checked_sub(1).map(|i| plausible[i]),
        plausible.get(after),
    ) {
        (_, Some(&(t, offset))) if t == timestamp => offset,
        (Some((t0, a)), Some(&(t1, b))) => a + (b - a) * ((timestamp - t0) / (t1 - t0)) as f32,
        (Some((_, a)), None) => a,
        (None, Some(&(_, b))) => b,
        (None, None) => 0.0,
    }
}

#[cfg(test)]
#[path = "altitude_tests.rs"]
pub(crate) mod tests;

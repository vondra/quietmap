//! Altitude rule cases: own geometric altitude, gaps, spikes, the regional offset, the datum.

use super::*;
use crate::aircraft::trace::SURFACE_REPORT;

/// An airborne sample over 0 m terrain at `altitude_ft` (barometric), no geometric altitude.
pub fn sample(timestamp: f64, lat: f32, lon: f32, altitude_ft: f32, speed_kt: f32) -> Sample {
    Sample {
        point: TracePoint {
            timestamp,
            lat,
            lon,
            altitude_ft,
            geometric_altitude_ft: f32::NAN,
            ground_speed_kt: speed_kt,
            track_deg: 90.0,
            vertical_rate_fpm: 0.0,
            flags: 0,
        },
        terrain_m: 0.0,
        altitude_m: altitude_ft * M_PER_FT,
        height_m: altitude_ft * M_PER_FT,
    }
}

fn point(timestamp: f64, barometric_ft: f32, geometric_ft: f32) -> TracePoint {
    let mut point = sample(timestamp, 50.0, 14.0, barometric_ft, 150.0).point;
    point.geometric_altitude_ft = geometric_ft;
    point
}

fn surface(timestamp: f64) -> TracePoint {
    let mut point = point(timestamp, f32::NAN, f32::NAN);
    point.flags = SURFACE_REPORT;
    point
}

const UNDULATION_M: f64 = 45.0;

fn corrected(points: &[TracePoint], regional: &RegionalOffsets) -> (Vec<f32>, AltitudeSource) {
    let terrain = vec![300.0; points.len()];
    let geoid = Geoid::uniform(UNDULATION_M);
    let (samples, source) = samples(
        points,
        &terrain,
        GeometricDatum::Ellipsoid,
        &geoid,
        regional,
    );
    (samples.iter().map(|s| s.altitude_m).collect(), source)
}

fn no_regional() -> RegionalOffsets {
    RegionalOffsets {
        offset_m: HashMap::new(),
    }
}

#[test]
fn a_plausible_geometric_altitude_minus_the_geoid_is_the_altitude() {
    let points: Vec<_> = (0..20)
        .map(|i| point(f64::from(i) * 5.0, 3000.0, 3400.0))
        .collect();
    let (altitudes, source) = corrected(&points, &no_regional());
    assert_eq!(source, AltitudeSource::Geometric);
    let expected = 3400.0 * M_PER_FT - UNDULATION_M as f32;
    assert!(
        altitudes.iter().all(|a| (a - expected).abs() < 1e-3),
        "{altitudes:?}"
    );
}

/// Between geometric samples the true-minus-pressure offset is linear in time; beyond the ends
/// the nearest holds; a surface report sits on the terrain.
#[test]
fn gaps_take_the_offset_interpolated_in_time() {
    let mut points = vec![point(0.0, 3000.0, 3300.0), point(10.0, 3000.0, f32::NAN)];
    points.push(point(20.0, 3000.0, 3500.0));
    points.push(point(30.0, 3100.0, f32::NAN));
    points.push(surface(31.0));
    let (altitudes, _) = corrected(&points, &no_regional());
    let offset =
        |geometric_ft: f32| geometric_ft * M_PER_FT - UNDULATION_M as f32 - 3000.0 * M_PER_FT;
    let middle = 3000.0 * M_PER_FT + (offset(3300.0) + offset(3500.0)) / 2.0;
    assert!((altitudes[1] - middle).abs() < 1e-3);
    assert!((altitudes[3] - (3100.0 * M_PER_FT + offset(3500.0))).abs() < 1e-3);
    assert_eq!(altitudes[4], 300.0);
}

/// One geometric sample 150 m off its neighbours' offsets is a bad delta, and so is a stretch of
/// zeros far beyond any weather: both take the plausible offsets around them.
#[test]
fn geometric_spikes_and_zeros_are_not_plausible() {
    let mut points: Vec<_> = (0..24)
        .map(|i| point(f64::from(i) * 5.0, 3000.0, 3400.0))
        .collect();
    points[6].geometric_altitude_ft = 3400.0 + 150.0 / M_PER_FT;
    for point in &mut points[12..20] {
        point.geometric_altitude_ft = 0.0;
    }
    let (altitudes, _) = corrected(&points, &no_regional());
    assert!((altitudes[6] - altitudes[5]).abs() < 1e-3, "{altitudes:?}");
    assert!(
        altitudes[12..20]
            .iter()
            .all(|a| (a - altitudes[5]).abs() < 1e-3)
    );
}

/// A flight without geometric altitude takes the median offset of other aircraft in its cell,
/// hour and pressure band; with fewer than two aircraft there, its barometric altitude.
#[test]
fn a_flight_without_geometric_altitude_takes_the_regional_offset() {
    let geoid = Geoid::uniform(UNDULATION_M);
    let others: Vec<Vec<TracePoint>> = [40.0, 60.0, 90.0]
        .iter()
        .map(|offset_m| {
            (0..5)
                .map(|i| {
                    point(
                        f64::from(i),
                        3000.0,
                        3000.0 + (offset_m + UNDULATION_M as f32) / M_PER_FT,
                    )
                })
                .collect()
        })
        .collect();
    let traces: Vec<(&[TracePoint], GeometricDatum)> = others
        .iter()
        .map(|t| (t.as_slice(), GeometricDatum::Ellipsoid))
        .collect();
    let regional = RegionalOffsets::build(&traces, &geoid);
    let bare = [point(2.0, 3000.0, f32::NAN), point(3.0, 3010.0, f32::NAN)];
    let (altitudes, source) = corrected(&bare, &regional);
    assert_eq!(source, AltitudeSource::Regional);
    assert!(
        (altitudes[0] - (3000.0 * M_PER_FT + 60.0)).abs() < 0.01,
        "{altitudes:?}"
    );
    let alone = RegionalOffsets::build(&traces[..1], &geoid);
    let (altitudes, source) = corrected(&bare, &alone);
    assert_eq!(source, AltitudeSource::Barometric);
    assert_eq!(altitudes[0], 3000.0 * M_PER_FT);
}

/// An aircraft whose geometric altitude on the runway matches the terrain without the geoid
/// reports mean sea level; one transition decides nothing (it may be a stale reading), and where
/// the geoid is flat nothing can tell: the ellipsoid stays.
#[test]
fn the_datum_follows_the_geometric_altitude_on_the_runway() {
    let terrain_m = 380.0f32;
    let flight = |geometric_m: f32| {
        vec![
            surface(0.0),
            point(1.0, 1000.0, geometric_m / M_PER_FT),
            point(3.0, 1100.0, (geometric_m + 30.0) / M_PER_FT),
            point(600.0, 1100.0, (geometric_m + 30.0) / M_PER_FT),
            point(602.0, 1000.0, geometric_m / M_PER_FT),
            surface(603.0),
        ]
    };
    let at_terrain = |_: &TracePoint| Ok(terrain_m);
    let datum = |points: &[TracePoint], geoid: &Geoid| geometric_datum(points, geoid, at_terrain);
    let geoid = Geoid::uniform(UNDULATION_M);
    let ellipsoid = flight(terrain_m + UNDULATION_M as f32 + 5.0);
    assert_eq!(datum(&ellipsoid, &geoid), Ok(GeometricDatum::Ellipsoid));
    let sea_level = flight(terrain_m + 5.0);
    assert_eq!(datum(&sea_level, &geoid), Ok(GeometricDatum::MeanSeaLevel));
    assert_eq!(
        datum(&sea_level[..3], &geoid),
        Ok(GeometricDatum::Ellipsoid)
    );
    assert_eq!(
        datum(&sea_level, &Geoid::uniform(4.0)),
        Ok(GeometricDatum::Ellipsoid)
    );
    // The geometric sample of a transition may lie 30 s up, carried by the barometric difference.
    let mut late = flight(terrain_m + 5.0);
    late[1].geometric_altitude_ft = f32::NAN;
    late[2].geometric_altitude_ft = (terrain_m + 5.0) / M_PER_FT + 100.0;
    assert_eq!(datum(&late, &geoid), Ok(GeometricDatum::MeanSeaLevel));
}

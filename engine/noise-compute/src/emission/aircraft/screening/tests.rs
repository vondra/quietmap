//! Reference cases for the airborne vector-building horizon.

use std::sync::Arc;

use crate::propagation::obstacle_index::{CrossingScratch, ObstacleIndex, ObstacleKind};

use super::*;

struct FlatGround;

impl RasterSampler for FlatGround {
    fn elevation(&self, _lat: f64, _lon: f64) -> f64 {
        0.0
    }

    fn ground_g(&self, _lat: f64, _lon: f64) -> f64 {
        0.0
    }

    fn building_enclosure(&self, _lat: f64, _lon: f64) -> f64 {
        0.0
    }
}

fn rectangle_set(kind: ObstacleKind, height_m: f32) -> ObstacleSet {
    const LAT: f64 = 50.0;
    const LON: f64 = 14.0;
    let m_per_deg_lon = M_PER_DEG_LAT * LAT.to_radians().cos();
    let point =
        |east_m: f64, north_m: f64| (LAT + north_m / M_PER_DEG_LAT, LON + east_m / m_per_deg_lon);
    let mut builder = ObstacleIndex::builder(LAT, LON);
    builder.add_ring(
        &[
            point(90.0, -10.0),
            point(110.0, -10.0),
            point(110.0, 10.0),
            point(90.0, 10.0),
        ],
        height_m,
        kind,
        0,
    );
    ObstacleSet {
        indexes: vec![Arc::new(builder.build())],
    }
}

fn surrounding_building_set() -> ObstacleSet {
    const LAT: f64 = 50.0;
    const LON: f64 = 14.0;
    let m_per_deg_lon = M_PER_DEG_LAT * LAT.to_radians().cos();
    let point =
        |east_m: f64, north_m: f64| (LAT + north_m / M_PER_DEG_LAT, LON + east_m / m_per_deg_lon);
    let mut builder = ObstacleIndex::builder(LAT - 0.01, LON - 0.01);
    for (id, (east_lo, east_hi, north_lo, north_hi, height_m)) in [
        (90.0, 110.0, -10.0, 10.0, 12.0),
        (-110.0, -90.0, -10.0, 10.0, 18.0),
        (180.0, 220.0, 180.0, 220.0, 24.0),
        (-310.0, -290.0, 90.0, 110.0, 30.0),
    ]
    .into_iter()
    .enumerate()
    {
        builder.add_ring(
            &[
                point(east_lo, north_lo),
                point(east_hi, north_lo),
                point(east_hi, north_hi),
                point(east_lo, north_hi),
            ],
            height_m,
            ObstacleKind::Building,
            id as u32,
        );
    }
    builder.add_ring(
        &[
            point(-10.0, -60.0),
            point(10.0, -60.0),
            point(10.0, -40.0),
            point(-10.0, -40.0),
        ],
        100.0,
        ObstacleKind::Barrier,
        4,
    );
    ObstacleSet {
        indexes: vec![Arc::new(builder.build())],
    }
}

fn horizon_from_individual_sector_rays(set: &ObstacleSet) -> BuildingHorizon {
    const LAT: f64 = 50.0;
    const LON: f64 = 14.0;
    const RECEIVER_ALT_M: f64 = 4.0;
    let directions = building_local_directions();
    let m_per_deg_lon = M_PER_DEG_LAT * LAT.to_radians().cos();
    let mut best = [[(f64::NEG_INFINITY, 0.0_f64); BUILDING_LOCAL_HORIZON_BANDS];
        BUILDING_LOCAL_HORIZON_SECTORS];
    let mut crossings = Vec::new();
    for (sector, &(sin_angle, cos_angle)) in directions.iter().enumerate() {
        let end_lat = LAT + sin_angle * BUILDING_LOCAL_MAX_M / M_PER_DEG_LAT;
        let end_lon = LON + cos_angle * BUILDING_LOCAL_MAX_M / m_per_deg_lon;
        set.crossings(LAT, LON, end_lat, end_lon, &mut crossings);
        for crossing in crossings
            .iter()
            .filter(|crossing| crossing.kind == ObstacleKind::Building)
        {
            let range_m = crossing.t * BUILDING_LOCAL_MAX_M;
            if range_m <= 0.01 {
                continue;
            }
            let tangent = (f64::from(crossing.height_m) - RECEIVER_ALT_M) / range_m;
            let band = BUILDING_LOCAL_RANGE_BREAK_M
                .iter()
                .position(|&break_m| range_m <= break_m)
                .unwrap_or(BUILDING_LOCAL_HORIZON_BANDS - 1);
            if tangent > best[sector][band].0 {
                best[sector][band] = (tangent, range_m);
            }
        }
    }
    let (local, local_max_tangent_bits) = pack_sector_bands(&best);
    BuildingHorizon {
        local,
        local_max_tangent_bits,
    }
}

#[test]
fn building_below_and_above_line_of_sight() {
    let set = rectangle_set(ObstacleKind::Building, 12.0);
    let mut crossings = CrossingScratch::default();
    let horizon = BuildingHorizon::build(&set, &FlatGround, 50.0, 14.0, 4.0, &mut crossings);
    let blocked_db = horizon.screening_dz(200.0, 0.0, 5.0);
    assert!((5.0..=18.0).contains(&blocked_db), "{blocked_db}");
    assert_eq!(horizon.screening_dz(200.0, 0.0, 30.0), 0.0);
}

#[test]
fn noise_barrier_is_not_an_airborne_building() {
    let set = rectangle_set(ObstacleKind::Barrier, 100.0);
    let mut crossings = CrossingScratch::default();
    let horizon = BuildingHorizon::build(&set, &FlatGround, 50.0, 14.0, 4.0, &mut crossings);
    assert_eq!(horizon.screening_dz(200.0, 0.0, 5.0), 0.0);
}

#[test]
fn one_neighbourhood_scan_matches_individual_sector_rays() {
    let set = surrounding_building_set();
    let mut scratch = CrossingScratch::default();
    let scanned = BuildingHorizon::build(&set, &FlatGround, 50.0, 14.0, 4.0, &mut scratch);
    let individually_cast = horizon_from_individual_sector_rays(&set);
    assert_eq!(scanned.local, individually_cast.local);
    assert_eq!(
        scanned.local_max_tangent_bits,
        individually_cast.local_max_tangent_bits
    );
}

#[test]
fn anchored_iso_form_is_zero_at_grazing_and_caps_at_eighteen() {
    assert_eq!(single_edge_diffraction_db(0.0), 0.0);
    assert_eq!(single_edge_diffraction_db(1_000.0), 18.0);
}

#[test]
fn negative_zero_bearing_stays_inside_the_sector_array() {
    let set = ObstacleSet::empty();
    let mut crossings = CrossingScratch::default();
    let horizon = BuildingHorizon::build(&set, &FlatGround, 50.0, 14.0, 4.0, &mut crossings);
    assert_eq!(horizon.screening_dz(2.0, -0.0, 1.0), 0.0);
}

#[test]
fn building_tangent_encoding_keeps_vertical_range_without_raising_roofs() {
    for tangent in [-50.125, -0.123_456, 0.0, 0.123_456, 50.125] {
        let decoded = decode_building_tangent(encode_building_tangent_floor(tangent));
        assert!(decoded <= tangent, "decoded={decoded} tangent={tangent}");
        assert!((decoded - tangent).abs() <= tangent.abs().max(1.0) / 128.0);
    }
}

#[test]
fn close_facade_range_keeps_centimetres() {
    let set = rectangle_set(ObstacleKind::Building, 12.0);
    let mut crossings = CrossingScratch::default();
    let horizon = BuildingHorizon::build(&set, &FlatGround, 50.0, 14.0, 4.0, &mut crossings);
    let range_q = horizon.local[0][3].1;
    assert!((9_000..=9_010).contains(&range_q), "range_q={range_q}");
    assert_ne!(range_q % 100, 0, "range collapsed to whole metres");
}

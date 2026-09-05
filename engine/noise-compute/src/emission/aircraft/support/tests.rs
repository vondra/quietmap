//! Actual popup kernels and floating-point envelope boundaries must fit publication support.

use super::*;
use crate::compute::aircraft_v6::{
    airborne, cruise, AirborneRowView, BBox, CruiseRowView, SubSegmentSlice,
};
use crate::emission::aircraft::{ClassWeights, ReceiverHorizon};
use crate::types::{RasterSampler, Receiver};
use std::collections::HashMap;

struct FlatGround;
impl RasterSampler for FlatGround {
    fn elevation(&self, _: f64, _: f64) -> f64 {
        0.0
    }
    fn ground_g(&self, _: f64, _: f64) -> f64 {
        1.0
    }
    fn building_enclosure(&self, _: f64, _: f64) -> f64 {
        0.0
    }
}

#[test]
fn periodic_selection_distinguishes_short_arcs_from_aggregate_bounds() {
    let seam = AirborneEnvelope::new(0.001, 179.5);
    let opposite = AirborneEnvelope::new(0.001, 0.0);
    let bbox = [0.0, -179.0, 0.0, 179.0];
    assert!(seam.intersects_bbox(bbox));
    assert!(opposite.intersects_bbox(bbox));
    assert!(seam.intersects_segment([0.0, 179.0], [0.0, -179.0]));
    assert!(!opposite.intersects_segment([0.0, 179.0], [0.0, -179.0]));
    for lon in [-180.0, 180.0, 540.0] {
        let envelope = AirborneEnvelope::new(0.0, lon);
        assert!(envelope.intersects_segment([0.0, 179.99], [0.0, -179.99]));
        assert!(!envelope.intersects_segment([0.0, -179.85], [0.0, -179.75]));
        assert!(!envelope.intersects_bbox([0.0, -0.01, 0.0, 0.01]));
        assert!(!envelope.intersects_bbox([1.0, -179.0, 2.0, 179.0]));
    }
    // The existing shortest-delta convention chooses the negative half-turn.
    assert!(opposite.intersects_segment([0.0, 90.0], [0.0, -90.0]));
    assert!(!AirborneEnvelope::new(0.0, 180.0).intersects_segment([0.0, 90.0], [0.0, -90.0]));
}

#[test]
fn publication_encloses_periodic_f32_boundary_bins_without_seam_copies() {
    let lat_pad = meters_to_lat_deg(AIRCRAFT_MAX_HORIZONTAL_REACH_M);
    let mut accepted = 0;
    for lat in [-85.0_f32, -50.0, 0.0, 50.0, 85.0] {
        for lon in [-180.0_f32, -179.99, -90.0, 0.0, 89.99, 179.99, 180.0] {
            for length in [0.0, 0.01, 2.0, 179.999, 180.0] {
                let end_lon = grid::geo::normalize_longitude(f64::from(lon) + length) as f32;
                let start = [lat, lon];
                let end = [lat, end_lon];
                let support = airborne_support_cells(start, end).unwrap();
                let [west, east] = airborne_longitude_interval(lon, end_lon);
                for rx_lat in [f64::from(lat) - lat_pad, f64::from(lat) + lat_pad] {
                    for rx_lat in [rx_lat.next_down(), rx_lat, rx_lat.next_up()] {
                        let lon_pad = meters_to_lon_deg(rx_lat, AIRCRAFT_MAX_HORIZONTAL_REACH_M);
                        for rx_lon in [west - lon_pad, west, east, east + lon_pad] {
                            for rx_lon in [rx_lon.next_down(), rx_lon, rx_lon.next_up()] {
                                let rx_lon = grid::geo::normalize_longitude(rx_lon);
                                let envelope = AirborneEnvelope::new(rx_lat, rx_lon);
                                if envelope.intersects_segment(start, end) {
                                    accepted += 1;
                                    assert!(envelope.intersects_bbox([
                                        f64::from(lat),
                                        f64::from(lon.min(end_lon)),
                                        f64::from(lat),
                                        f64::from(lon.max(end_lon)),
                                    ]));
                                    assert!(support.contains(grid::square_of(rx_lat, rx_lon)),
                                        "{start:?}->{end:?}, receiver={rx_lat},{rx_lon}, {support:?}");
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(accepted > 0);
    let local = airborne_support_cells([0.0, -0.001], [0.0, 0.001]).unwrap();
    assert!(!local.contains(grid::square_of(0.0, 180.0)));
    let crossing = airborne_support_cells([0.0, 179.0], [0.0, -179.0]).unwrap();
    assert!(crossing.contains(grid::square_of(0.0, 179.5)));
    assert!(!crossing.contains(grid::square_of(0.0, 0.0)));
    eprintln!("periodic publication boundary proof: accepted={accepted}");
}

#[test]
fn support_contains_actual_kernel_receivers_and_rounded_bbox_edges() {
    let profile = crate::emission::profiles_generated::profile_idx("B738");
    let mut airborne_positive = 0;
    for (start, end, receiver) in [
        ([52.001_f32, 14.26], [50.001_f32, 14.26], [50.001, 14.261]),
        (
            [80.178_71, 0.0],
            [80.178_71, 0.001],
            [80.05804856215623, 0.0],
        ),
        ([50.0, 179.99], [50.0, -179.99], [50.001, 180.0]),
        ([-50.0, -179.99], [-50.0, 179.99], [-50.001, -180.0]),
        ([0.0, -0.001], [0.0, 0.001], [0.0, 180.0]),
    ] {
        let support = airborne_support_cells(start, end).unwrap();
        let row = AirborneRowView {
            flight_id: 42,
            callsign: "SUPPORT42",
            aircraft_type: *b"B738",
            profile_idx: profile,
            source_id: 2,
            origin: 0,
            bbox: BBox {
                min_lat: start[0].min(end[0]),
                max_lat: start[0].max(end[0]),
                min_lon: start[1].min(end[1]),
                max_lon: start[1].max(end[1]),
            },
            sub_segments: SubSegmentSlice {
                start_lat: &[start[0]],
                start_lon: &[start[1]],
                start_alt_m: &[1000.0],
                end_lat: &[end[0]],
                end_lon: &[end[1]],
                end_alt_m: &[1000.0],
                speed_kt: &[450.0],
                length_m: &[221080.0],
                period: &[0],
                date_id: &[0],
                flags: &[1],
                terrain_start_elev_m: &[0.0],
                terrain_end_elev_m: &[0.0],
            },
        };
        let receiver = Receiver::new(receiver[0], receiver[1], 0.0);
        let horizon = ReceiverHorizon::build(
            |_, _| 0.0,
            receiver.lat,
            receiver.lon,
            receiver.altitude_m(),
        );
        let flights = airborne::scatter(
            &receiver,
            &[row],
            12.0,
            &ClassWeights::uniform(),
            &horizon,
            None,
            0,
            None,
        );
        if !flights.is_empty() {
            airborne_positive += 1;
            assert!(support.contains(grid::square_of(receiver.lat, receiver.lon)));
        }
        assert_eq!(
            support.cell_count(),
            support
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
        );
        // Exercise the same f32 receiver-envelope comparisons at adjacent
        // representable endpoint values, including translated seam intervals.
        let pad = meters_to_lat_deg(AIRCRAFT_MAX_HORIZONTAL_REACH_M);
        for lat in [
            f64::from(start[0].min(end[0])) - pad,
            f64::from(start[0].max(end[0])) + pad,
        ] {
            for lat in [lat.next_down(), lat, lat.next_up()] {
                for lon in [f64::from(start[1]), -180.0, 180.0] {
                    if AirborneEnvelope::new(lat, lon).intersects_segment(start, end) {
                        assert!(
                            support.contains(grid::square_of(lat, lon)),
                            "{lat},{lon}: {support:?}"
                        );
                    }
                }
            }
        }
    }
    assert!(
        airborne_positive >= 3,
        "actual airborne positives: {airborne_positive}"
    );

    let mut cruise_positive = 0;
    for lat in [0.0, 50.0, 80.0, -80.0] {
        for lon in [0.0, 179.99, -179.99] {
            for rep_len_m in [5000.0_f32, 400000.0, 2778000.0] {
                let row = CruiseRowView {
                    lat,
                    lon,
                    class: 3,
                    rep_profile_idx: profile,
                    fl_bin: 4,
                    period: 2,
                    sum_length_m: 1000.0,
                    rep_len_m,
                    rep_alt_m: 11000.0,
                    rep_speed_kt: 450.0,
                    source_id: 2,
                    origin: 0,
                    unique_count: 1,
                    top_candidates: &[],
                };
                let support = cruise_support_cells(lat, lon, rep_len_m).unwrap();
                let (dy, dx) = cruise::cruise_synth_offsets(lat, f64::from(rep_len_m) / 2.0);
                for fraction in [-1.05, -0.8, 0.0, 0.8, 1.05] {
                    let rx_lat = lat + fraction * dy;
                    if !(-90.0..=90.0).contains(&rx_lat) {
                        continue;
                    }
                    let rx_lon = grid::geo::normalize_longitude(lon + fraction * dx);
                    let receiver = Receiver::new(rx_lat, rx_lon, 0.0);
                    let mut flights = HashMap::new();
                    cruise::scatter(
                        &receiver,
                        std::slice::from_ref(&row),
                        &FlatGround,
                        12.0,
                        &mut flights,
                        &mut HashMap::new(),
                        &mut HashMap::new(),
                        None,
                    );
                    if !flights.is_empty() {
                        cruise_positive += 1;
                        assert!(
                            support.contains(grid::square_of(rx_lat, rx_lon)),
                            "{lat},{lon} len={rep_len_m} rx={rx_lat},{rx_lon}"
                        );
                    }
                }
            }
        }
    }
    assert!(
        cruise_positive >= 36,
        "actual cruise positives: {cruise_positive}"
    );
    eprintln!(
        "support actual-kernel proof: airborne={airborne_positive}, cruise={cruise_positive}"
    );
}

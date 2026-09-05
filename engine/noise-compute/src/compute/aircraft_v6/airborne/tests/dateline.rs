//! The same physical flight and ridge must survive a longitude seam unchanged.

use super::*;
use grid::geo::{normalize_longitude, wrapped_longitude_delta};

// Binary fractions survive the runtime f32 view exactly, even beside ±180°.
const STEP_DEG: f64 = 1.0 / 1024.0;

fn scene(receiver_lat: f64, receiver_lon: f64, eastbound: bool, side: f64) -> [f64; 8] {
    let receiver = Receiver::new(receiver_lat, receiver_lon, 0.0);
    let longitude_scale = aircraft::M_PER_DEG_LAT * receiver_lat.to_radians().cos().max(0.2);
    let (start_x, start_y, end_x, end_y) = if eastbound {
        (-4.0, side * 2.0, 4.0, side * 2.0)
    } else {
        (side * 2.0, -4.0, side * 2.0, 4.0)
    };
    let mut columns = one_subseg("B738");
    columns.start_lon = [normalize_longitude(receiver_lon + start_x * STEP_DEG) as f32];
    columns.end_lon = [normalize_longitude(receiver_lon + end_x * STEP_DEG) as f32];
    columns.start_lat = [(receiver_lat + start_y * STEP_DEG) as f32];
    columns.end_lat = [(receiver_lat + end_y * STEP_DEG) as f32];
    columns.alt = [20.0];
    columns.flags = [1];
    columns.length = [(8.0
        * STEP_DEG
        * if eastbound {
            longitude_scale
        } else {
            aircraft::M_PER_DEG_LAT
        }) as f32];
    let horizon = aircraft::ReceiverHorizon::build(
        |lat, lon| {
            let offset = side
                * if eastbound {
                    (lat - receiver_lat) * aircraft::M_PER_DEG_LAT
                } else {
                    wrapped_longitude_delta(receiver_lon, lon) * longitude_scale
                };
            if (30.0..=40.0).contains(&offset) {
                100.0
            } else {
                0.0
            }
        },
        receiver_lat,
        receiver_lon,
        receiver.altitude_m(),
    );
    let row = one_subseg_row(1, "B738", &columns);
    let flights = scatter(
        &receiver,
        &[row],
        1.0,
        &aircraft::ClassWeights::uniform(),
        &horizon,
        None,
        0,
        None,
    );
    let flight = flights.get(&1).unwrap_or_else(|| panic!(
        "nearby flight disappeared: lat={receiver_lat} lon={receiver_lon} eastbound={eastbound} side={side}"
    ));
    assert!(
        flight.free_period_energy[0] > flight.period_energy[0],
        "ridge must screen"
    );
    let segment = AircraftSegment {
        flight_id: 1,
        profile_idx: row.profile_idx,
        is_departure: true,
        on_ground: false,
        period: 0,
        date_id: 0,
        start_lat: columns.start_lat[0] as f64,
        start_lon: columns.start_lon[0] as f64,
        start_alt_m: columns.alt[0],
        end_lat: columns.end_lat[0] as f64,
        end_lon: columns.end_lon[0] as f64,
        end_alt_m: columns.alt[0],
        speed_kt: columns.speed[0],
        segment_length_m: columns.length[0],
        count_weight: 1.0,
        surface_model: false,
        ground_context: aircraft::GROUND_CONTEXT_NONE,
        ground_ops_kind: aircraft::GROUND_OPS_KIND_NONE,
        source_id: 0,
    };
    let receiver_alt = receiver.altitude_m();
    let cpa = aircraft::compute_cpa(
        receiver_lat,
        receiver_lon,
        receiver_alt,
        segment.start_lat,
        segment.start_lon,
        segment.start_alt_m as f64,
        segment.end_lat,
        segment.end_lon,
        segment.end_alt_m as f64,
    );
    let slant_sq = aircraft::segment_min_slant_sq(
        &segment,
        receiver_lat,
        receiver_lon,
        receiver_alt,
        receiver_lat.to_radians().cos().max(0.2),
    );
    assert!(aircraft::within_kernel_reach(
        &segment,
        receiver_lat,
        receiver_lon,
        receiver_alt
    ));
    let prepared = aircraft::prepare_segment(&segment, -30.0, -30.0);
    let row_state = aircraft::prepare_row(&prepared, receiver_lat, longitude_scale);
    let (sel, _) = aircraft::segment_sel_at_pixel(
        &prepared,
        &row_state,
        receiver_lon,
        receiver_alt,
        aircraft::NpdLuts::shared(),
        Some(&horizon),
    )
    .expect("prepared SEL");
    let energy_sel = aircraft::segment_sel_at_pixel_energy(
        &prepared,
        &row_state,
        receiver_lon,
        receiver_alt,
        aircraft::NpdLuts::shared(),
        Some(&horizon),
    )
    .expect("prepared energy SEL");
    assert_eq!(sel.to_bits(), energy_sel.to_bits());
    assert!(
        (sel - flight.peak_sel).abs() < 1e-8,
        "prepared/popup SEL differs"
    );
    [
        flight.peak_sel,
        flight.peak_lmax,
        flight.min_dist_m,
        flight.free_period_energy[0].log10(),
        cpa.d_p_m,
        cpa.seg_len_m,
        cpa.t,
        slant_sq.sqrt(),
    ]
}

#[test]
fn dateline_airborne_popup_and_kernel_match_translated_scene() {
    let mut maximum_difference = 0.0_f64;
    let mut compared = 0;
    for latitude in [0.0, 50.0, 80.0] {
        for eastbound in [false, true] {
            for side in [-1.0, 1.0] {
                let expected = scene(latitude, 0.0, eastbound, side);
                for longitude in [180.0 - STEP_DEG, -180.0 + STEP_DEG, -180.0, 180.0] {
                    let actual = scene(latitude, longitude, eastbound, side);
                    for (index, (got, want)) in actual.iter().zip(&expected).enumerate() {
                        maximum_difference = maximum_difference.max((got - want).abs());
                        assert!((got - want).abs() < 1e-8,
                            "metric {index}: lat={latitude} lon={longitude} eastbound={eastbound} side={side}: {got} != {want}");
                    }
                    compared += 1;
                }
            }
        }
    }
    // Independently wrapping endpoint offsets would stretch this Greenwich
    // segment across the globe for a receiver on the opposite meridian.
    for receiver_lon in [0.0, 180.0, -180.0] {
        let cpa = aircraft::compute_cpa(
            0.0,
            receiver_lon,
            4.0,
            0.0,
            -STEP_DEG,
            100.0,
            0.0,
            STEP_DEG,
            100.0,
        );
        assert!((cpa.seg_len_m - 2.0 * STEP_DEG * aircraft::M_PER_DEG_LAT).abs() < 1e-8);
    }
    eprintln!("airborne dateline: {compared} translated scenes; max metric difference {maximum_difference:e}");
}

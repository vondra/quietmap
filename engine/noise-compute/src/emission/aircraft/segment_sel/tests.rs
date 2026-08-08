use super::super::segment_filters::{
    GROUND_CONTEXT_AIRPORT_LINE, GROUND_CONTEXT_NONE, GROUND_OPS_KIND_NONE,
};
use super::*;
use crate::sources::AIRCRAFT_ADSB_SOURCE_ID;
use crate::types::default_receiver_altitude_m;

struct FlatGround;

impl RasterSampler for FlatGround {
    fn elevation(&self, _lat: f64, _lon: f64) -> f64 {
        250.0
    }
    fn building_height(&self, _lat: f64, _lon: f64) -> f64 {
        0.0
    }
    fn ground_g(&self, _lat: f64, _lon: f64) -> f64 {
        0.0
    }
    fn building_enclosure(&self, _lat: f64, _lon: f64) -> f64 {
        0.0
    }
}

#[test]
fn test_segment_sel_b738_approach() {
    let seg = AircraftSegment {
        flight_id: 1,
        profile_idx: 0,
        is_departure: false,
        on_ground: false,
        period: 0,
        date_id: 0,
        start_lat: 50.0,
        start_lon: 14.0,
        start_alt_m: 1000.0,
        end_lat: 50.01,
        end_lon: 14.0,
        end_alt_m: 900.0,
        speed_kt: 150.0,
        segment_length_m: 1100.0,
        ground_context: GROUND_CONTEXT_NONE,
        ground_ops_kind: GROUND_OPS_KIND_NONE,
        count_weight: 1.0,
        surface_model: false,
        source_id: AIRCRAFT_ADSB_SOURCE_ID,
    };
    let result = segment_sel(&seg, 50.005, 14.005, 300.0, &FlatGround);
    assert!(result.is_some(), "should compute SEL for nearby segment");
    let (sel, cpa) = result.unwrap();
    assert!(sel > 50.0 && sel < 110.0, "SEL = {sel}");
    assert!(
        cpa.d_p_m > 100.0 && cpa.d_p_m < 2000.0,
        "d_p = {}",
        cpa.d_p_m
    );
}

#[test]
fn test_segment_sel_far_away() {
    let seg = AircraftSegment {
        flight_id: 1,
        profile_idx: 0,
        is_departure: false,
        on_ground: false,
        period: 0,
        date_id: 0,
        start_lat: 51.0,
        start_lon: 15.0,
        start_alt_m: 10000.0,
        end_lat: 51.01,
        end_lon: 15.0,
        end_alt_m: 10000.0,
        speed_kt: 250.0,
        segment_length_m: 1100.0,
        ground_context: GROUND_CONTEXT_NONE,
        ground_ops_kind: GROUND_OPS_KIND_NONE,
        count_weight: 1.0,
        surface_model: false,
        source_id: AIRCRAFT_ADSB_SOURCE_ID,
    };
    let result = segment_sel(&seg, 50.0, 14.0, 300.0, &FlatGround);
    assert!(result.is_none(), "should be None for far segment");
}

#[test]
fn test_airport_ground_sel_recovers_bad_altitude() {
    let seg = AircraftSegment {
        flight_id: 1,
        profile_idx: 7,
        is_departure: false,
        on_ground: false,
        period: 0,
        date_id: 0,
        start_lat: 50.0,
        start_lon: 14.0,
        start_alt_m: 200.0,
        end_lat: 50.0008,
        end_lon: 14.0,
        end_alt_m: 200.0,
        speed_kt: 35.0,
        segment_length_m: 90.0,
        ground_context: GROUND_CONTEXT_AIRPORT_LINE,
        ground_ops_kind: GROUND_OPS_KIND_NONE,
        count_weight: 1.0,
        surface_model: false,
        source_id: AIRCRAFT_ADSB_SOURCE_ID,
    };

    let normal = segment_sel(&seg, 50.0004, 14.0, 254.0, &FlatGround)
        .expect("baseline airport segment should still compute")
        .0;
    let corrected = segment_sel_airport_ground(&seg, 50.0004, 14.0, 254.0, &FlatGround)
        .expect("airport ground override should compute")
        .0;

    assert!(
        corrected > normal + 10.0,
        "normal={normal:.2} corrected={corrected:.2}"
    );
}

#[test]
fn test_ground_ops_model_avoids_doc29_near_field_extrapolation() {
    // Cross-module integration: ground_ops model shouldn't extrapolate
    // through Doc 29's near-field. The ground reference SEL is tabulated
    // separately so it stays bounded vs the airborne kernel.
    let seg = AircraftSegment {
        flight_id: 1,
        profile_idx: 7,
        is_departure: false,
        on_ground: true,
        period: 0,
        date_id: 0,
        start_lat: 50.0,
        start_lon: 14.0,
        start_alt_m: 0.0,
        end_lat: 50.006,
        end_lon: 14.0,
        end_alt_m: 0.0,
        speed_kt: 70.0,
        segment_length_m: 650.0,
        ground_context: GROUND_CONTEXT_AIRPORT_LINE,
        ground_ops_kind: 1, // RUNWAY_ROLL
        count_weight: 1.0,
        surface_model: false,
        source_id: AIRCRAFT_ADSB_SOURCE_ID,
    };
    let rasters = FlatGround;
    let rx_lat = (seg.start_lat + seg.end_lat) * 0.5;
    let rx_lon = seg.start_lon + super::super::segment_filters::meters_to_lon_deg(rx_lat, 5.0);
    let rx_alt = default_receiver_altitude_m(rasters.elevation(rx_lat, rx_lon));
    let doc29_sel = segment_sel_airport_ground(&seg, rx_lat, rx_lon, rx_alt, &rasters)
        .expect("Doc 29 runway reference SEL should compute")
        .0;

    // Sanity: airborne extrapolation in the near field shouldn't blow up.
    assert!(doc29_sel.is_finite(), "doc29_sel = {doc29_sel}");
}

/// Parity between the hoisted three-stage API and the single-shot
/// `segment_sel_with_cuts`. Sub-ULP drift accepted (see inline note
/// on the SEL assert).
#[test]
fn hoisted_matches_segment_sel_with_cuts() {
    let seg = AircraftSegment {
        flight_id: 1,
        profile_idx: 0,
        is_departure: true,
        on_ground: false,
        period: 0,
        date_id: 0,
        start_lat: 50.10,
        start_lon: 14.25,
        start_alt_m: 1500.0,
        end_lat: 50.12,
        end_lon: 14.30,
        end_alt_m: 1700.0,
        speed_kt: 220.0,
        segment_length_m: 4500.0,
        ground_context: GROUND_CONTEXT_NONE,
        ground_ops_kind: GROUND_OPS_KIND_NONE,
        count_weight: 1.0,
        surface_model: false,
        source_id: AIRCRAFT_ADSB_SOURCE_ID,
    };
    let start_cut = 200.0;
    let end_cut = 210.0;
    let npd_luts = NpdLuts::shared();
    let prepared = prepare_segment(&seg, start_cut, end_cut);

    // Sample on a grid that exercises far-field, near-field, and
    // out-of-reach pixels in one sweep.
    let lats = [50.05, 50.08, 50.10, 50.12, 50.18];
    let lons = [14.10, 14.20, 14.25, 14.28, 14.40];
    let elevs = [200.0, 300.0, 800.0];

    let mut compared = 0usize;
    for &rx_lat in &lats {
        let row_state = prepare_row(
            &prepared,
            rx_lat,
            M_PER_DEG_LAT * rx_lat.to_radians().cos().max(0.2),
        );
        for &rx_lon in &lons {
            for &rx_elev in &elevs {
                let want = segment_sel_with_cuts(
                    &seg, rx_lat, rx_lon, rx_elev, start_cut, end_cut, npd_luts, None,
                );
                let got =
                    segment_sel_at_pixel(&prepared, &row_state, rx_lon, rx_elev, npd_luts, None);
                match (want, got) {
                        (None, None) => {}
                        (Some((s_w, c_w)), Some((s_g, c_g))) => {
                            // Sub-ULP drift: hoisted path computes `sdx`
                            // as `d_lon · m_per_deg_lon`, donor as
                            // `(bx − ax)`. Mathematically equal, f64
                            // non-associative — diverges by a few ULPs.
                            // Tile cell quantizes to u8 × 0.5 dB.
                            assert!((s_w - s_g).abs() < 1e-6,
                                "SEL drift at ({rx_lat},{rx_lon},{rx_elev}): want={s_w} got={s_g} diff={}",
                                s_w - s_g);
                            assert!((c_w.d_p_m - c_g.d_p_m).abs() < 1e-3);
                            assert!((c_w.lateral_m - c_g.lateral_m).abs() < 1e-3);
                            assert!((c_w.relative_alt_m - c_g.relative_alt_m).abs() < 1e-6);
                            compared += 1;
                        }
                        (a, b) => panic!(
                            "filter mismatch at ({rx_lat},{rx_lon},{rx_elev}): want_is_some={} got_is_some={}",
                            a.is_some(), b.is_some()),
                    }
            }
        }
    }
    assert!(
        compared >= 30,
        "expected at least 30 paired samples, got {compared}"
    );
}

/// Display/energy split on a curving-departure phantom: the kernel keeps a
/// tiny unclamped `cpa.d_p_m` (the SEL ΔF integral needs the infinite-line
/// foot), while `clamped_display_cpa` recovers the real far distance for the
/// popup. SEL is the kernel's own output — the display clamp never feeds it.
#[test]
fn display_clamp_does_not_touch_sel() {
    let seg = AircraftSegment {
        flight_id: 1,
        profile_idx: 0,
        is_departure: true,
        on_ground: false,
        period: 0,
        date_id: 0,
        start_lat: 50.1700,
        start_lon: 14.4197,
        start_alt_m: 1509.0,
        end_lat: 50.1711,
        end_lon: 14.4204,
        end_alt_m: 1532.0,
        speed_kt: 180.0,
        segment_length_m: 131.0,
        ground_context: GROUND_CONTEXT_NONE,
        ground_ops_kind: GROUND_OPS_KIND_NONE,
        count_weight: 1.0,
        surface_model: false,
        source_id: AIRCRAFT_ADSB_SOURCE_ID,
    };
    // Terrain cuts well below the segment so Filter D keeps it — the foot is
    // above terrain, which is exactly why the phantom leaks into display.
    let (sel, cpa) = segment_sel_with_cuts(
        &seg,
        50.1110,
        14.3819,
        279.0,
        170.0,
        170.0,
        NpdLuts::shared(),
        None,
    )
    .expect("phantom segment should still compute SEL");
    assert!(
        cpa.d_p_m < 100.0,
        "kernel keeps the phantom CPA for ΔF, got {}",
        cpa.d_p_m
    );

    let sdz = seg.end_alt_m as f64 - seg.start_alt_m as f64;
    let (disp_dist, _) = crate::emission::aircraft::clamped_display_cpa(&cpa, sdz);
    assert!(
        disp_dist > 5000.0,
        "display distance should be the real far approach, got {disp_dist}"
    );
    assert!(
        sel.is_finite() && sel > 20.0,
        "SEL is the kernel value, untouched by the clamp, got {sel}"
    );
}

// C2 horizon-screening kernel integration. Shared geometry: receiver
// at (50, 14); level N-S segments whose CPA foot lands due EAST of
// the receiver at a chosen lateral offset; ridges built due east via
// a receiver-local sampler (same projection the build uses).

const C2_RX_LAT: f64 = 50.0;
const C2_RX_LON: f64 = 14.0;

fn c2_m_per_deg_lon() -> f64 {
    M_PER_DEG_LAT * C2_RX_LAT.to_radians().cos().max(0.2)
}

/// Level N-S segment crossing lat 50 at `lateral_m` east of the
/// receiver — CPA foot at (50, lon + Δ), inside [0, 1] (no Filter D).
fn c2_level_segment(lateral_m: f64, alt_m: f32) -> AircraftSegment {
    let lon = C2_RX_LON + lateral_m / c2_m_per_deg_lon();
    AircraftSegment {
        flight_id: 1,
        profile_idx: 0, // class 0 = WING_FALLBACK (wing-mounted jet)
        is_departure: true,
        on_ground: false,
        period: 0,
        date_id: 0,
        start_lat: 49.97,
        start_lon: lon,
        start_alt_m: alt_m,
        end_lat: 50.03,
        end_lon: lon,
        end_alt_m: alt_m,
        speed_kt: 160.0,
        segment_length_m: 6670.0,
        ground_context: GROUND_CONTEXT_NONE,
        ground_ops_kind: GROUND_OPS_KIND_NONE,
        count_weight: 1.0,
        surface_model: false,
        source_id: AIRCRAFT_ADSB_SOURCE_ID,
    }
}

/// East-facing ridge horizon: band `[r0, r1]` m east at `top_m`,
/// flat `base_m` elsewhere; receiver datum `rx_alt`.
fn c2_east_ridge_horizon(
    r0: f64,
    r1: f64,
    top_m: f64,
    base_m: f64,
    rx_alt: f64,
) -> ReceiverHorizon {
    let m_per_deg_lon = c2_m_per_deg_lon();
    ReceiverHorizon::build(
        move |lat: f64, lon: f64| {
            let east = (lon - C2_RX_LON) * m_per_deg_lon;
            let north = (lat - C2_RX_LAT) * M_PER_DEG_LAT;
            if east >= r0 && east <= r1 && north.abs() < 600.0 {
                top_m
            } else {
                base_m
            }
        },
        C2_RX_LAT,
        C2_RX_LON,
        rx_alt,
    )
}

/// Hard invariant: `horizon = None` and a flat-terrain horizon
/// (max_sin_sq = 0 ⇒ precheck skips every positive-rel_alt pair)
/// produce BIT-identical SEL across the hoisted parity grid.
#[test]
fn flat_horizon_is_bit_identical_to_none() {
    let seg = c2_level_segment(2_000.0, 1_500.0);
    let npd_luts = NpdLuts::shared();
    let lats = [49.95, 50.0, 50.02, 50.10];
    let lons = [13.90, 14.0, 14.05, 14.20];
    let elevs = [200.0, 300.0, 800.0];
    let mut compared = 0usize;
    for &rx_lat in &lats {
        for &rx_lon in &lons {
            for &rx_elev in &elevs {
                // Flat ground 4 m below the receiver mast, like a real
                // flat-terrain popup query.
                let hz = ReceiverHorizon::build(|_, _| rx_elev - 4.0, rx_lat, rx_lon, rx_elev);
                assert_eq!(hz.max_sin_sq, 0.0);
                let none = segment_sel_with_cuts(
                    &seg, rx_lat, rx_lon, rx_elev, 170.0, 170.0, npd_luts, None,
                );
                let some = segment_sel_with_cuts(
                    &seg,
                    rx_lat,
                    rx_lon,
                    rx_elev,
                    170.0,
                    170.0,
                    npd_luts,
                    Some(&hz),
                );
                match (none, some) {
                    (None, None) => {}
                    (Some((s_n, _)), Some((s_s, _))) => {
                        assert_eq!(
                            s_n.to_bits(),
                            s_s.to_bits(),
                            "flat horizon must be bit-identical at ({rx_lat},{rx_lon},{rx_elev})"
                        );
                        compared += 1;
                    }
                    (a, b) => {
                        panic!("filter mismatch: none={} some={}", a.is_some(), b.is_some())
                    }
                }
            }
        }
    }
    assert!(
        compared >= 10,
        "expected ≥ 10 paired samples, got {compared}"
    );
}

/// Halltal-class deep valley (audit A2): horizon ≈ 34°, aircraft at
/// β = 20° behind it ⇒ screened by the capped Dz minus Λ. The
/// geometry deliberately sits ABOVE a fixed 15° precheck — if anyone
/// reintroduces one (the delta-3 rejected form), screening silently
/// turns off for this exact target case and the drop assert fails.
#[test]
fn halltal_class_valley_screens_above_15_degrees() {
    // 574 m rise at ~880 m east (the audit's Halltal blockage figure).
    let hz = c2_east_ridge_horizon(800.0, 960.0, 874.0, 300.0, 300.0);
    let seg = c2_level_segment(2_000.0, 1_028.0); // rel_alt 728, β = 20.0°
    let npd_luts = NpdLuts::shared();

    let (sel_none, cpa) = segment_sel_with_cuts(
        &seg, C2_RX_LAT, C2_RX_LON, 300.0, 270.0, 270.0, npd_luts, None,
    )
    .expect("unscreened SEL");
    let (sel_hz, _) = segment_sel_with_cuts(
        &seg,
        C2_RX_LAT,
        C2_RX_LON,
        300.0,
        270.0,
        270.0,
        npd_luts,
        Some(&hz),
    )
    .expect("screened SEL should stay above the 20 dB floor here");

    // Fixed-15° precheck regression guard: β = 20° > 15°, so the old
    // `rel_alt² < slant²·sin²15°` gate would SKIP this pair entirely.
    let slant_sq = cpa.d_p_m * cpa.d_p_m;
    let sin15_sq = 15f64.to_radians().sin().powi(2);
    assert!(
        cpa.relative_alt_m * cpa.relative_alt_m >= slant_sq * sin15_sq,
        "geometry must sit above a fixed 15° precheck for this regression to bite"
    );

    let drop = sel_none - sel_hz;
    assert!(
        drop >= 5.0,
        "Halltal-class valley must screen ≥ 5 dB, got {drop:.2}"
    );
    assert!(
        drop <= 18.0 + 1e-9,
        "net screening can never exceed the 18 dB cap, got {drop:.2}"
    );
}

/// Kytín guard (plan §6): a near low overflight (β ≈ 27°, lateral
/// 100 m) in front of a steep hillside at ~600 m must stay
/// UNSCREENED — the stored edge lies beyond the aircraft, so the
/// r_edge ≤ lateral bucket rule rejects it. Bit-identical to None.
#[test]
fn kytin_class_near_overflight_unscreened_bitwise() {
    let hz = c2_east_ridge_horizon(550.0, 650.0, 800.0, 300.0, 300.0);
    assert!(hz.max_sin_sq > 0.0, "the hillside must be a real horizon");
    let seg = c2_level_segment(100.0, 350.0); // rel_alt 50, β ≈ 26.6°
    let npd_luts = NpdLuts::shared();

    let (sel_none, _) = segment_sel_with_cuts(
        &seg, C2_RX_LAT, C2_RX_LON, 300.0, 270.0, 270.0, npd_luts, None,
    )
    .unwrap();
    let (sel_hz, _) = segment_sel_with_cuts(
        &seg,
        C2_RX_LAT,
        C2_RX_LON,
        300.0,
        270.0,
        270.0,
        npd_luts,
        Some(&hz),
    )
    .unwrap();
    assert_eq!(sel_none.to_bits(), sel_hz.to_bits());
}

/// Delta 1 (CRITICAL): screening is branch-aware. Both pairs sit in
/// deep shadow (Dz = 18 dB cap, identical), but the CFFK fast path
/// never subtracted Λ so it takes the full −18, while the full branch
/// credits its Λ back: −(18 − Λ).
#[test]
fn screening_is_branch_aware_cffk_vs_full() {
    use crate::emission::aircraft::fast_lateral_attenuation;
    // 500 m ridge at ~1.9 km: tan ≈ 0.26, deep shadow for both pairs.
    let hz = c2_east_ridge_horizon(1_900.0, 2_100.0, 800.0, 300.0, 300.0);
    let npd_luts = NpdLuts::shared();

    // Full-branch pair: slant ≈ 3.0 km < 7 620 m.
    let seg_near = c2_level_segment(3_000.0, 700.0); // rel_alt 400, β ≈ 7.6°
    let (sel_n_none, cpa_n) = segment_sel_with_cuts(
        &seg_near, C2_RX_LAT, C2_RX_LON, 300.0, 270.0, 270.0, npd_luts, None,
    )
    .unwrap();
    assert!(cpa_n.d_p_m < 7_620.0, "near pair must take the full branch");
    let (sel_n_hz, _) = segment_sel_with_cuts(
        &seg_near,
        C2_RX_LAT,
        C2_RX_LON,
        300.0,
        270.0,
        270.0,
        npd_luts,
        Some(&hz),
    )
    .unwrap();
    let lambda = fast_lateral_attenuation(
        cpa_n.relative_alt_m,
        cpa_n.lateral_m,
        false,
        Installation::Wing,
    );
    assert!(
        lambda > 1.0,
        "test geometry must carry real lateral attenuation"
    );
    let drop_full = sel_n_none - sel_n_hz;
    assert!(
        (drop_full - (18.0 - lambda)).abs() < 1e-9,
        "full branch must net (Dz − Λ): got {drop_full:.4}, want {:.4}",
        18.0 - lambda
    );

    // CFFK pair: slant ≈ 8.0 km > 7 620 m, same capped Dz.
    let seg_far = c2_level_segment(8_000.0, 900.0); // rel_alt 600, β ≈ 4.3°
    let (sel_f_none, cpa_f) = segment_sel_with_cuts(
        &seg_far, C2_RX_LAT, C2_RX_LON, 300.0, 270.0, 270.0, npd_luts, None,
    )
    .unwrap();
    assert!(cpa_f.d_p_m > 7_620.0, "far pair must take the CFFK branch");
    let (sel_f_hz, _) = segment_sel_with_cuts(
        &seg_far,
        C2_RX_LAT,
        C2_RX_LON,
        300.0,
        270.0,
        270.0,
        npd_luts,
        Some(&hz),
    )
    .unwrap();
    let drop_cffk = sel_f_none - sel_f_hz;
    assert!(
        (drop_cffk - 18.0).abs() < 1e-9,
        "CFFK branch must net the plain Dz (no Λ to credit): got {drop_cffk:.4}"
    );
}

/// Cruise-β geometry never screens (plan §1 structural exemption):
/// the cruise phase floor (AGL ≥ 7 200 m, slant ≤ 16 km) puts
/// β ≥ 26.6°, above any terrain horizon outside cliff faces — so a
/// strong-but-realistic 21.8° horizon must leave a cruise-floor pair
/// bit-identical even if a horizon were (wrongly) wired into the
/// cruise path. The production cruise path additionally hard-wires
/// `None` (`segment_sel_with_terrain` takes no horizon parameter).
#[test]
fn cruise_beta_never_screens() {
    // tan 0.4 ≈ 21.8° ridge at ~840 m — already steeper than real
    // valley horizons outside cliff faces.
    let hz = c2_east_ridge_horizon(800.0, 960.0, 636.0, 300.0, 300.0);
    // β = 26.6° at 8 km lateral ⇒ rel_alt ≈ 4 005 m (cruise floor).
    let seg = c2_level_segment(8_000.0, 4_305.0);
    let npd_luts = NpdLuts::shared();
    let none = segment_sel_with_cuts(
        &seg, C2_RX_LAT, C2_RX_LON, 300.0, 270.0, 270.0, npd_luts, None,
    );
    let some = segment_sel_with_cuts(
        &seg,
        C2_RX_LAT,
        C2_RX_LON,
        300.0,
        270.0,
        270.0,
        npd_luts,
        Some(&hz),
    );
    match (none, some) {
        (Some((s_n, _)), Some((s_s, _))) => {
            assert_eq!(
                s_n.to_bits(),
                s_s.to_bits(),
                "cruise-β pair must not screen"
            );
        }
        (a, b) => panic!(
            "cruise-β pair must compute: none={} some={}",
            a.is_some(),
            b.is_some()
        ),
    }
}

/// Receiver above the aircraft (negative rel_alt) routes through the
/// explicit `rel_alt <= 0` precheck arm — no NaN, no squaring trap —
/// and a descent below a hilltop's negative horizon still screens
/// net of Λ.
#[test]
fn negative_rel_alt_goes_through_full_check() {
    // Summit receiver at 1 000 m, gentle −0.05 downslope all around.
    let m_per_deg_lon = c2_m_per_deg_lon();
    let hz = ReceiverHorizon::build(
        move |lat: f64, lon: f64| {
            let east = (lon - C2_RX_LON) * m_per_deg_lon;
            let north = (lat - C2_RX_LAT) * M_PER_DEG_LAT;
            1_000.0 - 0.05 * (east * east + north * north).sqrt()
        },
        C2_RX_LAT,
        C2_RX_LON,
        1_000.0,
    );
    assert_eq!(hz.max_sin_sq, 0.0, "all-downhill horizon clamps to 0");

    // Aircraft 500 m BELOW the receiver at 1 km lateral: β = −26.6°,
    // well under the −2.9° horizon ⇒ blocked.
    let seg = c2_level_segment(1_000.0, 500.0);
    let npd_luts = NpdLuts::shared();
    let (sel_none, cpa) = segment_sel_with_cuts(
        &seg, C2_RX_LAT, C2_RX_LON, 1_000.0, 270.0, 270.0, npd_luts, None,
    )
    .unwrap();
    assert!(
        cpa.relative_alt_m < 0.0,
        "geometry must be receiver-above-aircraft"
    );
    let (sel_hz, _) = segment_sel_with_cuts(
        &seg,
        C2_RX_LAT,
        C2_RX_LON,
        1_000.0,
        270.0,
        270.0,
        npd_luts,
        Some(&hz),
    )
    .unwrap();
    assert!(sel_hz.is_finite());
    // Wing jet at β < 0 already carries Λ = 10.857; the net effect is
    // (Dz − Λ).max(0) ≥ 0 — assert direction, not magnitude.
    assert!(sel_hz <= sel_none, "screening can only reduce SEL");
}

/// `within_kernel_reach` must never reject a segment the full kernel
/// would have kept: cruise skips the DEM probes on its `false` verdict,
/// so a false negative here silently deletes energy from the popup.
///
/// Sweeps a grid of cruise-shaped geometries (level 50 km diagonals at
/// FL250-FL400) across receiver offsets spanning inside, on, and well
/// beyond the class reach, plus a couple of degenerate short segments.
#[test]
fn reach_gate_matches_kernel_rejection() {
    use crate::compute::aircraft_v6::cruise::cruise_synth_offsets;

    let npd = NpdLuts::shared();
    let (rx_lat, rx_lon, rx_elev) = (49.8_f64, 14.4_f64, 300.0_f64);
    let mut checked = 0;
    let mut gate_false = 0;
    let mut kernel_some = 0;
    for &rep_len_m in &[50_000.0_f64, 5_000.0, 10.0] {
        for &rep_alt_m in &[7_600.0_f32, 10_000.0, 12_200.0] {
            for &profile_idx in &[0_u8, 3, 7] {
                // Quadratic sweep: dense inside the class reach (~16 km),
                // coarse out to ~350 km so the gate is exercised on both
                // sides of its threshold.
                for step in 0..40 {
                    let d_deg = (step * step) as f64 * 0.002;
                    let (lat_off, lon_off) = cruise_synth_offsets(rx_lat + d_deg, rep_len_m * 0.5);
                    let seg = AircraftSegment {
                        flight_id: 1,
                        profile_idx,
                        is_departure: true,
                        on_ground: false,
                        period: 0,
                        date_id: 0,
                        start_lat: rx_lat + d_deg - lat_off,
                        start_lon: rx_lon + d_deg - lon_off,
                        start_alt_m: rep_alt_m,
                        end_lat: rx_lat + d_deg + lat_off,
                        end_lon: rx_lon + d_deg + lon_off,
                        end_alt_m: rep_alt_m,
                        speed_kt: 450.0,
                        segment_length_m: rep_len_m as f32,
                        count_weight: 1.0,
                        surface_model: false,
                        ground_context: crate::emission::aircraft::GROUND_CONTEXT_NONE,
                        ground_ops_kind: crate::emission::aircraft::GROUND_OPS_KIND_NONE,
                        source_id: 0,
                    };
                    // Terrain the gate knows nothing about — the guarantee
                    // must hold for any of these.
                    for &elev in &[0.0_f64, 2_000.0, 8_000.0] {
                        let terrain = SegmentTerrain {
                            start_elev: elev,
                            q1_elev: elev,
                            mid_elev: elev,
                            q3_elev: elev,
                            end_elev: elev,
                        };
                        let kept =
                            segment_sel_with_terrain(&seg, rx_lat, rx_lon, rx_elev, &terrain, npd)
                                .is_some();
                        let gate = within_kernel_reach(&seg, rx_lat, rx_lon, rx_elev);
                        checked += 1;
                        if !gate {
                            gate_false += 1;
                        }
                        if kept {
                            kernel_some += 1;
                        }
                        assert!(
                            gate || !kept,
                            "reach gate rejected a segment the kernel keeps: \
                             rep_len={rep_len_m} alt={rep_alt_m} profile={profile_idx} \
                             d_deg={d_deg} elev={elev}"
                        );
                    }
                }
            }
        }
    }
    // The gate has to actually bite, or the test proves nothing.
    assert!(checked > 1000, "grid too small: {checked}");
    assert!(
        gate_false > 100,
        "gate never fired ({gate_false}/{checked})"
    );
    assert!(
        kernel_some > 100,
        "kernel never kept anything ({kernel_some}/{checked})"
    );
}

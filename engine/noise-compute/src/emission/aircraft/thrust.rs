//! Thrust-dependent NPD power interpolation (Doc 29 4th ed.) and helicopter certification corrections.
//!
//! Fixed-wing departures no longer read the max-thrust NPD row. Each segment's
//! corrected net thrust per engine selects two bracketing power rows and an
//! Eq. 4-3 weight, computed once per segment (popup, CPU painter and CUDA pack
//! share this) and applied as two LUT reads plus a lerp inside the kernel.
//! Helicopters keep the MV-22 distance shape with an additive per-typecode,
//! per-state correction from EASA certification levels.

use crate::types::AircraftSegment;

use super::npd::is_helicopter_profile;
use crate::emission::thrust_generated::{HELI_CORRECTIONS, THRUST};

/// Power-row stride of the per-class thrust tables (generated paddings repeat
/// the loudest row, so a bracket never reads past the tabulated rows).
pub const MAX_POWER_ROWS: usize = 6;

/// Meters per foot.
const M_PER_FT: f64 = 0.3048;

/// Helicopter descent gate (m of segment altitude loss). ADS-B barometric
/// altitude quantizes to 25 ft (7.62 m): level flight flickers by at most one
/// step, so anything past −10 m is a real descent carrying BVI. Shallow
/// descents missed here keep the level curve; their BVI is physically small.
pub const HELI_DESCENT_SDZ_M: f64 = -10.0;

/// Per-noise-class thrust model (generated data, see `thrust_generated.rs`).
pub struct ThrustModel {
    pub class_name: &'static str,
    pub anchor_name: &'static str,
    pub has_thrust: bool,
    pub engines: u8,
    /// Median DEFAULT stage weight (lb): observed stage lengths are unknown.
    pub weight_lb: f64,
    /// Clean-configuration drag/lift ratio R (minimum-R departure flap).
    pub drag_ratio: f64,
    /// Cutback height (ft above field): initial climb flies MaxTakeoff below it.
    pub cutback_ft_afe: f64,
    /// B-1 coefficients (E, F, Ga, Gb, H) per rating.
    pub takeoff_coef: [f64; 5],
    pub climb_coef: [f64; 5],
    pub idle_coef: [f64; 5],
    pub dep_rows: u8,
    pub dep_power: [f64; MAX_POWER_ROWS],
    pub dep_sel: [[f64; 10]; MAX_POWER_ROWS],
    pub dep_lmax: [[f64; 10]; MAX_POWER_ROWS],
    pub app_rows: u8,
    pub app_power: [f64; MAX_POWER_ROWS],
    pub app_sel: [[f64; 10]; MAX_POWER_ROWS],
    pub app_lmax: [[f64; 10]; MAX_POWER_ROWS],
}

impl ThrustModel {
    /// Pinned class (fallback proxy, piston, turboprop % power, helicopter):
    /// no thrust tables; the kernel reads the anchor curve at row 0.
    pub const fn pinned(class_name: &'static str, anchor_name: &'static str) -> Self {
        ThrustModel {
            class_name,
            anchor_name,
            has_thrust: false,
            engines: 0,
            weight_lb: 0.0,
            drag_ratio: 0.0,
            cutback_ft_afe: 0.0,
            takeoff_coef: [0.0; 5],
            climb_coef: [0.0; 5],
            idle_coef: [0.0; 5],
            dep_rows: 1,
            dep_power: [0.0; MAX_POWER_ROWS],
            dep_sel: [[0.0; 10]; MAX_POWER_ROWS],
            dep_lmax: [[0.0; 10]; MAX_POWER_ROWS],
            app_rows: 1,
            app_power: [0.0; MAX_POWER_ROWS],
            app_sel: [[0.0; 10]; MAX_POWER_ROWS],
            app_lmax: [[0.0; 10]; MAX_POWER_ROWS],
        }
    }
}

/// Additive helicopter correction (dB) per flight state, indexed by profile.
pub struct HeliCorrection {
    pub level_db: f64,
    pub climb_db: f64,
    pub descent_db: f64,
}

impl HeliCorrection {
    pub const fn zero() -> Self {
        HeliCorrection {
            level_db: 0.0,
            climb_db: 0.0,
            descent_db: 0.0,
        }
    }
}

/// ISA pressure ratio δ at pressure altitude `h_ft`.
#[inline]
pub fn isa_delta(h_ft: f64) -> f64 {
    (1.0 - 6.8756e-6 * h_ft).powf(5.2559)
}

/// ISA density ratio σ at pressure altitude `h_ft`.
#[inline]
pub fn isa_sigma(h_ft: f64) -> f64 {
    (1.0 - 6.8756e-6 * h_ft).powf(4.2559)
}

/// ISA ambient temperature (°C) at pressure altitude `h_ft`.
#[inline]
pub fn isa_temp_c(h_ft: f64) -> f64 {
    15.0 - 1.98 * h_ft / 1000.0
}

/// Doc 29 Eq. B-1: corrected net thrust per engine (lb) at a thrust rating.
/// `vc_kt` is calibrated airspeed, `h_ft` pressure altitude, `t_c` ambient °C.
#[inline]
pub fn rated_thrust_lb(coef: &[f64; 5], vc_kt: f64, h_ft: f64, t_c: f64) -> f64 {
    coef[0] + coef[1] * vc_kt + coef[2] * h_ft + coef[3] * h_ft * h_ft + coef[4] * t_c
}

/// Doc 29 Eq. B-12 inverted (bank 0, no acceleration term): corrected thrust
/// per engine holding the observed climb gradient. `k` is 1.01 at Vc ≤ 200 kt,
/// else 0.95 (headwind + constant-CAS acceleration allowance).
#[inline]
pub fn force_balance_thrust_lb(model: &ThrustModel, sin_gamma: f64, k: f64, delta: f64) -> f64 {
    (model.weight_lb / delta) * (sin_gamma / k + model.drag_ratio) / f64::from(model.engines)
}

/// Segment state feeding [`power_bracket`]. All fields are receiver-independent
/// (stored columns and Filter-D cuts), so popup, CPU painter and CUDA pack
/// compute the identical bracket.
pub struct ThrustInput {
    pub is_departure: bool,
    pub on_ground: bool,
    pub speed_kt: f64,
    pub alt_m: f64,
    pub sin_gamma: f64,
    /// Height above the departure field (m): mean altitude minus the terrain
    /// under the flight's own takeoff roll. Falls back to local AGL when the
    /// roll was not observed (overflights, coverage gaps at the airport).
    pub height_above_field_m: f64,
}

/// Build the [`ThrustInput`] from a stored segment, its effective endpoint
/// altitudes (callers with altitude overrides pass the overridden values, so
/// thrust sees the same geometry as the kernel), its Filter-D terrain cuts
/// (`terrain_elev − 30`) and the departure field elevation (NaN when the
/// takeoff roll was not observed). Climb angle comes from the stored
/// horizontal length, never from a receiver projection, so the bracket is
/// bit-identical on every path.
pub fn thrust_input_for_segment(
    seg: &AircraftSegment,
    start_alt_m: f64,
    end_alt_m: f64,
    terrain_start_cut_m: f64,
    terrain_end_cut_m: f64,
    departure_field_elev_m: f64,
) -> ThrustInput {
    let sdz = end_alt_m - start_alt_m;
    let slen = f64::from(seg.segment_length_m);
    let len3d = slen.hypot(sdz);
    let alt_m = 0.5 * (start_alt_m + end_alt_m);
    // The Filter-D `elev − 30` cuts cancel, so this is real height above
    // local ground — the fallback when the departure field is unknown.
    let agl_m = 0.5 * (start_alt_m + end_alt_m - terrain_start_cut_m - terrain_end_cut_m) - 30.0;
    ThrustInput {
        is_departure: seg.is_departure,
        on_ground: seg.on_ground,
        speed_kt: f64::from(seg.speed_kt),
        alt_m,
        sin_gamma: if len3d > 1e-9 { sdz / len3d } else { 0.0 },
        height_above_field_m: if departure_field_elev_m.is_nan() {
            agl_m
        } else {
            alt_m - departure_field_elev_m
        },
    }
}

/// Eq. 4-3 power bracket `(row, w)` for one segment: interpolating the NPD rows
/// costs two LUT reads plus a lerp in the kernel. Rules: pinned classes stay
/// on today's curves; ground rolls use their rating (takeoff/idle); initial
/// climb below the cutback height above the field flies MaxTakeoff;
/// everything else follows force balance within [Idle, MaxClimb].
/// Out-of-table thrust clamps to the edge row.
pub fn power_bracket(model: &ThrustModel, input: &ThrustInput) -> (u8, f64) {
    let (powers, rows) = if input.is_departure {
        (&model.dep_power, model.dep_rows)
    } else {
        (&model.app_power, model.app_rows)
    };
    if !model.has_thrust {
        return (0, 0.0);
    }
    let h_ft = input.alt_m / M_PER_FT;
    let delta = isa_delta(h_ft);
    let vc_kt = input.speed_kt * isa_sigma(h_ft).sqrt();
    let temp_c = isa_temp_c(h_ft);
    let thrust_lb = if input.on_ground {
        // Takeoff roll at MaxTakeoff, landing roll and taxi at idle (thrust
        // reversers have no ANP model and stay unmodelled).
        rated_thrust_lb(
            if input.is_departure {
                &model.takeoff_coef
            } else {
                &model.idle_coef
            },
            vc_kt,
            h_ft,
            temp_c,
        )
    } else if input.is_departure && input.height_above_field_m < model.cutback_ft_afe * M_PER_FT {
        rated_thrust_lb(&model.takeoff_coef, vc_kt, h_ft, temp_c)
    } else {
        let k = if vc_kt <= 200.0 { 1.01 } else { 0.95 };
        force_balance_thrust_lb(model, input.sin_gamma, k, delta).clamp(
            rated_thrust_lb(&model.idle_coef, vc_kt, h_ft, temp_c),
            rated_thrust_lb(&model.climb_coef, vc_kt, h_ft, temp_c),
        )
    };
    bracket_power(powers, rows, thrust_lb)
}

/// Doc 29 Eq. 4-3 bracket of corrected thrust `p_lb` over `rows` tabulated
/// powers: the lower row index plus the interpolation weight toward the next
/// row. Out-of-table thrust clamps to the edge row with weight 0.
pub fn bracket_power(powers: &[f64; MAX_POWER_ROWS], rows: u8, p_lb: f64) -> (u8, f64) {
    let n = rows as usize;
    if p_lb <= powers[0] {
        return (0, 0.0);
    }
    if p_lb >= powers[n - 1] {
        return ((n - 1) as u8, 0.0);
    }
    for row in 0..n - 1 {
        if p_lb < powers[row + 1] {
            return (
                row as u8,
                (p_lb - powers[row]) / (powers[row + 1] - powers[row]),
            );
        }
    }
    ((n - 1) as u8, 0.0)
}

/// Helicopter certification correction (dB) for one segment: climbing rows
/// take the takeoff uplift, descending rows the BVI approach uplift, level
/// rows the bare level correction. Zero for every fixed-wing profile.
#[inline]
pub fn heli_correction_db(profile_idx: u8, is_departure: bool, sdz_m: f64) -> f64 {
    if !is_helicopter_profile(profile_idx) {
        return 0.0;
    }
    let correction = &HELI_CORRECTIONS[(profile_idx as usize).min(HELI_CORRECTIONS.len() - 1)];
    if is_departure {
        correction.climb_db
    } else if sdz_m < HELI_DESCENT_SDZ_M {
        correction.descent_db
    } else {
        correction.level_db
    }
}

/// Thrust model of a noise class (index into the generated table).
#[inline]
pub fn thrust_model_for_class(class_idx: usize) -> &'static ThrustModel {
    &THRUST[class_idx.min(THRUST.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::super::npd::{
        interpolate_sel, noise_class_of, profile_idx, CLASS_NAMES, CLASS_REP_PROFILE_IDX, PROFILES,
    };
    use super::*;

    #[test]
    fn b738_cutback_thrust_matches_the_doc29_pilot() {
        // ANP DEFAULT stage 1 step 6 (climb 2040 -> 3000 ft AFE), LKPR field
        // 355.5 m: MaxClimb at CAS 204.8 kt gives Fn/delta 18,093 lb, on the
        // 16,000/19,000 lb rows with w 0.6977 (SEL 93.77 dB at 1,000 ft).
        let model = thrust_model_for_class(noise_class_of(profile_idx("B738")) as usize);
        let h_ft = 3000.0 + 355.5 / M_PER_FT;
        let thrust = rated_thrust_lb(&model.climb_coef, 204.8, h_ft, 15.0);
        assert!((thrust - 18093.0).abs() < 0.5, "Fn/delta = {thrust}");
        let (row, w) = bracket_power(&model.dep_power, model.dep_rows, thrust);
        assert_eq!(row, 2);
        assert!((w - 0.6977).abs() < 0.0001, "w = {w}");
        // Clamp arms: at and past the table edges the weight is exactly 0.
        assert_eq!(
            bracket_power(&model.dep_power, model.dep_rows, 10_000.0),
            (0, 0.0)
        );
        assert_eq!(
            bracket_power(&model.dep_power, model.dep_rows, 100_000.0),
            (model.dep_rows - 1, 0.0)
        );
    }

    #[test]
    fn power_bracket_routes_ground_cutback_and_force_balance() {
        let model = thrust_model_for_class(noise_class_of(profile_idx("B738")) as usize);
        // Takeoff roll: MaxTakeoff rating, near the top departure row.
        let roll = ThrustInput {
            is_departure: true,
            on_ground: true,
            speed_kt: 160.0,
            alt_m: 355.5,
            sin_gamma: 0.0,
            height_above_field_m: 0.0,
        };
        let (row, _) = power_bracket(model, &roll);
        assert!(row >= model.dep_rows - 2, "roll row = {row}");
        // Initial climb below the 2,040 ft cutback: MaxTakeoff as well.
        let initial = ThrustInput {
            on_ground: false,
            speed_kt: 185.0,
            alt_m: 355.5 + 500.0 * M_PER_FT,
            sin_gamma: 0.12,
            height_above_field_m: 500.0 * M_PER_FT,
            ..roll
        };
        let (row, _) = power_bracket(model, &initial);
        assert!(row >= model.dep_rows - 2, "initial-climb row = {row}");
        // Level cruise at FL360: force balance reads a high departure row.
        // Corrected thrust is actual thrust divided by δ = 0.22, so the NPD
        // index stays loud even though the engines sip fuel — that 1/δ is
        // exactly why Doc 29 indexes NPDs by corrected, not actual, thrust.
        let cruise = ThrustInput {
            on_ground: false,
            speed_kt: 450.0,
            alt_m: 11000.0,
            sin_gamma: 0.0,
            height_above_field_m: 10500.0,
            ..roll
        };
        let (row, w) = power_bracket(model, &cruise);
        assert!(
            row >= 2 && (0.0..=1.0).contains(&w),
            "cruise = ({row}, {w})"
        );
        // Pinned classes never interpolate.
        let pinned = thrust_model_for_class(noise_class_of(profile_idx("DH8D")) as usize);
        assert_eq!(power_bracket(pinned, &cruise), (0, 0.0));
    }

    /// Cutback compares height above the field, not local AGL: a B738 7°
    /// climb 700 m above the aerodrome has passed the 622 m cutback even
    /// when the ground under the track sits 270 m above the runway (AGL
    /// 430 m) — while 500 m above the field over ground 300 m below it
    /// (AGL 800 m) still flies MaxTakeoff.
    #[test]
    fn cutback_compares_height_above_the_field() {
        let model = thrust_model_for_class(noise_class_of(profile_idx("B738")) as usize);
        let climb = |alt_m: f64, height_above_field_m: f64| ThrustInput {
            is_departure: true,
            on_ground: false,
            speed_kt: 185.0,
            alt_m,
            sin_gamma: 0.122,
            height_above_field_m,
        };
        let bracket = |(row, w): (u8, f64)| (row, (w * 10000.0).round() as i64);
        // Past cutback: force balance on the 13,000/16,000 lb rows.
        assert_eq!(bracket(power_bracket(model, &climb(700.0, 700.0))), (1, 5142));
        // Below cutback: MaxTakeoff rating on the 19,000/23,500 lb rows.
        assert_eq!(bracket(power_bracket(model, &climb(900.0, 500.0))), (3, 5239));
    }

    /// The cutback height is altitude minus the departure field, falling back
    /// to local AGL (Filter-D cuts cancelled) when the takeoff roll was not
    /// observed.
    #[test]
    fn cutback_height_falls_back_to_agl_without_a_takeoff_roll() {
        let seg = crate::types::AircraftSegment {
            flight_id: 1,
            profile_idx: 0,
            is_departure: true,
            on_ground: false,
            period: 0,
            date_id: 0,
            start_lat: 50.0,
            start_lon: 14.0,
            start_alt_m: 900.0,
            end_lat: 50.01,
            end_lon: 14.01,
            end_alt_m: 900.0,
            speed_kt: 185.0,
            segment_length_m: 1000.0,
            departure_field_elev_m: 400.0,
            count_weight: 1.0,
            surface_model: false,
            ground_context: 0,
            ground_ops_kind: 0,
            source_id: 1,
        };
        let known = thrust_input_for_segment(&seg, 900.0, 900.0, 70.0, 70.0, 400.0);
        assert_eq!(known.height_above_field_m, 500.0);
        let unknown = thrust_input_for_segment(&seg, 900.0, 900.0, 70.0, 70.0, f64::NAN);
        assert_eq!(unknown.height_above_field_m, 800.0);
    }

    #[test]
    fn generated_tables_follow_class_names_and_anchors() {
        assert_eq!(THRUST.len(), CLASS_NAMES.len());
        for (class_idx, model) in THRUST.iter().enumerate() {
            assert_eq!(model.class_name, CLASS_NAMES[class_idx]);
            let anchor = &PROFILES[CLASS_REP_PROFILE_IDX[class_idx] as usize];
            assert_eq!(model.anchor_name, anchor.name, "class {class_idx}");
            if model.has_thrust {
                assert!(model.dep_rows >= 2 && model.app_rows >= 2);
                assert!(model.engines >= 1 && model.weight_lb > 0.0);
            }
        }
    }

    #[test]
    fn heli_corrections_cover_exactly_the_helicopter_profiles() {
        for (idx, c) in HELI_CORRECTIONS.iter().enumerate() {
            let nonzero = c.level_db != 0.0 || c.climb_db != 0.0 || c.descent_db != 0.0;
            assert_eq!(nonzero, is_helicopter_profile(idx as u8), "profile {idx}");
        }
        // Spot values from EASA Issue 52 (EC35 light, B412 heavy).
        let ec35 = &HELI_CORRECTIONS[profile_idx("EC35") as usize];
        assert!(ec35.level_db < -10.0 && ec35.descent_db > ec35.level_db);
        let b412 = &HELI_CORRECTIONS[profile_idx("B412") as usize];
        assert!(b412.level_db < -3.0 && b412.level_db > ec35.level_db);
        assert_eq!(heli_correction_db(profile_idx("B738"), false, -100.0), 0.0);
        assert_eq!(
            heli_correction_db(profile_idx("EC35"), true, 0.0),
            HELI_CORRECTIONS[profile_idx("EC35") as usize].climb_db
        );
        assert_eq!(
            heli_correction_db(profile_idx("EC35"), false, -11.0),
            HELI_CORRECTIONS[profile_idx("EC35") as usize].descent_db
        );
        assert_eq!(
            heli_correction_db(profile_idx("EC35"), false, -7.62),
            HELI_CORRECTIONS[profile_idx("EC35") as usize].level_db
        );
    }

    #[test]
    fn heli_corrections_reproduce_easa_sel_at_150m() {
        // Certified SEL at 150 m (EASA Issue 52, representative records):
        // (typecode, level, climb uplift, descent uplift).
        for (typecode, sel150, climb_up, descent_up) in [
            ("EC35", 80.6, 3.4, 8.7),  // light, Ch11 energy mean, n 7
            ("AS50", 84.6, 2.3, 3.9),  // light, Ch11 energy mean, n 6
            ("B412", 90.7, -0.6, 2.2), // heavy, Ch8 overflight mean − D, n 30
        ] {
            let anchor = &PROFILES[profile_idx("AS50") as usize];
            let model_level = interpolate_sel(anchor, 150.0 * 3.28084, false);
            let model_climb = interpolate_sel(anchor, 150.0 * 3.28084, true);
            let c = &HELI_CORRECTIONS[profile_idx(typecode) as usize];
            assert!(
                (model_level + c.level_db - sel150).abs() < 0.15,
                "{typecode} level: {}",
                model_level + c.level_db
            );
            assert!(
                (model_climb + c.climb_db - (sel150 + climb_up)).abs() < 0.15,
                "{typecode} climb: {}",
                model_climb + c.climb_db
            );
            assert!(
                (model_level + c.descent_db - (sel150 + descent_up)).abs() < 0.15,
                "{typecode} descent: {}",
                model_level + c.descent_db
            );
        }
    }
}

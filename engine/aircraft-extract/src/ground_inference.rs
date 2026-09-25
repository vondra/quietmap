//! Composite ground-flag inference. readsb reports on-ground only as the
//! altitude string `"ground"`; aircraft whose transponder never switches
//! to surface reports still taxi with a numeric altitude.
//!
//! The composite layered inference (all altitude thresholds are AGL in
//! metres, computed via DEM by Stage 1):
//!   1. A surface report (`alt = "ground"`) is ground.
//!   2. Edge-window scan recovers ground prefixes / suffixes without
//!      surface reports that match surface signatures (low AGL + low
//!      speed + low baro_rate) for ≥ 3 consecutive points.
//!   3. The edge scan terminates on the first strongly-airborne sample
//!      (AGL ≥ 165 ft OR speed ≥ 130 kt) so cruise points never get
//!      flipped to ground.
//!
//! AGL semantics matter at high-elevation airports: La Paz (13 325 ft
//! MSL elevation), Lhasa, Cusco, Quito etc. sit far above 600 ft MSL.
//! Using absolute MSL would reject every legitimate ground sample
//! there. Stage 1 (`stage_1::stage_1_one_flight`) computes per-point
//! AGL from Copernicus DEM and passes it through `ground_flags`.

use crate::trace::TracePoint;

const SURFACE_EDGE_WINDOW_POINTS: usize = 32;
// 1 ft = 0.3048 m exactly. The acoustic thresholds are documented in
// feet to keep the Doc 29 reasoning legible; the runtime values stay
// in metres to match Stage 1's `agl_m` source of truth.
//
// Surface inference is paranoid at 30 ft (Doc 29 §B-7 departure-segment
// start altitude) so a B738 on final at 30-150 m AGL cannot seed fake
// ground-ops clusters along the ILS corridor, and edge-airborne
// termination sits at 165 ft (~50 m, ICAO Annex 14 obstacle limitation
// surface at the runway-end gate) so a slow climb transitions out of
// ground inference within ~16 s of liftoff.
const SURFACE_MAX_AGL_M: f32 = 30.0 * 0.3048; // ≈ 9.14 m
const SURFACE_MAX_SPEED_KT: f32 = 90.0;
const SURFACE_MAX_BARO_RATE_FPM: f32 = 1200.0;
const SURFACE_MIN_INFERRED_POINTS: usize = 3;
const SURFACE_EDGE_STRONG_AIRBORNE_AGL_M: f32 = 165.0 * 0.3048; // ≈ 50.29 m
const SURFACE_EDGE_STRONG_AIRBORNE_SPEED_KT: f32 = 130.0;
const SURFACE_LOCAL_WINDOW: usize = 2;

/// Per-point composite ground flag. Length matches `points`.
pub fn ground_flags(points: &[TracePoint], agl_m: &[f32]) -> Vec<bool> {
    debug_assert_eq!(points.len(), agl_m.len());
    let mut flags: Vec<bool> = points
        .iter()
        .map(|point| point.alt_is_ground())
        .collect();
    if points.len() < 2 {
        return flags;
    }
    infer_edge_ground(points, agl_m, &mut flags, true);
    infer_edge_ground(points, agl_m, &mut flags, false);
    flags
}

/// Surface signature without a surface report — low AGL +
/// slow + flat. Used both for the early-reject path inside
/// [`is_surface_candidate`] and for the per-neighbour evidence the
/// edge scan counts; the two were inlined in the previous revision
/// and drifted easily out of sync.
fn is_surface_signature(pt: &TracePoint, agl_m: f32) -> bool {
    agl_m <= SURFACE_MAX_AGL_M
        && pt.speed_kt <= SURFACE_MAX_SPEED_KT
        && pt.baro_rate_fpm.abs() <= SURFACE_MAX_BARO_RATE_FPM
}

fn infer_edge_ground(points: &[TracePoint], agl_m: &[f32], flags: &mut [bool], is_prefix: bool) {
    let n = points.len();
    let edge_len = n.min(SURFACE_EDGE_WINDOW_POINTS);
    if edge_len == 0 {
        return;
    }
    let has_surface_report = if is_prefix {
        flags[..edge_len].iter().any(|g| *g)
    } else {
        flags[n - edge_len..].iter().any(|g| *g)
    };

    let mut inferred = Vec::new();
    let mut seen_candidate = false;
    let mut misses_after_candidate = 0usize;
    let indices: Vec<usize> = if is_prefix {
        (0..edge_len).collect()
    } else {
        (n - edge_len..n).rev().collect()
    };
    for idx in indices {
        let candidate = flags[idx] || is_surface_candidate(points, agl_m, idx);
        if candidate {
            seen_candidate = true;
            misses_after_candidate = 0;
            if !flags[idx] {
                inferred.push(idx);
            }
            continue;
        }
        if seen_candidate {
            misses_after_candidate += 1;
            if misses_after_candidate >= 2 {
                break;
            }
            continue;
        }
        if agl_m[idx] >= SURFACE_EDGE_STRONG_AIRBORNE_AGL_M
            || points[idx].speed_kt >= SURFACE_EDGE_STRONG_AIRBORNE_SPEED_KT
        {
            break;
        }
    }
    if has_surface_report || inferred.len() >= SURFACE_MIN_INFERRED_POINTS {
        for idx in inferred {
            flags[idx] = true;
        }
    }
}

fn is_surface_candidate(points: &[TracePoint], agl_m: &[f32], idx: usize) -> bool {
    if points[idx].alt_is_ground() {
        return true;
    }
    if !is_surface_signature(&points[idx], agl_m[idx]) {
        return false;
    }
    // Indexed walk so each neighbour's AGL stays paired with its
    // TracePoint — a closure over `points` alone would lose the
    // parallel slice's index.
    let lo = idx.saturating_sub(SURFACE_LOCAL_WINDOW);
    let hi = (idx + SURFACE_LOCAL_WINDOW + 1).min(points.len());
    let mut local_matches = 0usize;
    for j in lo..hi {
        if points[j].alt_is_ground() || is_surface_signature(&points[j], agl_m[j]) {
            local_matches += 1;
        }
    }
    local_matches >= 3
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::FLAG_ALT_IS_GROUND;

    const FT_TO_M: f32 = 0.3048;

    /// `surface_report` mirrors readsb's `alt = "ground"` (NaN altitude).
    fn pt(alt_ft: f32, speed_kt: f32, baro: f32, surface_report: bool) -> TracePoint {
        TracePoint {
            timestamp: 0.0,
            lat: 50.0,
            lon: 14.0,
            alt_ft: if surface_report { f32::NAN } else { alt_ft },
            speed_kt,
            track_deg: 90.0,
            baro_rate_fpm: baro,
            flags: if surface_report { FLAG_ALT_IS_GROUND } else { 0 },
        }
    }

    /// Helper for `ground_flags` fixtures: AGL in metres for an
    /// `alt_ft` sample over terrain of `terrain_msl_ft`.
    fn agl_m_vec(points: &[TracePoint], terrain_msl_ft: f32) -> Vec<f32> {
        points
            .iter()
            .map(|p| (p.alt_ft - terrain_msl_ft) * FT_TO_M)
            .collect()
    }

    #[test]
    fn edge_window_recovers_ground_prefix() {
        // Post-Phase-7c tightening: SURFACE_MAX_AGL_M = 30 ft (~9 m),
        // so points at 50 / 100 ft AGL no longer qualify as surface
        // signature. Only the first two (0 ft AGL) flip to ground.
        // That's the correct behaviour — a 50 ft AGL frame is a
        // climb-out sample, not a runway sample.
        let points = vec![
            pt(0.0, 8.0, 0.0, false),
            pt(0.0, 10.0, 0.0, false),
            pt(25.0, 12.0, 0.0, false), // 7.6 m AGL — still ground
            pt(50.0, 18.0, 0.0, false), // 15.2 m AGL — now airborne
            pt(800.0, 200.0, 0.0, false),
            pt(2_000.0, 250.0, 0.0, false),
            pt(4_000.0, 280.0, 0.0, false),
            pt(8_000.0, 320.0, 0.0, false),
        ];
        let agl_m = agl_m_vec(&points, 0.0);
        let flags = ground_flags(&points, &agl_m);
        assert!(flags[0] && flags[1] && flags[2]);
        assert!(!flags[3] && !flags[4]);
    }

    #[test]
    fn edge_window_la_paz_prefix() {
        // Same as above but over La Paz terrain (13_000 ft MSL).
        // Pre-fix MSL-absolute thresholds would have rejected every
        // sample (alt > 500 ft MSL); AGL semantics correctly flip
        // the first three.
        let points = vec![
            pt(13_000.0, 8.0, 0.0, false),
            pt(13_010.0, 10.0, 0.0, false),
            pt(13_025.0, 12.0, 0.0, false), // 7.6 m AGL — ground
            pt(13_050.0, 18.0, 0.0, false), // 15.2 m AGL — airborne
            pt(13_800.0, 200.0, 0.0, false),
            pt(15_000.0, 250.0, 0.0, false),
            pt(17_000.0, 280.0, 0.0, false),
            pt(21_000.0, 320.0, 0.0, false),
        ];
        let agl_m = agl_m_vec(&points, 13_000.0);
        let flags = ground_flags(&points, &agl_m);
        assert!(flags[0] && flags[1] && flags[2]);
        assert!(!flags[3] && !flags[4]);
    }

    #[test]
    fn cruise_low_speed_burst_does_not_get_flipped_to_ground() {
        // Cruise sample with momentary speed dip — must stay airborne.
        let points = vec![
            pt(35_000.0, 450.0, 0.0, false),
            pt(35_000.0, 80.0, 0.0, false), // glitchy speed; not ground
            pt(35_000.0, 450.0, 0.0, false),
        ];
        let agl_m = agl_m_vec(&points, 0.0);
        let flags = ground_flags(&points, &agl_m);
        assert!(!flags[0] && !flags[1] && !flags[2]);
    }

    #[test]
    fn slow_climb_after_takeoff_at_45m_agl_is_airborne() {
        // The 165 ft (~50 m) edge-airborne gate must transition a
        // slow climb out of ground inference promptly — a 500 ft
        // gate would leave a slow GA climb flagged "ground" for
        // ~30 s after rotation.
        let points = vec![
            pt(0.0, 8.0, 0.0, true),         // taxi, surface report → ground
            pt(0.0, 100.0, 200.0, true),     // accelerating
            pt(50.0, 130.0, 1500.0, false),  // rotation, 15 m AGL — still in inference window
            pt(150.0, 140.0, 1800.0, false), // 45 m AGL, 140 kt climbing — must NOT be ground
            pt(400.0, 150.0, 1900.0, false), // 122 m AGL — clearly airborne
            pt(800.0, 160.0, 1900.0, false),
        ];
        let agl_m = agl_m_vec(&points, 0.0);
        let flags = ground_flags(&points, &agl_m);
        assert!(flags[0], "taxi with a surface report");
        // 45 m AGL climb: surface signature fails (> 9 m), and
        // edge-strong-airborne (50 m) kicks in just above this,
        // terminating the inference window before this point.
        assert!(
            !flags[3],
            "45 m AGL climb must be airborne, got flags={flags:?}"
        );
        assert!(!flags[4] && !flags[5]);
    }
}

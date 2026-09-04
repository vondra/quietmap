//! Composite ground-flag inference. The raw ADS-B `on_ground` bit is
//! unreliable: jets at FL350 sometimes still report it, taxiing aircraft
//! sometimes don't. Trusting the raw bit produced 99 dB ground-source
//! ghosts at FL350 in v5.
//!
//! The composite layered inference (all altitude thresholds are AGL in
//! metres, computed via DEM by Stage 1):
//!   1. Rate-limits the raw bit (AGL ≤ 80 ft, speed ≤ 140 kt,
//!      |baro_rate| ≤ 2 000 fpm) — otherwise reject as bogus.
//!   2. Edge-window scan recovers ground prefixes / suffixes that have
//!      NO raw bit set but match surface signatures (low AGL + low
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
// The gates are asymmetric so a B738 on final at 30-150 m AGL with
// the on_ground bit flipped early (transponder flip before flare)
// cannot seed fake ground-ops clusters along the ILS approach
// corridor: raw transponder is trusted ONLY up to 80 ft AGL (covers
// DEM/baro uncertainty at landfill airports per `filters.rs:23-29`),
// surface inference is paranoid at 30 ft (Doc 29 §B-7
// departure-segment start altitude), and edge-airborne termination
// drops to 165 ft (~50 m, ICAO Annex 14 obstacle limitation surface
// at runway-end gate) so a slow climb transitions out of ground
// inference within ~16 s of liftoff.
const SURFACE_MAX_AGL_M: f32 = 30.0 * 0.3048; // ≈ 9.14 m
const SURFACE_MAX_SPEED_KT: f32 = 90.0;
const SURFACE_MAX_BARO_RATE_FPM: f32 = 1200.0;
const SURFACE_MIN_INFERRED_POINTS: usize = 3;
const SURFACE_EDGE_STRONG_AIRBORNE_AGL_M: f32 = 165.0 * 0.3048; // ≈ 50.29 m
const SURFACE_EDGE_STRONG_AIRBORNE_SPEED_KT: f32 = 130.0;
const SURFACE_LOCAL_WINDOW: usize = 2;
const RAW_GROUND_FLAG_MAX_AGL_M: f32 = 80.0 * 0.3048; // ≈ 24.38 m
const RAW_GROUND_FLAG_MAX_SPEED_KT: f32 = 140.0;
const RAW_GROUND_FLAG_MAX_BARO_RATE_FPM: f32 = 2000.0;

/// Per-point composite ground flag. Length matches `points`.
pub fn ground_flags(points: &[TracePoint], agl_m: &[f32]) -> Vec<bool> {
    debug_assert_eq!(points.len(), agl_m.len());
    let mut flags: Vec<bool> = (0..points.len())
        .map(|i| raw_ground_signal(&points[i], agl_m[i]))
        .collect();
    if points.len() < 2 {
        return flags;
    }
    infer_edge_ground(points, agl_m, &mut flags, true);
    infer_edge_ground(points, agl_m, &mut flags, false);
    flags
}

/// Raw on-ground signal: the surface-position sentinel `alt = "ground"`
/// always wins (the aircraft itself has reported WoW directly); the
/// bitfield bit is gated against AGL / speed / baro_rate so a jet at
/// FL350 still asserting `on_ground` doesn't get treated as one.
pub fn raw_ground_signal(pt: &TracePoint, agl_m: f32) -> bool {
    if pt.alt_is_ground() {
        return true;
    }
    raw_ground_motion(pt) && agl_m <= RAW_GROUND_FLAG_MAX_AGL_M
}

/// Stage 0 has no DEM, but a cruise-speed raw bit cannot establish a turnaround.
pub(crate) fn raw_ground_motion(pt: &TracePoint) -> bool {
    pt.alt_is_ground()
        || (pt.on_ground_raw()
            && pt.speed_kt <= RAW_GROUND_FLAG_MAX_SPEED_KT
            && pt.baro_rate_fpm.abs() <= RAW_GROUND_FLAG_MAX_BARO_RATE_FPM)
}

/// Surface signature without consulting the raw bit — low AGL +
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
    let has_raw_ground = if is_prefix {
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
    if has_raw_ground || inferred.len() >= SURFACE_MIN_INFERRED_POINTS {
        for idx in inferred {
            flags[idx] = true;
        }
    }
}

fn is_surface_candidate(points: &[TracePoint], agl_m: &[f32], idx: usize) -> bool {
    if raw_ground_signal(&points[idx], agl_m[idx]) {
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
        if raw_ground_signal(&points[j], agl_m[j]) || is_surface_signature(&points[j], agl_m[j]) {
            local_matches += 1;
        }
    }
    local_matches >= 3
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::FLAG_ON_GROUND_RAW;

    const FT_TO_M: f32 = 0.3048;

    fn pt(alt_ft: f32, speed_kt: f32, baro: f32, raw_ground: bool) -> TracePoint {
        TracePoint {
            timestamp: 0.0,
            lat: 50.0,
            lon: 14.0,
            alt_ft,
            speed_kt,
            track_deg: 90.0,
            baro_rate_fpm: baro,
            flags: if raw_ground { FLAG_ON_GROUND_RAW } else { 0 },
        }
    }

    #[test]
    fn raw_signal_rejects_high_alt_jet_with_ground_bit() {
        // Jet at FL350 spuriously asserting on_ground — must NOT be treated as ground.
        let p = pt(35_000.0, 450.0, 0.0, true);
        assert!(!raw_ground_signal(&p, 35_000.0 * FT_TO_M));
    }

    #[test]
    fn raw_signal_accepts_taxi() {
        let p = pt(0.0, 15.0, 0.0, true);
        assert!(raw_ground_signal(&p, 0.0));
    }

    #[test]
    fn raw_signal_la_paz_high_elevation_taxi() {
        // SLLP La Paz (13_325 ft MSL elev). Aircraft on runway: alt_baro
        // ≈ 13_325 ft, AGL ≈ 0. Pre-fix MSL gate would reject (13_325 >
        // 600); AGL gate accepts.
        let p = pt(13_325.0, 15.0, 0.0, true);
        assert!(raw_ground_signal(&p, 0.0));
    }

    #[test]
    fn raw_signal_high_agl_cruise_rejected_despite_bogus_bit() {
        // Light GA cruise at FL080 over flat terrain (DEM ≈ 0).
        // AGL ≈ 8_000 ft → must reject the bogus on_ground bit.
        let p = pt(8_000.0, 130.0, 0.0, true);
        assert!(!raw_ground_signal(&p, 8_000.0 * FT_TO_M));
    }

    #[test]
    fn raw_signal_sea_level_taxi_still_works() {
        // Regression: LKPR (1247 ft MSL) taxi — alt_baro 1247 ft, AGL 0.
        let p = pt(1_247.0, 15.0, 0.0, true);
        assert!(raw_ground_signal(&p, 0.0));
    }

    /// Helper for `ground_flags` fixtures: AGL in metres for an
    /// `alt_ft` sample over terrain of `terrain_msl_ft`. Tests at
    /// sea-level use `terrain_msl_ft = 0` to reproduce the pre-fix
    /// MSL semantics on legacy fixtures.
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
    fn taxi_at_high_dem_bias_stays_ground() {
        // At a landfill airport with 8 m DEM bias the computed AGL
        // for a real taxi sample can run > 9 m even though the wheels
        // are on the tarmac. The raw transponder bit (24 m gate)
        // absorbs this.
        let p = pt(1_247.0 + 26.0, 15.0, 0.0, true); // alt=1273ft, terrain msl=1247ft → AGL ≈ 7.9m
        let agl = (26.0_f32) * FT_TO_M;
        // 26 ft = 7.93 m, under both surface (9 m) and raw (24 m) gates.
        assert!(raw_ground_signal(&p, agl));
        // Even at 20 m AGL (DEM way off), raw gate still accepts.
        assert!(raw_ground_signal(
            &pt(1_247.0 + 66.0, 15.0, 0.0, true),
            20.0
        ));
    }

    #[test]
    fn final_approach_30m_agl_stays_airborne() {
        // A B738 on final at 30 m AGL with the on_ground bit
        // prematurely set (transponder flip before flare) must NOT
        // be classified ground. The raw gate (24 m) rejects on AGL
        // grounds; even if a hypothetical implementation relaxed it,
        // the 140 kt speed gate would also reject.
        let p = pt(100.0, 145.0, -800.0, true); // ~30 m AGL, 145 kt, descending
        assert!(!raw_ground_signal(&p, 30.0));
    }

    #[test]
    fn flare_at_5m_agl_taxi_speed_is_ground() {
        // Just before touchdown, AGL 5 m, speed 20 kt, raw bit set
        // — still inside both gates.
        let p = pt(16.0, 20.0, 0.0, true);
        assert!(raw_ground_signal(&p, 5.0));
    }

    #[test]
    fn slow_climb_after_takeoff_at_45m_agl_is_airborne() {
        // The 165 ft (~50 m) edge-airborne gate must transition a
        // slow climb out of ground inference promptly — a 500 ft
        // gate would leave a slow GA climb flagged "ground" for
        // ~30 s after rotation.
        let points = vec![
            pt(0.0, 8.0, 0.0, true),         // taxi, raw bit set → ground
            pt(0.0, 100.0, 200.0, true),     // accelerating
            pt(50.0, 130.0, 1500.0, false),  // rotation, 15 m AGL — still in inference window
            pt(150.0, 140.0, 1800.0, false), // 45 m AGL, 140 kt climbing — must NOT be ground
            pt(400.0, 150.0, 1900.0, false), // 122 m AGL — clearly airborne
            pt(800.0, 160.0, 1900.0, false),
        ];
        let agl_m = agl_m_vec(&points, 0.0);
        let flags = ground_flags(&points, &agl_m);
        assert!(flags[0], "taxi at 0 ft with raw bit");
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

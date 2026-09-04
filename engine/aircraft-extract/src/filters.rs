//! Receiver-INDEPENDENT filters — the single source of truth for which
//! ADS-B points and segments contribute to the v6 popup arrows.
//!
//! Filter D (per-receiver terrain extrapolation invalidation, see
//! `noise-compute::emission::aircraft::{...}`) is *receiver-dependent*
//! and stays in the popup kernel. Everything here can be applied at
//! extraction time so the popup hot path doesn't repeat the work.
//!
//! Layered pipeline:
//!   * [`point_is_sane`] — per-point structural sanity (Stage 0).
//!   * [`validate_flight_trajectory`] — trajectory-aware truncation that
//!     drops the bogus tail of a widebody descending into terrain
//!     instead of dropping segments one-by-one (Stage 1, post-DEM AGL).
//!   * [`segment_is_keepable`] — per-segment sanity (Stage 1, post-segmentation).
//!
//! Anonymous ICAOs (`0xFFFFFF` reserved or zero) are NOT dropped —
//! Stage 0 instead packs a synthetic `flight_id` via
//! [`noise_compute::flight_id::pack_synth`] keyed on the typecode and
//! first-point seed, so anonymous traffic still contributes noise.

use crate::trace::TracePoint;

/// 4× the combined error envelope of barometric altitude (~50 m) +
/// Copernicus DEM (~30 m) + transient pressure offsets (~50 m). Below
/// this we treat the whole tail as fabricated rather than try to rescue
/// individual points — the popup's previous filter D dropped only the
/// underground sub-segment and let the bogus low-AGL approach segments
/// preceding it leak ~65–75 dB of false noise.
pub const HARD_AGL_FLOOR_M: f32 = -300.0;

/// Lossy de-icing / pull-up regimes hit ~6 000 fpm. Sustained anomalies
/// at 8 000 fpm are not real flight; they are the receiver tracking
/// secondary radar returns into the ground after the aircraft is gone.
pub const ANOMALY_DESCENT_RATE_FPM: f32 = 8_000.0;

/// Number of consecutive samples meeting [`ANOMALY_DESCENT_RATE_FPM`]
/// before the trajectory is treated as compromised. Three samples at
/// adsb.lol's median 5 s cadence is ~15 s of sustained descent.
pub const ANOMALY_SUSTAINED_SAMPLES: usize = 3;

/// Minimum credible segment length. Anything shorter is a taxi remnant
/// — keeping it skews the rep_line for ground-ops sub-buckets.
pub const MIN_SEGMENT_LENGTH_M: f32 = 10.0;

/// Below this airspeed a fixed-wing jet is not flying — drop the
/// segment so the airborne energy isn't anchored on a stalled trace.
pub const JET_STALL_SPEED_KT: f32 = 80.0;

/// Maximum credible barometric altitude in feet above MSL. Concorde's
/// service ceiling was 60 000 ft; this is +10 kft of headroom for
/// extreme high-altitude ferry. Beyond is corrupt data.
pub const MAX_PLAUSIBLE_ALT_FT: f32 = 70_000.0;

/// Minimum credible barometric altitude in feet. Subsea-level
/// aerodromes (Bet She'an, Schiphol) report negative; -2000 covers them
/// with margin against pressure-offset glitches.
pub const MIN_PLAUSIBLE_ALT_FT: f32 = -2_000.0;

/// Sustained physical maximum speed for any aircraft (Mach 3 at FL600
/// ≈ 1700 kt). Above is wildly bad data.
pub const MAX_PLAUSIBLE_SPEED_KT: f32 = 1_500.0;

/// m/s → kt, exact by definition (1 nm = 1852 m).
pub const MPS_TO_KT: f32 = 3600.0 / 1852.0;

/// Helicopter AGL ceiling — anything claiming helicopter @ ≥5 km AGL
/// is ADS-B mode-S decode error or military spoof. Civil helicopter
/// service ceilings: EC135/AS350 ≤ 6 km MSL, R22/R44 ≤ 4.3 km MSL;
/// mountain rescue tops at ~4 km AGL. AGL is DEM-relative (Stage 1
/// per-point sample), so a helicopter at FL130 over 3 km terrain ≈
/// 1 km AGL and passes; only flat-terrain ADS-B garbage is rejected.
pub const HELICOPTER_AGL_CEIL_M: f32 = 5_000.0;

/// Per-point sanity. Drops NaN/out-of-range coords, the (0,0) "no-fix"
/// sentinel, implausible airborne altitudes, and impossibly fast
/// vehicles. Ground-flagged rows are exempt from altitude checks — the
/// canonical "is on the surface" signal is the flag, and `alt_ft` carries
/// no information for those points (it's NaN by parser contract).
#[inline]
pub fn point_is_sane(pt: &TracePoint) -> bool {
    if !pt.timestamp.is_finite() || !(0.0..=u32::MAX as f64).contains(&pt.timestamp) {
        return false;
    }
    if !pt.lat.is_finite() || !pt.lon.is_finite() {
        return false;
    }
    if pt.lat.abs() > 90.0 || pt.lon.abs() > 180.0 {
        return false;
    }
    if pt.lat == 0.0 && pt.lon == 0.0 {
        return false;
    }
    if let Some(alt_ft) = pt.airborne_alt_ft() {
        if !alt_ft.is_finite() || !(MIN_PLAUSIBLE_ALT_FT..=MAX_PLAUSIBLE_ALT_FT).contains(&alt_ft) {
            return false;
        }
    }
    if !pt.speed_kt.is_finite()
        || !(0.0..=MAX_PLAUSIBLE_SPEED_KT).contains(&pt.speed_kt)
        || !pt.track_deg.is_finite()
        || !pt.baro_rate_fpm.is_finite()
    {
        return false;
    }
    true
}

/// Truncate `points` (and the parallel `agl_m` view) at the first point
/// where the trajectory becomes implausible — either the AGL drops
/// below [`HARD_AGL_FLOOR_M`] or a sustained-descent run of
/// [`ANOMALY_SUSTAINED_SAMPLES`] points exceeds
/// [`ANOMALY_DESCENT_RATE_FPM`]. After truncation, also drops
/// teleport-style outliers (>10 000 ft jump or >1 500 kt apparent speed
/// over <10 s).
///
/// Trajectory-level fix per plan: a per-segment filter would drop only
/// the underground sub-segment, leaving the fabricated approach
/// segments before it. Truncating the whole tail eliminates the leak.
pub fn validate_flight_trajectory(
    points: &mut Vec<TracePoint>,
    agl_m: &mut Vec<f32>,
    elev_m: &mut Vec<f32>,
) {
    debug_assert_eq!(points.len(), agl_m.len());
    debug_assert_eq!(points.len(), elev_m.len());
    let underground = agl_m.iter().position(|&a| a < HARD_AGL_FLOOR_M);
    let descent = scan_for_sustained_descent(points);
    let cut = match (underground, descent) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    if let Some(idx) = cut {
        let keep = backtrack_to_last_credible(points, agl_m, idx);
        points.truncate(keep);
        agl_m.truncate(keep);
        elev_m.truncate(keep);
    }
    drop_teleport_points(points, agl_m, elev_m);
}

fn scan_for_sustained_descent(points: &[TracePoint]) -> Option<usize> {
    if points.len() < ANOMALY_SUSTAINED_SAMPLES + 1 {
        return None;
    }
    let mut run = 0usize;
    let mut start = 0usize;
    for (i, pt) in points.iter().enumerate() {
        // baro_rate_fpm is signed: negative = descending.
        if pt.baro_rate_fpm < -ANOMALY_DESCENT_RATE_FPM {
            if run == 0 {
                start = i;
            }
            run += 1;
            if run >= ANOMALY_SUSTAINED_SAMPLES {
                return Some(start);
            }
        } else {
            run = 0;
        }
    }
    None
}

fn backtrack_to_last_credible(points: &[TracePoint], agl_m: &[f32], anomaly_idx: usize) -> usize {
    // Walk back to the last point whose AGL is comfortably above the
    // floor (>= 0 m) — the descent that LED to the anomaly may have
    // been fabricated for several samples before crossing the floor.
    let mut idx = anomaly_idx;
    while idx > 0 && agl_m[idx - 1] < 0.0 {
        idx -= 1;
    }
    idx.min(points.len())
}

fn drop_teleport_points(points: &mut Vec<TracePoint>, agl_m: &mut Vec<f32>, elev_m: &mut Vec<f32>) {
    if points.len() < 2 {
        return;
    }
    let mut keep_idx = vec![true; points.len()];
    let mut previous = 0;
    for i in 1..points.len() {
        let dt = (points[i].timestamp - points[previous].timestamp).abs() as f32;
        if dt > 0.0 && dt < 10.0 {
            // Altitude jump check skips ground/airborne boundaries — NaN
            // alt_ft would defeat `d_alt > 10000` (NaN > x is false).
            let alt_jump = match (
                points[previous].airborne_alt_ft(),
                points[i].airborne_alt_ft(),
            ) {
                (Some(a), Some(b)) => (b - a).abs() > 10_000.0,
                _ => false,
            };
            // Implied horizontal speed via the local-flat formula —
            // we only need a coarse upper bound to flag teleports.
            let dx_deg = grid::geo::wrapped_longitude_delta(
                points[i].lon as f64,
                points[previous].lon as f64,
            ) as f32;
            let dy_deg = points[i].lat - points[previous].lat;
            let cos_lat = ((points[i].lat as f64).to_radians().cos()) as f32;
            let dx_m = dx_deg * 111_320.0 * cos_lat;
            let dy_m = dy_deg * 110_540.0;
            let dist_m = (dx_m * dx_m + dy_m * dy_m).sqrt();
            let kt = dist_m / dt * MPS_TO_KT;
            if alt_jump || kt > MAX_PLAUSIBLE_SPEED_KT {
                keep_idx[i] = false;
            }
        }
        if keep_idx[i] {
            previous = i;
        }
    }
    let mut j = 0;
    for (i, &keep) in keep_idx.iter().enumerate() {
        if keep {
            if j != i {
                points.swap(j, i);
                agl_m.swap(j, i);
                elev_m.swap(j, i);
            }
            j += 1;
        }
    }
    points.truncate(j);
    agl_m.truncate(j);
    elev_m.truncate(j);
}

/// Per-segment receiver-independent filter. `length_m` is the segment
/// arc length in metres; `dt_s` is the time between the sample-pair
/// endpoints; `start_agl_m` / `end_agl_m` are post-DEM AGL.
/// `is_airborne` reflects the classified phase (Airborne or Cruise).
///
/// Implied speed `length_m / dt_s` against [`MAX_PLAUSIBLE_SPEED_KT`]
/// is the canonical garbage detector: two samples placed 200 km apart
/// over 30 s is a mode-S decode error, the same gap over 30 min is
/// real cruise at 400 kt. A hard length cap couldn't make that
/// distinction and silently dropped legitimate sparse oceanic cruise.
#[inline]
pub fn segment_is_keepable(
    length_m: f32,
    dt_s: f32,
    start_agl_m: f32,
    end_agl_m: f32,
    avg_speed_kt: f32,
    profile_idx: u8,
    is_airborne: bool,
) -> bool {
    if !length_m.is_finite()
        || !start_agl_m.is_finite()
        || !end_agl_m.is_finite()
        || !avg_speed_kt.is_finite()
    {
        return false;
    }
    if start_agl_m.min(end_agl_m) < HARD_AGL_FLOOR_M {
        return false;
    }
    if length_m < MIN_SEGMENT_LENGTH_M {
        return false;
    }
    if !dt_s.is_finite() || dt_s <= 0.0 {
        return false;
    }
    let derived_kt = (length_m / dt_s) * MPS_TO_KT;
    if derived_kt > MAX_PLAUSIBLE_SPEED_KT {
        return false;
    }
    if is_airborne
        && noise_compute::emission::aircraft::is_jet_profile(profile_idx)
        && avg_speed_kt < JET_STALL_SPEED_KT
    {
        return false;
    }
    // Symmetric to the HARD_AGL_FLOOR_M check above: any single endpoint
    // breaching the ceiling drops the segment (mode-S decode errors are
    // typically a single-sample altitude spike, not a sustained climb).
    if is_airborne
        && noise_compute::emission::aircraft::is_helicopter_profile(profile_idx)
        && start_agl_m.max(end_agl_m) > HELICOPTER_AGL_CEIL_M
    {
        return false;
    }
    true
}

#[cfg(test)]
#[path = "filters_tests.rs"]
mod tests;

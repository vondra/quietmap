//! Which samples are on the ground (dev4 `ground_inference.rs`): surface reports, plus edge runs of
//! a flight whose transponder never reports the surface but whose samples look like it (low above
//! the terrain, slow, level) for at least 3 samples.

use super::altitude::Sample;

const EDGE_WINDOW_SAMPLES: usize = 32;
/// 30 ft (Doc 29 departure-segment start height): an ILS approach at 30-150 m never seeds ground.
const SURFACE_MAX_HEIGHT_M: f32 = 30.0 * 0.3048;
const SURFACE_MAX_SPEED_KT: f32 = 90.0;
const SURFACE_MAX_VERTICAL_RATE_FPM: f32 = 1200.0;
const SURFACE_MIN_INFERRED_SAMPLES: usize = 3;
/// 165 ft (the runway-end obstacle limitation surface) or 130 kt ends the edge scan: a climb leaves
/// ground inference within about 16 s of lift-off.
const STRONGLY_AIRBORNE_HEIGHT_M: f32 = 165.0 * 0.3048;
const STRONGLY_AIRBORNE_SPEED_KT: f32 = 130.0;
const LOCAL_WINDOW: usize = 2;

/// Per-sample on-ground flag.
pub fn ground_flags(samples: &[Sample]) -> Vec<bool> {
    let mut flags: Vec<bool> = samples
        .iter()
        .map(|s| s.point.is_surface_report())
        .collect();
    if samples.len() >= 2 {
        infer_edge(samples, &mut flags, true);
        infer_edge(samples, &mut flags, false);
    }
    flags
}

fn looks_like_surface(sample: &Sample) -> bool {
    sample.height_m <= SURFACE_MAX_HEIGHT_M
        && sample.point.ground_speed_kt <= SURFACE_MAX_SPEED_KT
        && sample.point.vertical_rate_fpm.abs() <= SURFACE_MAX_VERTICAL_RATE_FPM
}

fn infer_edge(samples: &[Sample], flags: &mut [bool], prefix: bool) {
    let n = samples.len();
    let edge = n.min(EDGE_WINDOW_SAMPLES);
    let reported = if prefix {
        flags[..edge].iter().any(|&g| g)
    } else {
        flags[n - edge..].iter().any(|&g| g)
    };
    let indices: Vec<usize> = if prefix {
        (0..edge).collect()
    } else {
        (n - edge..n).rev().collect()
    };
    let (mut inferred, mut seen_candidate, mut misses) = (Vec::new(), false, 0);
    for index in indices {
        if flags[index] || surface_candidate(samples, index) {
            seen_candidate = true;
            misses = 0;
            if !flags[index] {
                inferred.push(index);
            }
            continue;
        }
        if seen_candidate {
            misses += 1;
            if misses >= 2 {
                break;
            }
            continue;
        }
        if samples[index].height_m >= STRONGLY_AIRBORNE_HEIGHT_M
            || samples[index].point.ground_speed_kt >= STRONGLY_AIRBORNE_SPEED_KT
        {
            break;
        }
    }
    if reported || inferred.len() >= SURFACE_MIN_INFERRED_SAMPLES {
        for index in inferred {
            flags[index] = true;
        }
    }
}

/// A surface report, or a surface-like sample with at least 3 surface-like samples within +-2.
fn surface_candidate(samples: &[Sample], index: usize) -> bool {
    if samples[index].point.is_surface_report() {
        return true;
    }
    if !looks_like_surface(&samples[index]) {
        return false;
    }
    let low = index.saturating_sub(LOCAL_WINDOW);
    let high = (index + LOCAL_WINDOW + 1).min(samples.len());
    samples[low..high]
        .iter()
        .filter(|s| s.point.is_surface_report() || looks_like_surface(s))
        .count()
        >= 3
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aircraft::altitude::tests::sample;

    fn heights(samples: &[(f32, f32, f32, bool)]) -> Vec<Sample> {
        samples
            .iter()
            .enumerate()
            .map(|(i, &(height_ft, speed, rate, surface))| {
                let mut s = sample(
                    i as f64,
                    50.0,
                    14.0,
                    if surface { f32::NAN } else { height_ft },
                    speed,
                );
                s.point.vertical_rate_fpm = rate;
                if surface {
                    s.point.flags = crate::aircraft::trace::SURFACE_REPORT;
                }
                s.height_m = if surface { 0.0 } else { height_ft * 0.3048 };
                s
            })
            .collect()
    }

    #[test]
    fn a_ground_prefix_without_surface_reports_is_recovered() {
        let samples = heights(&[
            (0.0, 8.0, 0.0, false),
            (0.0, 10.0, 0.0, false),
            (25.0, 12.0, 0.0, false),
            (50.0, 18.0, 0.0, false),
            (800.0, 200.0, 0.0, false),
            (2_000.0, 250.0, 0.0, false),
            (4_000.0, 280.0, 0.0, false),
            (8_000.0, 320.0, 0.0, false),
        ]);
        let flags = ground_flags(&samples);
        assert_eq!(flags[..5], [true, true, true, false, false]);
    }

    #[test]
    fn a_slow_cruise_sample_is_not_ground() {
        let samples = heights(&[
            (35_000.0, 450.0, 0.0, false),
            (35_000.0, 80.0, 0.0, false),
            (35_000.0, 450.0, 0.0, false),
        ]);
        assert!(!ground_flags(&samples).iter().any(|&g| g));
    }

    /// The 165 ft edge gate ends inference within seconds of rotation: 45 m at 140 kt is airborne.
    #[test]
    fn a_slow_climb_after_lift_off_is_airborne() {
        let samples = heights(&[
            (0.0, 8.0, 0.0, true),
            (0.0, 100.0, 200.0, true),
            (50.0, 130.0, 1500.0, false),
            (150.0, 140.0, 1800.0, false),
            (400.0, 150.0, 1900.0, false),
            (800.0, 160.0, 1900.0, false),
        ]);
        let flags = ground_flags(&samples);
        assert!(flags[0] && flags[1]);
        assert!(!flags[3] && !flags[4] && !flags[5], "{flags:?}");
    }
}

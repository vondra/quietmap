//! Phase classification (Ground / Airborne / Cruise) per trace point.
//!
//! Driven by AGL altitude with hysteresis around the 7 620 m
//! (= 25 000 ft) boundary — the last anchor of the Doc 29 NPD table.
//! Below the boundary the engine has validated NPD curves; above it
//! we rely on log-linear extrapolation per Doc 29 Eq. 4-5b.
//!
//! Hysteresis: enter cruise at ≥ 8 000 m AGL, exit at < 7 200 m AGL.
//! Without it a level cruise oscillating ± 200 ft around 25 000 ft
//! would flap phase 8–10 times per minute and produce a fresh segment
//! row every flap.

use crate::flight::Phase;

/// AGL boundary between Airborne and Cruise. Matches the
/// `AIRCRAFT_NPD_REF_SLANT_M = 7 620` constant in `noise-compute`.
pub const PHASE_BOUNDARY_AGL_M: f32 = 7_620.0;
pub const CRUISE_ENTER_AGL_M: f32 = 8_000.0;
pub const CRUISE_EXIT_AGL_M: f32 = 7_200.0;

/// No-hysteresis classification of one airborne sample. Used to seed
/// `prev_phase` from the first non-ground point so a cold-cache trace
/// whose first sample sits inside the buffer band (e.g. mid-descent at
/// 7 700 m AGL) is seeded into the correct side.
fn no_hysteresis_phase(agl_m: f32) -> Phase {
    if agl_m >= PHASE_BOUNDARY_AGL_M {
        Phase::Cruise
    } else {
        Phase::Airborne
    }
}

pub struct ClassifyInput<'a> {
    pub on_ground: &'a [bool],
    pub agl_m: &'a [f32],
}

/// Classify each trace point into a phase. Hysteresis is applied across
/// the AGL 7 200 m / 8 000 m buffer when the previous phase was airborne
/// or cruise; ground points always classify as Ground.
pub fn classify_points(input: ClassifyInput<'_>) -> Vec<Phase> {
    let n = input.on_ground.len();
    debug_assert_eq!(n, input.agl_m.len());
    let mut phases = Vec::with_capacity(n);

    let mut prev_phase = input
        .on_ground
        .iter()
        .zip(input.agl_m.iter())
        .find(|(g, _)| !**g)
        .map(|(_, &agl)| no_hysteresis_phase(agl))
        .unwrap_or(Phase::Airborne);

    for i in 0..n {
        if input.on_ground[i] {
            phases.push(Phase::Ground);
            continue;
        }
        let agl = input.agl_m[i];
        let phase = match prev_phase {
            Phase::Cruise if agl < CRUISE_EXIT_AGL_M => Phase::Airborne,
            Phase::Cruise => Phase::Cruise,
            _ if agl >= CRUISE_ENTER_AGL_M => Phase::Cruise,
            _ => Phase::Airborne,
        };
        prev_phase = phase;
        phases.push(phase);
    }
    phases
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descending_holds_cruise_in_buffer() {
        let phases = classify_points(ClassifyInput {
            on_ground: &[false; 4],
            agl_m: &[9000.0, 8500.0, 7500.0, 7000.0],
        });
        assert_eq!(
            phases,
            vec![Phase::Cruise, Phase::Cruise, Phase::Cruise, Phase::Airborne]
        );
    }

    #[test]
    fn ascending_waits_for_upper_buffer() {
        let phases = classify_points(ClassifyInput {
            on_ground: &[false; 4],
            agl_m: &[7000.0, 7620.0, 7900.0, 8100.0],
        });
        assert_eq!(
            phases,
            vec![
                Phase::Airborne,
                Phase::Airborne,
                Phase::Airborne,
                Phase::Cruise
            ]
        );
    }

    #[test]
    fn ground_overrides_agl() {
        let phases = classify_points(ClassifyInput {
            on_ground: &[true, true, true],
            agl_m: &[9000.0, 9000.0, 9000.0],
        });
        assert!(phases.iter().all(|p| *p == Phase::Ground));
    }

    #[test]
    fn cold_start_at_7700_seeds_into_cruise() {
        // 7 700 m sits between PHASE_BOUNDARY (7 620) and CRUISE_ENTER (8 000):
        // without seeding, prev_phase would default to Airborne and the trace
        // would mis-classify; with the seed it picks Cruise.
        let phases = classify_points(ClassifyInput {
            on_ground: &[false; 2],
            agl_m: &[7700.0, 7650.0],
        });
        assert_eq!(phases, vec![Phase::Cruise, Phase::Cruise]);
    }
}

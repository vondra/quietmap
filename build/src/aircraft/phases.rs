//! Flight phase per sample: ground, airborne, or cruise above the last Doc 29 NPD distance
//! (25,000 ft) with hysteresis (enter at 8,000 m, leave below 7,200 m above the terrain), so a
//! level flight near the boundary does not flap into a new segment every few samples.

/// Stored as u8 in the segment `phase` column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    Ground = 0,
    Airborne = 1,
    Cruise = 2,
}

/// The last Doc 29 NPD distance, 25,000 ft.
pub const PHASE_BOUNDARY_HEIGHT_M: f32 = 7_620.0;
const CRUISE_ENTER_HEIGHT_M: f32 = 8_000.0;
const CRUISE_EXIT_HEIGHT_M: f32 = 7_200.0;

/// Ground samples are ground; the first airborne sample seeds the state without hysteresis, so a
/// trace first seen inside the band starts on its side of the boundary.
pub fn classify(on_ground: &[bool], height_m: impl Fn(usize) -> f32) -> Vec<Phase> {
    let seed = (0..on_ground.len())
        .find(|&i| !on_ground[i])
        .map(|i| {
            if height_m(i) >= PHASE_BOUNDARY_HEIGHT_M {
                Phase::Cruise
            } else {
                Phase::Airborne
            }
        })
        .unwrap_or(Phase::Airborne);
    let mut previous = seed;
    (0..on_ground.len())
        .map(|i| {
            if on_ground[i] {
                return Phase::Ground;
            }
            let height = height_m(i);
            let phase = match previous {
                Phase::Cruise if height < CRUISE_EXIT_HEIGHT_M => Phase::Airborne,
                Phase::Cruise => Phase::Cruise,
                _ if height >= CRUISE_ENTER_HEIGHT_M => Phase::Cruise,
                _ => Phase::Airborne,
            };
            previous = phase;
            phase
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phases(ground: &[bool], heights: &[f32]) -> Vec<Phase> {
        classify(ground, |i| heights[i])
    }

    #[test]
    fn hysteresis_holds_cruise_and_waits_for_the_upper_edge() {
        use Phase::*;
        let level = [false; 4];
        assert_eq!(
            phases(&level, &[9000.0, 8500.0, 7500.0, 7000.0]),
            [Cruise, Cruise, Cruise, Airborne]
        );
        assert_eq!(
            phases(&level, &[7000.0, 7620.0, 7900.0, 8100.0]),
            [Airborne, Airborne, Airborne, Cruise]
        );
        assert_eq!(phases(&[true; 3], &[9000.0; 3]), [Ground; 3]);
        // 7,700 m lies inside the band: the seed puts it on the cruise side.
        assert_eq!(phases(&[false; 2], &[7700.0, 7650.0]), [Cruise, Cruise]);
    }
}

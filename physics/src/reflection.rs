//! Receiver reflection from the share of its neighbourhood covered by tall buildings.

/// The former enclosure neighbourhood extends 75 m either side of the receiver.
pub const REFLECTION_WINDOW_M: f64 = 2.0 * 75.0;
/// The former enclosure rule's saturated receiver gain.
pub const REFLECTION_MAX_DB: f64 = 3.0;

/// The old nine-probe gain averaged over independent probes with this building probability.
pub fn receiver_reflection_db(building_share: f64) -> f64 {
    let covered = building_share.clamp(0.0, 1.0);
    let open = 1.0 - covered;
    let mut covered_powers = [1.0; 10];
    let mut open_powers = [1.0; 10];
    for count in 1..=9 {
        covered_powers[count] = covered_powers[count - 1] * covered;
        open_powers[count] = open_powers[count - 1] * open;
    }
    // Nine choose k; the inherited >1/5 and >1/2 thresholds mean k >= 2 and k >= 5.
    [1, 9, 36, 84, 126, 126, 84, 36, 9, 1]
        .into_iter()
        .enumerate()
        .skip(2)
        .map(|(count, combinations)| {
            let gain = if count >= 5 {
                REFLECTION_MAX_DB
            } else {
                REFLECTION_MAX_DB / 2.0
            };
            gain * f64::from(combinations) * covered_powers[count] * open_powers[9 - count]
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_expected_gain_preserves_every_old_probe_outcome_and_is_smooth() {
        let mut previous = 0.0;
        for step in 0..=100 {
            let share = f64::from(step) / 100.0;
            let enumerated: f64 = (0u16..512)
                .map(|outcome| {
                    let count = outcome.count_ones() as i32;
                    let gain = if count >= 5 {
                        3.0
                    } else if count >= 2 {
                        1.5
                    } else {
                        0.0
                    };
                    gain * share.powi(count) * (1.0 - share).powi(9 - count)
                })
                .sum();
            let actual = receiver_reflection_db(share);
            assert!((actual - enumerated).abs() < 1e-12);
            assert!(actual >= previous - 1e-12);
            assert!((receiver_reflection_db(share + 1e-9) - actual).abs() < 1.35e-8);
            previous = actual;
        }
        assert_eq!(receiver_reflection_db(0.0), 0.0);
        assert_eq!(receiver_reflection_db(1.0), 3.0);
        assert!((receiver_reflection_db(0.2) - 0.875060736).abs() < 1e-12);
        assert!((receiver_reflection_db(0.5) - 2.220703125).abs() < 1e-12);
    }
}

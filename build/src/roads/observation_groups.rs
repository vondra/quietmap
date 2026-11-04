//! Preserve published vehicle-group totals while the selected year's class shares infer their unknown members.

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VehicleGroup {
    pub classes: u8,
    pub count: f64,
}

pub fn validate(groups: &[VehicleGroup]) -> Result<()> {
    let mut used = 0;
    for group in groups {
        ensure!(
            group.classes < 16
                && group.classes.count_ones() > 1
                && group.classes & used == 0
                && group.count.is_finite()
                && group.count >= 0.0,
            "invalid or overlapping published vehicle groups"
        );
        used |= group.classes;
    }
    Ok(())
}

pub fn fill(
    groups: &[VehicleGroup],
    counts: &mut [Option<f64>; 4],
    shares: [f64; 4],
    to_daily: impl Fn(f64) -> f64,
) -> Result<u8> {
    validate(groups)?;
    let mut status = 0;
    for group in groups {
        let members = (0..4)
            .filter(|i| group.classes & (1 << i) != 0)
            .collect::<Vec<_>>();
        let known: f64 = members.iter().filter_map(|&i| counts[i]).sum();
        let mut remaining = to_daily(group.count) - known;
        ensure!(
            remaining >= 0.0,
            "observed classes exceed their published group"
        );
        let unknown = members
            .into_iter()
            .filter(|&i| counts[i].is_none())
            .collect::<Vec<_>>();
        let mut weight: f64 = unknown.iter().map(|&i| shares[i]).sum();
        ensure!(
            (weight > 0.0 && !unknown.is_empty()) || remaining == 0.0,
            "published group has no admitted class share"
        );
        for (position, &i) in unknown.iter().enumerate() {
            let value = if position + 1 == unknown.len() {
                remaining
            } else if weight == 0.0 {
                0.0
            } else {
                (remaining * shares[i] / weight).round().min(remaining)
            };
            counts[i] = Some(value);
            status |= 1 << i;
            remaining -= value;
            weight -= shares[i];
        }
    }
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_group_keeps_its_total_and_measured_zero_without_double_counting_unknown_trucks() {
        let groups = [VehicleGroup {
            classes: 6,
            count: 30.0,
        }];
        let mut counts = [Some(70.0), None, None, Some(0.0)];
        assert_eq!(
            fill(&groups, &mut counts, [0.7, 0.1, 0.2, 0.01], |n| n).unwrap(),
            6
        );
        assert_eq!(counts, [Some(70.0), Some(10.0), Some(20.0), Some(0.0)]);
        let mut known = [Some(70.0), Some(0.0), None, None];
        assert_eq!(
            fill(&groups, &mut known, [0.7, 0.1, 0.2, 0.01], |n| n).unwrap(),
            4
        );
        assert_eq!(known[2], Some(30.0));
        assert!(
            validate(&[
                VehicleGroup {
                    classes: 6,
                    count: 30.0
                },
                VehicleGroup {
                    classes: 3,
                    count: 20.0
                }
            ])
            .is_err()
        );
    }
}

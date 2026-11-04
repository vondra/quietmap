//! Keep the exact dates for which a timetable defines service or a known zero.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Coverage {
    pub window_days: u16,
    pub covered: [u64; 6],
}

impl Coverage {
    pub fn from_days(days: &[bool]) -> Result<Self, String> {
        if days.is_empty() || days.len() > 366 {
            return Err("rail flow window must contain between one and 366 days".into());
        }
        let mut result = Self {
            window_days: days.len() as u16,
            covered: [0; 6],
        };
        for (index, covered) in days.iter().enumerate() {
            if *covered {
                result.covered[index / 64] |= 1 << (index % 64);
            }
        }
        Ok(result)
    }

    pub fn validate(self) -> Result<(), String> {
        if self.window_days > 366
            || (usize::from(self.window_days)..384)
                .any(|index| self.covered[index / 64] & (1 << (index % 64)) != 0)
        {
            return Err("rail coverage contains a day outside its flow window".into());
        }
        Ok(())
    }

    pub fn covered_days(self) -> u32 {
        self.covered.iter().map(|bits| bits.count_ones()).sum()
    }

    pub fn contains(self, index: usize) -> bool {
        index < usize::from(self.window_days) && self.covered[index / 64] & (1 << (index % 64)) != 0
    }

    pub fn missing_fraction(self) -> f64 {
        if self.window_days == 0 {
            return 0.0;
        }
        let covered = self.covered_days();
        f64::from(u32::from(self.window_days) - covered) / f64::from(self.window_days)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn coverage_preserves_dates_and_rejects_bits_outside_the_window() {
        let first = Coverage::from_days(&[true, true, false, false]).unwrap();
        assert_eq!(first.missing_fraction(), 0.5);
        assert!(first.contains(1));
        assert!(!first.contains(2));
        assert_eq!(Coverage::default().missing_fraction(), 0.0);
        assert!(
            Coverage {
                window_days: 1,
                covered: [2, 0, 0, 0, 0, 0]
            }
            .validate()
            .is_err()
        );
    }
}

//! Two-provider sampling window of an aircraft release: baseline days normalise primary-provider rows, increment days the secondary-only ones.
//!
//! Difference estimator per mean day: Σ_baseline E / |D_L| + Σ_increment
//! E_secondary-only / |D_X|. Every consumer divides by the baseline day count
//! and multiplies a row by its [`ProvenanceWeights`] entry, so a secondary-only
//! row contributes `E / |D_X|`.

use std::collections::HashMap;

/// Segment / row flag bit 6: the row touches a secondary-provider sample.
pub const SEGMENT_FLAG_SECONDARY_ONLY: u8 = 1 << 6;

pub const BASELINE_DAYS_KEY: &str = "baseline_days";
pub const INCREMENT_DAYS_KEY: &str = "increment_days";
pub const BASELINE_DAYS_SHA256_KEY: &str = "baseline_days_sha256";
pub const INCREMENT_DAYS_SHA256_KEY: &str = "increment_days_sha256";

/// Admitted day counts and the SHA-256 of each sorted day list (`\n`-joined).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SamplingWindow {
    pub baseline_days: u16,
    pub increment_days: u16,
    pub baseline_days_sha256: String,
    pub increment_days_sha256: String,
}

impl SamplingWindow {
    /// Schema metadata pairs every aircraft arrow carries.
    pub fn metadata(&self) -> [(&'static str, String); 4] {
        [
            (BASELINE_DAYS_KEY, self.baseline_days.to_string()),
            (INCREMENT_DAYS_KEY, self.increment_days.to_string()),
            (BASELINE_DAYS_SHA256_KEY, self.baseline_days_sha256.clone()),
            (INCREMENT_DAYS_SHA256_KEY, self.increment_days_sha256.clone()),
        ]
    }

    /// Fails on a missing, malformed or empty baseline stamp: arrows without
    /// it predate the provider union and must be re-extracted.
    pub fn from_metadata(metadata: &HashMap<String, String>) -> Result<Self, String> {
        let text = |key: &str| {
            metadata
                .get(key)
                .cloned()
                .ok_or_else(|| format!("{key} metadata missing — re-extract the aircraft layer"))
        };
        let count = |key: &str| -> Result<u16, String> {
            text(key)?
                .parse::<u16>()
                .map_err(|_| format!("{key} is not a day count"))
        };
        let window = Self {
            baseline_days: count(BASELINE_DAYS_KEY)?,
            increment_days: count(INCREMENT_DAYS_KEY)?,
            baseline_days_sha256: text(BASELINE_DAYS_SHA256_KEY)?,
            increment_days_sha256: text(INCREMENT_DAYS_SHA256_KEY)?,
        };
        if window.baseline_days == 0 {
            return Err(format!("{BASELINE_DAYS_KEY} = 0 — no admitted baseline day"));
        }
        Ok(window)
    }

    pub fn provenance_weights(&self) -> ProvenanceWeights {
        let secondary = if self.increment_days == 0 {
            0.0
        } else {
            f64::from(self.baseline_days) / f64::from(self.increment_days)
        };
        ProvenanceWeights([1.0, secondary])
    }
}

/// Energy and movement weight of a row relative to the baseline divisor:
/// `[primary, secondary-only]`. A window without increment days weights
/// secondary rows at zero; writers refuse such rows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProvenanceWeights([f64; 2]);

impl ProvenanceWeights {
    /// Primary-only identity for fixtures without a secondary provider.
    pub const PRIMARY_ONLY: Self = Self([1.0, 0.0]);

    #[inline]
    pub fn for_secondary_only(&self, secondary_only: bool) -> f64 {
        self.0[usize::from(secondary_only)]
    }

    #[inline]
    pub fn for_flags(&self, flags: u8) -> f64 {
        self.for_secondary_only(flags & SEGMENT_FLAG_SECONDARY_ONLY != 0)
    }

    /// `[primary, secondary]` for bulk upload (GPU, `f32` at the boundary).
    #[inline]
    pub fn as_array(&self) -> &[f64; 2] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secondary_rows_divide_by_the_increment_days() {
        let window = SamplingWindow {
            baseline_days: 360,
            increment_days: 11,
            baseline_days_sha256: "a".into(),
            increment_days_sha256: "b".into(),
        };
        let metadata: HashMap<String, String> = window
            .metadata()
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        assert_eq!(SamplingWindow::from_metadata(&metadata).unwrap(), window);
        let weights = window.provenance_weights();
        // E / baseline × weight = E / increment for a secondary-only row.
        let energy = 7.0;
        assert_eq!(weights.for_flags(0) * energy / 360.0, energy / 360.0);
        assert!((weights.for_flags(SEGMENT_FLAG_SECONDARY_ONLY) * energy / 360.0 - energy / 11.0).abs() < 1e-15);
        let mut stale = metadata.clone();
        stale.remove(INCREMENT_DAYS_SHA256_KEY);
        assert!(SamplingWindow::from_metadata(&stale).is_err());
        stale = metadata;
        stale.insert(BASELINE_DAYS_KEY.into(), "0".into());
        assert!(SamplingWindow::from_metadata(&stale).is_err());
    }
}

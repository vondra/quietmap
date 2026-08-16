//! Frozen H0 V3 field conversion, comparison, and H0/H32 decision.

use serde::{Deserialize, Serialize};

use crate::types::NUM_BANDS;

use super::h0_v3::{H0V3Error, H0V3Theta, H0_V3_MAX_DELTA_DB, JUDGE_HALVING_MAX_DELTA_DB};

/// One complete CPU field value. `None` is structural absence; callers must
/// not encode absence as an arbitrary dB floor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct H0V3Observation {
    pub key: u64,
    pub period_db: [Option<f64>; 3],
    pub period_band_db: [[Option<f64>; NUM_BANDS]; 3],
}

impl H0V3Observation {
    /// Convert one production-order `f32` period accumulation plus its
    /// full-precision per-band diagnostics into the sole V3 representation.
    pub fn from_accumulated_power(
        key: u64,
        period_power_f32: [f32; 3],
        period_band_power: [[f64; NUM_BANDS]; 3],
    ) -> Result<Self, H0V3Error> {
        let mut period_db = [None; 3];
        let mut period_band_db = [[None; NUM_BANDS]; 3];
        for period in 0..3 {
            let period_power = period_power_f32[period];
            if !period_power.is_finite() || period_power < 0.0 {
                return Err(H0V3Error::InvalidObservation);
            }
            for band in 0..NUM_BANDS {
                let power = period_band_power[period][band];
                if !power.is_finite() || power < 0.0 {
                    return Err(H0V3Error::InvalidObservation);
                }
                if power > 0.0 {
                    period_band_db[period][band] = Some(10.0 * power.log10());
                }
            }
            if period_power > 0.0 {
                period_db[period] = Some(10.0 * f64::from(period_power).log10());
            }
        }
        Ok(Self {
            key,
            period_db,
            period_band_db,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct H0V3DeltaSummary {
    pub pixels: usize,
    pub period_values: usize,
    pub period_presence_flips: usize,
    pub max_abs_period_db: f64,
    pub signed_mean_period_db: f64,
    pub band_values: [usize; NUM_BANDS],
    pub band_presence_flips: [usize; NUM_BANDS],
    pub max_abs_band_db: [f64; NUM_BANDS],
    pub signed_mean_band_db: [f64; NUM_BANDS],
}

/// Period-only report for the separately owned current-model/P2b fork. This
/// is deliberately not an H0 acceptance type: stock does not expose per-band
/// accumulations, and no model-delta threshold is hidden here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct H0V3PeriodDeltaSummary {
    pub pixels: usize,
    pub period_values: usize,
    pub period_presence_flips: usize,
    pub max_abs_period_db: f64,
    pub signed_mean_period_db: f64,
}

impl H0V3DeltaSummary {
    #[must_use]
    pub fn judge_halving_passes(&self) -> bool {
        self.period_presence_flips == 0 && self.max_abs_period_db <= JUDGE_HALVING_MAX_DELTA_DB
    }

    #[must_use]
    pub fn h0_v3_passes(&self) -> bool {
        self.period_presence_flips == 0 && self.max_abs_period_db <= H0_V3_MAX_DELTA_DB
    }
}

/// Score one candidate arm against one reference arm. Key order, presence and
/// every finite value are fail-closed; no caller-specific tolerance exists.
pub fn score_h0_v3(
    reference: &[H0V3Observation],
    candidate: &[H0V3Observation],
) -> Result<H0V3DeltaSummary, H0V3Error> {
    if reference.len() != candidate.len() {
        return Err(H0V3Error::ObservationKeyMismatch);
    }
    let mut period_values = 0_usize;
    let mut period_presence_flips = 0_usize;
    let mut period_signed_sum = 0.0;
    let mut max_abs_period_db = 0.0_f64;
    let mut band_values = [0_usize; NUM_BANDS];
    let mut band_presence_flips = [0_usize; NUM_BANDS];
    let mut band_signed_sum = [0.0_f64; NUM_BANDS];
    let mut max_abs_band_db = [0.0_f64; NUM_BANDS];
    let mut previous_key = None;
    for (reference, candidate) in reference.iter().zip(candidate) {
        if reference.key != candidate.key
            || previous_key.is_some_and(|previous| reference.key <= previous)
        {
            return Err(H0V3Error::ObservationKeyMismatch);
        }
        previous_key = Some(reference.key);
        for period in 0..3 {
            score_value(
                reference.period_db[period],
                candidate.period_db[period],
                &mut period_values,
                &mut period_presence_flips,
                &mut period_signed_sum,
                &mut max_abs_period_db,
            )?;
            for band in 0..NUM_BANDS {
                score_value(
                    reference.period_band_db[period][band],
                    candidate.period_band_db[period][band],
                    &mut band_values[band],
                    &mut band_presence_flips[band],
                    &mut band_signed_sum[band],
                    &mut max_abs_band_db[band],
                )?;
            }
        }
    }
    Ok(H0V3DeltaSummary {
        pixels: reference.len(),
        period_values,
        period_presence_flips,
        max_abs_period_db,
        signed_mean_period_db: mean(period_signed_sum, period_values),
        band_values,
        band_presence_flips,
        max_abs_band_db,
        signed_mean_band_db: std::array::from_fn(|band| {
            mean(band_signed_sum[band], band_values[band])
        }),
    })
}

/// Report the current-model/P2b delta without applying the quadrature budget.
pub fn score_h0_v3_periods(
    reference: &[H0V3Observation],
    candidate: &[H0V3Observation],
) -> Result<H0V3PeriodDeltaSummary, H0V3Error> {
    if reference.len() != candidate.len() {
        return Err(H0V3Error::ObservationKeyMismatch);
    }
    let mut values = 0;
    let mut presence_flips = 0;
    let mut signed_sum = 0.0;
    let mut max_abs = 0.0_f64;
    let mut previous_key = None;
    for (reference, candidate) in reference.iter().zip(candidate) {
        if reference.key != candidate.key
            || previous_key.is_some_and(|previous| reference.key <= previous)
        {
            return Err(H0V3Error::ObservationKeyMismatch);
        }
        previous_key = Some(reference.key);
        for period in 0..3 {
            score_value(
                reference.period_db[period],
                candidate.period_db[period],
                &mut values,
                &mut presence_flips,
                &mut signed_sum,
                &mut max_abs,
            )?;
        }
    }
    Ok(H0V3PeriodDeltaSummary {
        pixels: reference.len(),
        period_values: values,
        period_presence_flips: presence_flips,
        max_abs_period_db: max_abs,
        signed_mean_period_db: mean(signed_sum, values),
    })
}

fn mean(sum: f64, count: usize) -> f64 {
    if count == 0 {
        0.0
    } else {
        sum / count as f64
    }
}

fn score_value(
    reference: Option<f64>,
    candidate: Option<f64>,
    values: &mut usize,
    presence_flips: &mut usize,
    signed_sum: &mut f64,
    max_abs: &mut f64,
) -> Result<(), H0V3Error> {
    match (reference, candidate) {
        (None, None) => Ok(()),
        (Some(_), None) | (None, Some(_)) => {
            *presence_flips += 1;
            Ok(())
        }
        (Some(reference), Some(candidate)) => {
            if !reference.is_finite() || !candidate.is_finite() {
                return Err(H0V3Error::InvalidObservation);
            }
            let delta = candidate - reference;
            *values += 1;
            *signed_sum += delta;
            *max_abs = max_abs.max(delta.abs());
            Ok(())
        }
    }
}

/// Choose the coarsest arm whose own and every finer result pass, or request
/// H32. A non-monotonic finer failure can never hide behind a coarse pass.
pub fn select_h0_theta(
    summaries: &[(H0V3Theta, H0V3DeltaSummary)],
) -> Result<Option<H0V3Theta>, H0V3Error> {
    if summaries.len() != H0V3Theta::COARSE_TO_FINE.len()
        || !summaries
            .iter()
            .zip(H0V3Theta::COARSE_TO_FINE)
            .all(|((actual, _), expected)| *actual == expected)
    {
        return Err(H0V3Error::InvalidObservation);
    }
    Ok(summaries
        .iter()
        .enumerate()
        .find_map(|(index, (theta, _))| {
            summaries[index..]
                .iter()
                .all(|(_, summary)| summary.h0_v3_passes())
                .then_some(*theta)
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_delta_and_presence_mutations_fail() {
        let reference = vec![H0V3Observation {
            key: 7,
            period_db: [Some(40.0), Some(41.0), None],
            period_band_db: [[Some(30.0); NUM_BANDS]; 3],
        }];
        let mut candidate = reference.clone();
        candidate[0].period_db[0] = Some(40.500_001);
        assert!(!score_h0_v3(&reference, &candidate).unwrap().h0_v3_passes());
        candidate[0] = reference[0].clone();
        candidate[0].period_db[2] = Some(10.0);
        let delta = score_h0_v3(&reference, &candidate).unwrap();
        assert_eq!(delta.period_presence_flips, 1);
        assert!(!delta.h0_v3_passes());
    }

    #[test]
    fn field_conversion_pins_band_order_and_absence() {
        let mut powers = [[0.0; NUM_BANDS]; 3];
        powers[0][0] = 1.0;
        powers[0][1] = 9.0;
        powers[2][7] = 100.0;
        let observation =
            H0V3Observation::from_accumulated_power(9, [10.0, 0.0, 100.0], powers).unwrap();
        assert_eq!(observation.period_db, [Some(10.0), None, Some(20.0)]);
        assert_eq!(observation.period_band_db[0][0], Some(0.0));
        assert_eq!(observation.period_band_db[1], [None; NUM_BANDS]);
        powers[1][3] = f64::NAN;
        assert_eq!(
            H0V3Observation::from_accumulated_power(9, [10.0, 0.0, 100.0], powers),
            Err(H0V3Error::InvalidObservation)
        );
    }

    #[test]
    fn period_gate_uses_production_f32_accumulation_not_band_resummation() {
        let mut bands = [[0.0; NUM_BANDS]; 3];
        bands[0][0] = 1.0;
        let reference = H0V3Observation::from_accumulated_power(1, [1.0, 0.0, 0.0], bands).unwrap();
        let candidate = H0V3Observation::from_accumulated_power(
            1,
            [10.0_f32.powf(0.500_001 / 10.0), 0.0, 0.0],
            bands,
        )
        .unwrap();
        let delta = score_h0_v3(&[reference], &[candidate]).unwrap();
        assert!(!delta.h0_v3_passes());
        assert_eq!(delta.max_abs_band_db, [0.0; NUM_BANDS]);
    }

    #[test]
    fn selection_is_coarsest_passing_or_h32() {
        let summary = |max_abs_period_db| H0V3DeltaSummary {
            pixels: 1,
            period_values: 3,
            period_presence_flips: 0,
            max_abs_period_db,
            signed_mean_period_db: 0.0,
            band_values: [3; NUM_BANDS],
            band_presence_flips: [0; NUM_BANDS],
            max_abs_band_db: [0.0; NUM_BANDS],
            signed_mean_band_db: [0.0; NUM_BANDS],
        };
        let summaries = [
            (H0V3Theta::Degrees5, summary(0.8)),
            (H0V3Theta::Degrees4, summary(0.5)),
            (H0V3Theta::Degrees3, summary(0.2)),
            (H0V3Theta::Degrees2, summary(0.1)),
        ];
        assert_eq!(
            select_h0_theta(&summaries).unwrap(),
            Some(H0V3Theta::Degrees4)
        );
        let failed = H0V3Theta::COARSE_TO_FINE.map(|theta| (theta, summary(0.6)));
        assert_eq!(select_h0_theta(&failed).unwrap(), None);

        let non_monotonic = [
            (H0V3Theta::Degrees5, summary(0.2)),
            (H0V3Theta::Degrees4, summary(0.2)),
            (H0V3Theta::Degrees3, summary(0.9)),
            (H0V3Theta::Degrees2, summary(0.2)),
        ];
        assert_eq!(
            select_h0_theta(&non_monotonic).unwrap(),
            Some(H0V3Theta::Degrees2)
        );
        let finest_fails = [
            (H0V3Theta::Degrees5, summary(0.9)),
            (H0V3Theta::Degrees4, summary(0.2)),
            (H0V3Theta::Degrees3, summary(0.2)),
            (H0V3Theta::Degrees2, summary(0.9)),
        ];
        assert_eq!(select_h0_theta(&finest_fails).unwrap(), None);
    }

    #[test]
    fn stock_model_delta_is_report_only_and_period_only() {
        let stock = H0V3Observation {
            key: 1,
            period_db: [Some(40.0), None, Some(20.0)],
            period_band_db: [[None; NUM_BANDS]; 3],
        };
        let h0 = H0V3Observation {
            key: 1,
            period_db: [Some(41.0), Some(10.0), Some(19.5)],
            period_band_db: [[Some(200.0); NUM_BANDS]; 3],
        };
        let delta = score_h0_v3_periods(&[stock], &[h0]).unwrap();
        assert_eq!(delta.period_values, 2);
        assert_eq!(delta.period_presence_flips, 1);
        assert_eq!(delta.max_abs_period_db, 1.0);
        assert_eq!(delta.signed_mean_period_db, 0.25);
    }
}

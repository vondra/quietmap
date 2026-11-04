//! Publisher-independent road observations preserve time, count scope and each unknown vehicle class.

use anyhow::{Result, ensure};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CountBasis {
    Unknown,
    Directional,
    BothDirections,
    Allocated,
    StreetCrossSection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TimeBasis {
    AnnualDaily,
    WeekdayDaily,
    Hourly,
    PeriodDaily,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub identity: String,
    pub input_id: String,
    pub source_id: u16,
    pub source_rank: u32,
    pub measured: bool,
    pub country: u16,
    pub observed_start: String,
    pub observed_end: String,
    pub count_basis: CountBasis,
    pub time_basis: TimeBasis,
    pub direction: u8,
    pub reference: String,
    pub geometry: Vec<Vec<[f64; 2]>>,
    pub road_class_mask: u16,
    pub search_radius_m: f64,
    pub reference_required: bool,
    pub vehicle_classes: [Option<f64>; 4],
    #[serde(default)]
    pub vehicle_groups: Vec<super::observation_groups::VehicleGroup>,
    pub total: Option<f64>,
    pub estimated_classes: u8,
    pub daily_factor: f64,
    #[serde(default)]
    pub target: super::observation_target::Target,
    #[serde(default)]
    pub sample: super::observation_sample::Sample,
}

impl Observation {
    pub fn validate(&self, cutoff: NaiveDate) -> Result<()> {
        self.target.validate()?;
        super::observation_groups::validate(&self.vehicle_groups)?;
        self.sample.validate()?;
        if self.target.way_assignment.is_some() {
            ensure!(
                self.geometry.len() == 1 && self.geometry[0].len() == 1 && self.direction == 0,
                "way assignment requires a located counter point"
            );
        }
        if let Some(fallback) = &self.target.reference_fallback {
            ensure!(
                self.reference_required
                    && !self.geometry.is_empty()
                    && fallback.search_radius_m <= self.search_radius_m,
                "reference fallback needs enclosing geometric support"
            );
        }
        ensure!(
            !self.identity.is_empty() && !self.input_id.is_empty(),
            "observation identity is empty"
        );
        let start = NaiveDate::parse_from_str(&self.observed_start, "%Y-%m-%d")?;
        let end = NaiveDate::parse_from_str(&self.observed_end, "%Y-%m-%d")?;
        ensure!(
            start <= end && end < cutoff,
            "observation {} lies outside the admitted past",
            self.identity
        );
        ensure!(
            self.source_id > 0 && self.estimated_classes <= 15 && self.direction <= 2,
            "invalid observation source, class status or direction"
        );
        ensure!(
            self.road_class_mask > 0 && self.road_class_mask < 1 << 13,
            "invalid observation road-class mask"
        );
        ensure!(
            self.search_radius_m.is_finite()
                && self.search_radius_m >= 0.0
                && (self.geometry.is_empty() || self.search_radius_m > 0.0),
            "invalid observation search radius"
        );
        ensure!(
            self.daily_factor.is_finite() && self.daily_factor > 0.0,
            "missing conversion to annual daily traffic"
        );
        ensure!(
            self.time_basis != TimeBasis::AnnualDaily || self.daily_factor == 1.0,
            "annual daily traffic must not be scaled twice"
        );
        ensure!(
            !self.reference_required || !self.reference.is_empty(),
            "required reference is missing"
        );
        ensure!(
            (!self.geometry.is_empty() || self.country != 0)
                && self.geometry.iter().all(|part| !part.is_empty()
                    && part.iter().all(super::observation_target::coordinate)),
            "invalid observation geometry"
        );
        ensure!(
            !self.geometry.is_empty()
                || !self.measured
                || self.sample.fit_only
                || self.target.source_way.is_some()
                || self.reference_required
                || !self.target.identities.is_empty(),
            "an unlocated measurement needs road identity"
        );
        ensure!(
            self.vehicle_classes
                .iter()
                .flatten()
                .chain(self.total.iter())
                .all(|value| value.is_finite() && *value >= 0.0),
            "invalid observed count"
        );
        ensure!(
            self.total.is_some()
                || self.vehicle_classes.iter().any(Option::is_some)
                || !self.vehicle_groups.is_empty(),
            "observation has no count"
        );
        Ok(())
    }

    pub fn daily_classes(&self, missing_shares: [f64; 4]) -> Result<([f64; 4], u8)> {
        self.daily_classes_using_factor(missing_shares, self.daily_factor)
    }

    pub fn daily_classes_using_factor(
        &self,
        missing_shares: [f64; 4],
        daily_factor: f64,
    ) -> Result<([f64; 4], u8)> {
        ensure!(
            daily_factor.is_finite() && daily_factor > 0.0,
            "invalid daily conversion"
        );
        ensure!(
            missing_shares.iter().all(|v| v.is_finite() && *v >= 0.0),
            "invalid class shares"
        );
        let to_daily = |value: f64| {
            let daily = value * daily_factor;
            if matches!(
                self.time_basis,
                TimeBasis::WeekdayDaily | TimeBasis::PeriodDaily
            ) {
                daily.round()
            } else {
                daily
            }
        };
        let mut counts = self.vehicle_classes.map(|v| v.map(to_daily));
        let grouped_status = super::observation_groups::fill(
            &self.vehicle_groups,
            &mut counts,
            missing_shares,
            to_daily,
        )?;
        let known: f64 = counts.iter().flatten().sum();
        let weight: f64 = missing_shares.iter().sum();
        let missing = counts.iter().any(Option::is_none);
        let mut status = self.estimated_classes | grouped_status;
        if missing {
            let total = if let Some(total) = self.total {
                to_daily(total)
            } else {
                // A survey of cars and lorries does not establish a total including motorcycles.
                let known_weight: f64 = missing_shares
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| counts[*i].is_some())
                    .map(|(_, value)| value)
                    .sum();
                ensure!(
                    known_weight > 0.0 || known == 0.0,
                    "observed classes have no model share"
                );
                if known == 0.0 {
                    0.0
                } else {
                    known * weight / known_weight
                }
            };
            ensure!(total >= known, "observed classes exceed the total");
            ensure!(
                weight > 0.0 || total == known,
                "missing classes have no model share"
            );
            // Preserve the published classes; infer heavy, motorcycle and medium totals, with light
            // as the remainder (the original city census allocation). The last unknown conserves flow.
            let unknown = [2, 3, 1, 0]
                .into_iter()
                .filter(|&i| counts[i].is_none())
                .collect::<Vec<_>>();
            let mut remaining = total - known;
            for (position, &i) in unknown.iter().enumerate() {
                status |= 1 << i;
                let value = if position + 1 == unknown.len() || weight == 0.0 {
                    remaining
                } else {
                    (total * missing_shares[i] / weight).round().min(remaining)
                };
                counts[i] = Some(value);
                remaining -= value;
            }
        }
        Ok((counts.map(Option::unwrap), status))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn observation() -> Observation {
        Observation {
            identity: "counter".into(),
            input_id: "survey".into(),
            source_id: 1,
            source_rank: 1,
            measured: true,
            country: 0,
            observed_start: "2000-01-01".into(),
            observed_end: "2000-12-31".into(),
            count_basis: CountBasis::BothDirections,
            time_basis: TimeBasis::AnnualDaily,
            direction: 0,
            reference: "A1".into(),
            geometry: vec![vec![[0.0, 0.0]]],
            road_class_mask: 1,
            search_radius_m: 100.0,
            reference_required: true,
            vehicle_groups: Vec::new(),
            vehicle_classes: [Some(80.0), None, Some(0.0), None],
            total: Some(100.0),
            estimated_classes: 0,
            daily_factor: 1.0,
            target: Default::default(),
            sample: Default::default(),
        }
    }

    #[test]
    fn missing_classes_keep_the_observed_zero_and_fill_only_the_residual() {
        let (counts, flags) = observation().daily_classes([0.8, 0.1, 0.05, 0.05]).unwrap();
        assert_eq!(counts, [80.0, 15.0, 0.0, 5.0]);
        assert_eq!(flags, 0b1010);
        let mut row = observation();
        row.vehicle_classes = [Some(80.0), Some(10.0), Some(0.0), Some(10.0)];
        row.estimated_classes = 15;
        assert_eq!(
            row.daily_classes([0.25; 4]).unwrap().0,
            [80.0, 10.0, 0.0, 10.0]
        );
    }

    #[test]
    fn missing_motorcycles_are_inferred_without_replacing_the_counted_motor_vehicles() {
        let mut row = observation();
        row.vehicle_classes = [Some(80.0), Some(10.0), Some(0.0), None];
        row.total = None;
        let (classes, flags) = row.daily_classes([80.0, 10.0, 0.0, 10.0]).unwrap();
        assert_eq!(classes, [80.0, 10.0, 0.0, 10.0]);
        assert_eq!(flags, 8);
    }

    #[test]
    fn intervals_are_observed_time_and_the_state_cutoff_is_exclusive() {
        let row = observation();
        assert!(
            row.validate(NaiveDate::from_ymd_opt(2001, 1, 1).unwrap())
                .is_ok()
        );
        assert!(
            row.validate(NaiveDate::from_ymd_opt(2000, 12, 31).unwrap())
                .is_err()
        );
        let mut missing = row.clone();
        missing.total = None;
        assert!(missing.daily_classes([0.25; 4]).is_ok());
        missing.total = Some(70.0);
        assert!(missing.daily_classes([0.25; 4]).is_err());
    }
}

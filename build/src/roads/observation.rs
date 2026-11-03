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
    pub total: Option<f64>,
    pub estimated_classes: u8,
    pub daily_factor: f64,
}

impl Observation {
    pub fn validate(&self, cutoff: NaiveDate) -> Result<()> {
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
            self.search_radius_m.is_finite() && self.search_radius_m > 0.0,
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
            !self.geometry.is_empty()
                && self.geometry.iter().all(|part| !part.is_empty()
                    && part.iter().all(|[lon, lat]| lon.is_finite()
                        && lat.is_finite()
                        && (-180.0..=180.0).contains(lon)
                        && (-90.0..=90.0).contains(lat))),
            "invalid observation geometry"
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
            self.total.is_some() || self.vehicle_classes.iter().any(Option::is_some),
            "observation has no count"
        );
        Ok(())
    }

    pub fn daily_classes(&self, missing_shares: [f64; 4]) -> Result<([f64; 4], u8)> {
        ensure!(
            missing_shares.iter().all(|v| v.is_finite() && *v >= 0.0),
            "invalid class shares"
        );
        let known: f64 = self.vehicle_classes.iter().flatten().sum();
        let missing_weight: f64 = (0..4)
            .filter(|&i| self.vehicle_classes[i].is_none())
            .map(|i| missing_shares[i])
            .sum();
        let missing = self.vehicle_classes.iter().any(Option::is_none);
        let residual = if missing {
            let total = self
                .total
                .ok_or_else(|| anyhow::anyhow!("missing classes require a total"))?;
            ensure!(total >= known, "observed classes exceed the total");
            ensure!(
                missing_weight > 0.0 || total == known,
                "missing classes have no model share"
            );
            total - known
        } else {
            0.0
        };
        let mut status = self.estimated_classes;
        let counts = std::array::from_fn(|i| {
            self.vehicle_classes[i].unwrap_or_else(|| {
                status |= 1 << i;
                if residual == 0.0 {
                    0.0
                } else {
                    residual * missing_shares[i] / missing_weight
                }
            }) * self.daily_factor
        });
        Ok((counts, status))
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
            vehicle_classes: [Some(80.0), None, Some(0.0), None],
            total: Some(100.0),
            estimated_classes: 0,
            daily_factor: 1.0,
        }
    }

    #[test]
    fn missing_classes_keep_the_observed_zero_and_fill_only_the_residual() {
        let (counts, flags) = observation().daily_classes([0.8, 0.1, 0.05, 0.05]).unwrap();
        assert_eq!(
            counts,
            [
                80.0,
                20.0 * 0.1 / (0.1 + 0.05),
                0.0,
                20.0 * 0.05 / (0.1 + 0.05)
            ]
        );
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
        assert!(missing.daily_classes([0.25; 4]).is_err());
        missing.total = Some(70.0);
        assert!(missing.daily_classes([0.25; 4]).is_err());
    }
}

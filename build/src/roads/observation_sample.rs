//! Observed sampling context and complete-day hourly totals used to fit road models without future data.

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

/// How the publisher chose where to count, as its documentation states: every section of its
/// network, a random sample of its locations, or sites it selected (permanent stations, a city's
/// main streets).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SamplingDesign {
    #[default]
    Undeclared,
    WholeNetwork,
    RandomSample,
    SelectedSites,
}

impl SamplingDesign {
    /// Its code in the prepared roads' `traffic_observation_design` column.
    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(code: u8) -> Option<Self> {
        [
            Self::Undeclared,
            Self::WholeNetwork,
            Self::RandomSample,
            Self::SelectedSites,
        ]
        .get(usize::from(code))
        .copied()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Sample {
    #[serde(skip_serializing_if = "super::observation_target::is_default")]
    pub fit_only: bool,
    #[serde(skip_serializing_if = "super::observation_target::is_default")]
    pub road_class: Option<u8>,
    /// Unknown, rural, urban; publisher classification, never inferred from the traffic value.
    #[serde(skip_serializing_if = "super::observation_target::is_default")]
    pub environment: u8,
    #[serde(skip_serializing_if = "super::observation_target::is_default")]
    pub length_m: Option<f64>,
    #[serde(skip_serializing_if = "super::observation_target::is_default")]
    pub hourly: Option<Hourly>,
    #[serde(skip_serializing_if = "super::observation_target::is_default")]
    pub design: SamplingDesign,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Hourly {
    pub complete_days: u32,
    pub all_vehicles: [f64; 24],
    pub heavy_vehicles: Option<[f64; 24]>,
    #[serde(default)]
    pub weekday_days: u32,
    #[serde(default)]
    pub weekday_vehicles: f64,
}

impl Sample {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.road_class.is_none_or(|c| c < 13)
                && self.environment < 3
                && self.length_m.is_none_or(|m| m.is_finite() && m > 0.0),
            "invalid road sampling context"
        );
        if let Some(hourly) = &self.hourly {
            ensure!(
                hourly.weekday_days <= hourly.complete_days
                    && hourly.weekday_vehicles.is_finite()
                    && hourly.weekday_vehicles >= 0.0
                    && hourly.weekday_vehicles <= hourly.all_vehicles.iter().sum::<f64>()
                    && (hourly.weekday_days > 0 || hourly.weekday_vehicles == 0.0),
                "invalid weekday sample"
            );
            ensure!(
                hourly.complete_days > 0
                    && hourly
                        .all_vehicles
                        .iter()
                        .all(|v| v.is_finite() && *v >= 0.0)
                    && hourly.all_vehicles.iter().sum::<f64>() > 0.0,
                "invalid hourly sample"
            );
            ensure!(
                hourly.heavy_vehicles.as_ref().is_none_or(|heavy| heavy
                    .iter()
                    .zip(hourly.all_vehicles)
                    .all(|(h, all)| h.is_finite() && *h >= 0.0 && *h <= all)),
                "invalid hourly heavy component"
            );
        }
        Ok(())
    }
}

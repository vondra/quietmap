//! Observed sampling context and complete-day hourly totals used to fit road models without future data.

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Sample {
    pub fit_only: bool,
    pub road_class: Option<u8>,
    /// Unknown, rural, urban; publisher classification, never inferred from the traffic value.
    pub environment: u8,
    pub length_m: Option<f64>,
    pub hourly: Option<Hourly>,
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

//! The common screening-height ladder and demand storeys, parameterized by admitted measurements.

use crate::screening::BUILDING_HEIGHT_MAX_M;
use anyhow::{Result, ensure};
use serde::Deserialize;
use std::{collections::BTreeMap, path::Path};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeightModel {
    pub measured_bounds_m: [f64; 2],
    pub overture_minimum_m: f64,
    pub area_upper_m2: Vec<f64>,
    pub area_height_m: Vec<f64>,
    pub floor_ranges: Vec<FloorRange>,
    pub storey_height_m: f64,
    pub storey_roof_m: f64,
    pub wall_default_m: f64,
    pub wall_country_m: BTreeMap<String, f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FloorRange {
    pub through: u8,
    pub slope_m: f64,
    pub intercept_m: f64,
}

impl HeightModel {
    pub fn read(path: &Path) -> Result<Self> {
        let model: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        model.validate()?;
        Ok(model)
    }

    pub fn validate(&self) -> Result<()> {
        let heights = self
            .area_height_m
            .iter()
            .copied()
            .chain(self.measured_bounds_m)
            .chain([
                self.overture_minimum_m,
                self.storey_height_m,
                self.wall_default_m,
            ])
            .chain(self.wall_country_m.values().copied());
        ensure!(
            heights
                .into_iter()
                .all(|h| h.is_finite() && h > 0.0 && h <= 32767.0),
            "invalid height model metres"
        );
        ensure!(
            self.measured_bounds_m[0] <= self.measured_bounds_m[1],
            "reversed measured height bounds"
        );
        ensure!(
            self.area_height_m.len() == self.area_upper_m2.len() + 1,
            "area bins need one terminal height"
        );
        ensure!(
            self.area_upper_m2.iter().all(|v| v.is_finite() && *v > 0.0)
                && self.area_upper_m2.windows(2).all(|p| p[0] < p[1]),
            "invalid area bins"
        );
        ensure!(
            self.floor_ranges
                .last()
                .is_some_and(|r| r.through == u8::MAX)
                && self
                    .floor_ranges
                    .windows(2)
                    .all(|p| p[0].through < p[1].through),
            "floor ranges must cover all storeys"
        );
        for floors in 1..=u8::MAX {
            let height = self.floors_height(floors);
            ensure!(
                height.is_finite() && height > 0.0 && height <= 32767.0,
                "invalid floor height"
            );
        }
        ensure!(self.storey_roof_m.is_finite(), "invalid storey roof offset");
        ensure!(
            self.wall_country_m
                .keys()
                .all(|c| c.len() == 2 && c.bytes().all(|b| b.is_ascii_uppercase())),
            "wall mean needs an ISO country"
        );
        Ok(())
    }

    fn floors_height(&self, floors: u8) -> f64 {
        let range = self
            .floor_ranges
            .iter()
            .find(|r| floors <= r.through)
            .expect("validated floor ranges");
        f64::from(floors) * range.slope_m + range.intercept_m
    }

    /// A tagged height (OSM or Overture) taller than any building is a tag error, not evidence
    /// (2024's OSM tags 75 buildings above it, up to 320,000 m): the ladder goes on to the next.
    pub fn height(
        &self,
        measured: Option<f64>,
        mapped: Option<f64>,
        floors: u8,
        overture: Option<f64>,
        area_m2: f64,
    ) -> (f64, u8) {
        if let Some(value) = measured {
            (
                value.clamp(self.measured_bounds_m[0], self.measured_bounds_m[1]),
                3,
            )
        } else if let Some(value) = mapped.filter(|h| *h > 0.0 && *h <= BUILDING_HEIGHT_MAX_M) {
            (value, 0)
        } else if floors > 0 {
            (self.floors_height(floors), 1)
        } else if let Some(value) =
            overture.filter(|h| (self.overture_minimum_m..=BUILDING_HEIGHT_MAX_M).contains(h))
        {
            (value, 5)
        } else {
            let bin = self
                .area_upper_m2
                .iter()
                .position(|upper| area_m2 < *upper)
                .unwrap_or(self.area_upper_m2.len());
            (self.area_height_m[bin], 2)
        }
    }

    pub fn storeys(&self, floors: u8, height: Option<f64>) -> u8 {
        if floors > 0 {
            return floors;
        }
        height.map_or(1, |h| {
            ((h - self.storey_roof_m) / self.storey_height_m + 0.5)
                .floor()
                .clamp(1.0, 255.0) as u8
        })
    }

    pub fn wall_height(&self, mapped: f32, has_height: bool, country: u16) -> (f64, u8) {
        // A wall's tag obeys the same bound; outside it the country's wall height stands in.
        if has_height && (0.0..=BUILDING_HEIGHT_MAX_M).contains(&f64::from(mapped)) {
            return (f64::from(mapped), 0);
        }
        let bytes = country.to_le_bytes();
        let iso = std::str::from_utf8(&bytes).unwrap_or("");
        (
            self.wall_country_m
                .get(iso)
                .copied()
                .unwrap_or(self.wall_default_m),
            8,
        )
    }
}

pub fn screening_metres(value: f64) -> Result<i16> {
    ensure!(
        value.is_finite() && (0.0..=32767.0).contains(&value),
        "screening height outside Int16 metres: {value}"
    );
    Ok((value + 0.5).floor() as i16)
}

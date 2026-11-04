//! Preserve the source identity, country and admitted interval shared by publisher converters.

use crate::{
    roads::observation::{CountBasis, Observation, TimeBasis},
    year::{Input, YearInputs},
};
use anyhow::{Result, anyhow, ensure};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Source {
    pub input_id: String,
    pub source_id: u16,
    pub source_rank: u32,
}

impl Source {
    pub(super) fn input<'a>(&self, inputs: &'a YearInputs) -> Result<&'a Input> {
        inputs.by_id(&self.input_id).map_err(anyhow::Error::msg)?;
        inputs
            .manifest
            .inputs
            .iter()
            .find(|r| r.id == self.input_id)
            .ok_or_else(|| anyhow!("unknown road count input"))
    }
    pub(super) fn observation(
        &self,
        inputs: &YearInputs,
        identity: String,
        count_basis: CountBasis,
    ) -> Result<Observation> {
        let input = self.input(inputs)?;
        let (start, end) = input.observed.bounds().map_err(anyhow::Error::msg)?;
        let region = input
            .region
            .split(':')
            .next()
            .unwrap_or_default()
            .as_bytes();
        ensure!(
            region.len() == 2 && region.iter().all(u8::is_ascii_uppercase),
            "a national or municipal observation needs its country"
        );
        Ok(Observation {
            identity,
            input_id: self.input_id.clone(),
            source_id: self.source_id,
            source_rank: self.source_rank,
            measured: true,
            country: u16::from_le_bytes(region.try_into()?),
            observed_start: start.to_string(),
            observed_end: end.to_string(),
            count_basis,
            time_basis: TimeBasis::AnnualDaily,
            direction: 0,
            reference: String::new(),
            geometry: Vec::new(),
            road_class_mask: 0,
            search_radius_m: 0.0,
            reference_required: false,
            vehicle_classes: [None; 4],
            total: None,
            estimated_classes: 0,
            daily_factor: 1.0,
            target: Default::default(),
            sample: Default::default(),
        })
    }
}

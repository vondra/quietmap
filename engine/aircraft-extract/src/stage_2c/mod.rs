//! Ground operations: per-owner traffic and one global airport movement summary.
use crate::scope::ScopeBbox;
use anyhow::Result;
use noise_compute::types::AirportArea;
use std::path::Path;

pub mod airport_summary_reduce;
pub mod airport_traffic;
pub mod airport_traffic_writer;
pub const AIRPORT_SUMMARY_FILENAME: &str = "airport_summary.arrow";

pub fn run_stage_2c(
    segments_by_square_dir: &Path,
    airport_areas: &[AirportArea],
    prepared_year_dir: &Path,
    n_days: u16,
    ga_n_days: u16,
    scope: Option<&ScopeBbox>,
) -> Result<usize> {
    anyhow::ensure!(
        n_days > 0,
        "primary sampling window must contain at least one day"
    );
    let count = airport_traffic_writer::run_airport_traffic(
        segments_by_square_dir,
        airport_areas,
        prepared_year_dir,
        n_days,
        ga_n_days,
        scope,
    )?;
    let parts = prepared_year_dir.join("airport_summary_parts");
    let summary = prepared_year_dir
        .join("aircraft")
        .join(AIRPORT_SUMMARY_FILENAME);
    airport_summary_reduce::run_airport_summary_reduce(&parts, &summary)?;
    std::fs::remove_dir_all(&parts)?;
    Ok(count)
}

#[cfg(test)]
mod tests;

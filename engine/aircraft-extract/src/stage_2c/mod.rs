//! Ground operations: traffic and globally deduplicated airport summaries in their owner cells.
use crate::scope::ScopeBbox;
use anyhow::Result;
use noise_compute::types::AirportArea;
use std::path::Path;

pub(crate) mod admission;
pub mod airport_summary_reduce;
pub mod airport_traffic;
pub mod airport_line_index;
pub mod airport_traffic_writer;
pub(crate) mod movements;
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
    let pending = prepared_year_dir.join(".airport_traffic_pending");
    anyhow::ensure!(
        !pending.try_exists()?,
        "incomplete ground output exists at {}; inspect it before a fresh run",
        pending.display()
    );
    crate::arrow_io::create_directory_all_synced(prepared_year_dir)?;
    std::fs::create_dir(&pending)?;
    std::fs::File::open(prepared_year_dir)?.sync_all()?;
    let count = airport_traffic_writer::run_airport_traffic(
        segments_by_square_dir,
        airport_areas,
        prepared_year_dir,
        &pending,
        n_days,
        ga_n_days,
        scope,
    )?;
    let parts = pending.join("airport_summary_parts");
    airport_summary_reduce::run_airport_summary_reduce(&parts, &pending)?;
    // No old prepared output is removed until all counter and global-union
    // admission and writes succeed. This is a fresh-run boundary, not resume.
    for name in ["airport_traffic.arrow", AIRPORT_SUMMARY_FILENAME] {
        crate::wipe::wipe_stale_arrows_for_scope(prepared_year_dir, name, scope)?;
    }
    for (_, directory) in crate::spatial::square_directories(&pending)? {
        let relative = directory.strip_prefix(&pending)?;
        let destination = prepared_year_dir.join(relative);
        crate::arrow_io::create_directory_all_synced(&destination)?;
        for name in ["airport_traffic.arrow", AIRPORT_SUMMARY_FILENAME] {
            let source = directory.join(name);
            if source.try_exists()? {
                std::fs::rename(&source, destination.join(name))?;
                std::fs::File::open(&destination)?.sync_all()?;
            }
        }
    }
    std::fs::remove_dir_all(&pending)?;
    std::fs::File::open(prepared_year_dir)?.sync_all()?;
    Ok(count)
}

#[cfg(test)]
mod tests;

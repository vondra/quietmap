//! Explicit multi-directory shuffle; all phase execution goes through run-all.

use crate::cli_validate::{list_segments_day_paths_multi, parse_scope};
use aircraft_extract::progress::ts;
use anyhow::Result;
use std::path::PathBuf;

pub fn run_subcmd_shuffle(
    segments_dir: Vec<PathBuf>,
    ga_segments_dir: Vec<PathBuf>,
    out_dir: PathBuf,
    scope_bbox: Option<String>,
) -> Result<()> {
    let scope = parse_scope(scope_bbox.as_deref())?;
    let day_paths = list_segments_day_paths_multi(&segments_dir)?;
    let ga_day_paths = list_segments_day_paths_multi(&ga_segments_dir)?;
    aircraft_extract::shuffle::shuffle_per_square(
        &day_paths,
        &ga_day_paths,
        &out_dir,
        scope.as_ref(),
    )?;
    eprintln!(
        "{} [shuffle] {} airline + {} GA day shards → {}",
        ts(),
        day_paths.len(),
        ga_day_paths.len(),
        out_dir.display()
    );
    Ok(())
}

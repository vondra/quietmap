//! Aircraft stage dispatcher; arguments, validation, and orchestration live in focused modules.

use aircraft_extract::progress::ts;
use anyhow::Result;
use clap::Parser;
use std::num::NonZeroUsize;
#[path = "aircraft_extract/args.rs"]
mod args;
use args::*;
#[path = "aircraft_extract/cli_audit.rs"]
mod cli_audit;
#[path = "aircraft_extract/cli_days.rs"]
mod cli_days;
#[path = "aircraft_extract/cli_run_all.rs"]
mod cli_run_all;
#[path = "aircraft_extract/cli_runners.rs"]
mod cli_runners;
#[path = "aircraft_extract/cli_validate.rs"]
mod cli_validate;

#[path = "aircraft_extract/source_cache.rs"]
mod source_cache;

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_rayon_pool(cli.max_threads)?;
    match cli.cmd {
        Cmd::ValidateSegments {
            adsb_cache,
            segments_dir,
            days,
            class_filter,
            feed,
        } => {
            cli_validate::validate_segments(&segments_dir, &days, class_filter, feed, &adsb_cache)?
        }
        Cmd::Audit {
            prepared_year_dir,
            segments_by_square,
        } => cli_audit::audit_prepared(&prepared_year_dir, &segments_by_square)?,
        Cmd::Shuffle {
            segments_dir,
            ga_segments_dir,
            ga_adsb_cache,
            out_dir,
            scope_bbox,
        } => cli_runners::run_subcmd_shuffle(
            segments_dir,
            ga_segments_dir,
            ga_adsb_cache,
            out_dir,
            scope_bbox,
        )?,
        Cmd::RunAll {
            adsb_cache,
            prepared_year_dir,
            prepared_dir,
            work_dir,
            segments_dir,
            days,
            scope_bbox,
            from_stage,
            until_stage,
            feed,
            class_filter,
            ga_segments_dir,
            ga_adsb_cache,
            fail_on_ga_cruise,
        } => cli_run_all::run_all(
            adsb_cache,
            prepared_year_dir,
            prepared_dir,
            work_dir,
            days,
            scope_bbox,
            from_stage,
            until_stage,
            feed,
            class_filter,
            ga_segments_dir,
            ga_adsb_cache,
            fail_on_ga_cruise,
            segments_dir,
        )?,
    }
    Ok(())
}

fn from_stage_name(from_stage: FromStage) -> &'static str {
    match from_stage {
        FromStage::Stage0 => "stage0",
        FromStage::Stage1 => "stage1",
        FromStage::Shuffle => "shuffle",
        FromStage::Stage1_5 => "stage1-5",
        FromStage::Stage2a => "stage2a",
        FromStage::Stage2b => "stage2b",
        FromStage::Stage2c => "stage2c",
    }
}

fn init_rayon_pool(max_threads: Option<NonZeroUsize>) -> Result<()> {
    let Some(n) = max_threads else { return Ok(()) };
    rayon::ThreadPoolBuilder::new()
        .num_threads(n.get())
        .build_global()?;
    eprintln!("{} [rayon] global pool = {} threads", ts(), n);
    Ok(())
}

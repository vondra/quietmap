//! Explicit aircraft pipeline inputs; sampling denominators come from the admitted provider days.

use clap::{Parser, Subcommand, ValueEnum};
use std::num::NonZeroUsize;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "aircraft-extract", about = "Aircraft pipeline driver")]
pub struct Cli {
    #[arg(long, global = true)]
    pub max_threads: Option<NonZeroUsize>,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq, PartialOrd, Ord)]
pub enum FromStage {
    Stage0,
    Stage1,
    Shuffle,
    Stage1_5,
    Stage2a,
    Stage2b,
    Stage2c,
}

/// Stage 0/1 peak per concurrent day with every class of both providers.
pub const STAGE01_PEAK_PER_DAY_GB: f64 = 28.0;

#[derive(Subcommand)]
pub enum Cmd {
    /// Count primary cruise transits without producing spill or prepared outputs.
    CruiseCensus {
        #[arg(long, required = true)]
        segments_dir: Vec<PathBuf>,
        #[arg(long)]
        output: PathBuf,
    },
    /// Admit exact source days against the archive cache without reading them.
    PreflightDays {
        #[arg(long)]
        adsb_cache: PathBuf,
        #[arg(long, value_delimiter = ',', required = true)]
        days: Vec<String>,
    },
    /// Verify current prepared schemas and the sampling window before deployment.
    Audit {
        #[arg(long)]
        prepared_year_dir: PathBuf,
        #[arg(long)]
        segments_by_square: PathBuf,
    },
    /// Run one or more stages through the shared provider, window and prerequisite gates.
    RunAll {
        /// Primary provider archive (baseline days); a `catalog.sqlite` there
        /// binds every day to its publisher-verified assets.
        #[arg(long)]
        adsb_cache: PathBuf,
        /// Secondary provider archive, read on increment days only.
        #[arg(long)]
        secondary_adsb_cache: Option<PathBuf>,
        #[arg(long)]
        prepared_year_dir: PathBuf,
        #[arg(long)]
        prepared_dir: PathBuf,
        #[arg(long)]
        work_dir: PathBuf,
        /// Reuse completed day shards in place; allowed only from shuffle onward.
        #[arg(long)]
        segments_dir: Vec<PathBuf>,
        /// Requested sampling days (baseline candidates).
        #[arg(long, value_delimiter = ',')]
        days: Vec<String>,
        /// Requested days that also read the secondary provider (increment candidates).
        #[arg(long, value_delimiter = ',')]
        increment_days: Vec<String>,
        #[arg(long)]
        scope_bbox: Option<String>,
        #[arg(long, value_enum, default_value_t = FromStage::Stage0)]
        from_stage: FromStage,
        #[arg(long, value_enum, default_value_t = FromStage::Stage2c)]
        until_stage: FromStage,
        /// Retain completed raw spill for separately admitted fold/gather work.
        #[arg(long, value_enum, default_value_t = aircraft_extract::stage_2b::CruisePhase::All)]
        cruise_phase: aircraft_extract::stage_2b::CruisePhase,
        /// Reserved net filesystem growth for spill-only work, including its receipt.
        #[arg(long)]
        cruise_spill_disk_budget_bytes: Option<u64>,
    },
}

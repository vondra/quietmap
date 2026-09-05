//! Explicit aircraft pipeline inputs; sampling denominators are derived from validated day sets.

use aircraft_extract::source_adsb_tar::ClassWindowFilter;
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

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Feed {
    Adsblol,
    Adsbexchange,
}

impl Feed {
    pub fn source_id(self) -> u8 {
        use aircraft_extract::flight::source_id;
        match self {
            Feed::Adsblol => source_id::ADSB_LOL_TAR,
            Feed::Adsbexchange => source_id::ADSB_EXCHANGE,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ClassFilterArg {
    All,
    Ga,
    NonGa,
}

impl ClassFilterArg {
    pub fn window(self) -> ClassWindowFilter {
        match self {
            ClassFilterArg::All => ClassWindowFilter::All,
            ClassFilterArg::Ga => ClassWindowFilter::GaOnly,
            ClassFilterArg::NonGa => ClassWindowFilter::NonGa,
        }
    }

    pub fn stage01_peak_per_day_gb(self) -> f64 {
        match self {
            ClassFilterArg::Ga => 6.0,
            ClassFilterArg::All | ClassFilterArg::NonGa => 28.0,
        }
    }
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Check complete expected day files, class routing, provenance, and IPC payloads.
    ValidateSegments {
        #[arg(long)]
        adsb_cache: PathBuf,
        #[arg(long)]
        segments_dir: PathBuf,
        #[arg(long, value_delimiter = ',', required = true)]
        days: Vec<String>,
        #[arg(long, value_enum)]
        class_filter: ClassFilterArg,
        #[arg(long, value_enum)]
        feed: Feed,
    },
    /// Verify current prepared schemas and sampling windows before deployment.
    Audit {
        #[arg(long)]
        prepared_year_dir: PathBuf,
        #[arg(long)]
        segments_by_square: PathBuf,
    },
    Shuffle {
        #[arg(long, required = true)]
        segments_dir: Vec<PathBuf>,
        #[arg(long)]
        ga_segments_dir: Vec<PathBuf>,
        #[arg(long)]
        ga_adsb_cache: Option<PathBuf>,
        #[arg(long)]
        out_dir: PathBuf,
        #[arg(long)]
        scope_bbox: Option<String>,
    },
    /// Run one or more stages through the shared feed, window and prerequisite gates.
    RunAll {
        #[arg(long)]
        adsb_cache: PathBuf,
        #[arg(long)]
        prepared_year_dir: PathBuf,
        #[arg(long)]
        prepared_dir: PathBuf,
        #[arg(long)]
        work_dir: PathBuf,
        #[arg(long, value_delimiter = ',')]
        days: Vec<String>,
        #[arg(long)]
        scope_bbox: Option<String>,
        #[arg(long, value_enum, default_value_t = FromStage::Stage0)]
        from_stage: FromStage,
        #[arg(long, value_enum, default_value_t = FromStage::Stage2c)]
        until_stage: FromStage,
        #[arg(long, value_enum, default_value_t = Feed::Adsblol)]
        feed: Feed,
        #[arg(long, value_enum, default_value_t = ClassFilterArg::All)]
        class_filter: ClassFilterArg,
        #[arg(long)]
        ga_segments_dir: Option<PathBuf>,
        #[arg(long)]
        ga_adsb_cache: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        fail_on_ga_cruise: bool,
    },
}

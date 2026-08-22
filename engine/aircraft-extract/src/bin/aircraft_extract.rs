//! `aircraft-extract` CLI — driver that walks Stage 0..2C end to end
//! per day. Subcommands let an operator re-run any single stage from
//! its persisted input artifact (re-run Stage 1 without re-doing
//! Stage 0; re-run Stage 2A/2B/2C without re-running Stage 1, …).
//!
//! This file is the thin dispatcher: arg surface (`Cli`/`Cmd` +
//! `FromStage`/`Feed`/`ClassFilterArg`), `main`'s `match` over
//! subcommands, and two display/setup helpers. The actual stage bodies
//! live in `cli_runners`; input-validation/path/manifest helpers in
//! `cli_validate`; the Stage 0/1 RAM-concurrency probe in `mem`.

use std::num::NonZeroUsize;
use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};

use aircraft_extract::progress::ts;
use aircraft_extract::source_adsb_tar::ClassWindowFilter;

// Submodules live under `aircraft_extract/` (a subdir, so cargo does not
// auto-discover them as separate bin targets) and are wired with `#[path]`
// since the bin entry is a file (`aircraft_extract.rs`), not `…/main.rs`.
#[path = "aircraft_extract/cli_runners.rs"]
mod cli_runners;
#[path = "aircraft_extract/cli_validate.rs"]
mod cli_validate;
#[path = "aircraft_extract/mem.rs"]
mod mem;

#[derive(Parser)]
#[command(name = "aircraft-extract", about = "Aircraft pipeline driver")]
struct Cli {
    /// Cap rayon's global thread pool. Set when per-task RAM peak
    /// (decoded day ~1.5 GB, worst-R4 working set ~2 GB) × all cores
    /// exceeds host RAM — e.g. `--max-threads 20` on a 90 GB / 24-core
    /// box keeps any one stage below ~80 GB peak. Omit to keep rayon's
    /// default (honours `RAYON_NUM_THREADS` or `available_parallelism()`).
    #[arg(long, global = true)]
    max_threads: Option<NonZeroUsize>,
    #[command(subcommand)]
    cmd: Cmd,
}

/// Entry stage for `run-all`. Variants are declared in pipeline order
/// (Stage 0 → 2C); `PartialOrd`/`Ord` compare by that order so the
/// dispatcher can ask `from_stage <= FromStage::Shuffle` to decide
/// whether to execute each phase. Stage reuse skips every phase whose
/// output already exists on disk under `--work-dir`.
///
/// Required inputs under `--work-dir` per variant (output of an earlier
/// stage that we expect the operator's prior run to have produced):
///
/// | Variant   | flights/ | segments/ | segments_by_r4/ |
/// |-----------|----------|-----------|-----------------|
/// | Stage0    | —        | —         | —               |
/// | Stage1    | yes      | —         | —               |
/// | Shuffle   | —        | yes       | —               |
/// | Stage1_5  | —        | yes       | yes             |
/// | Stage2a   | —        | yes       | yes             |
/// | Stage2b   | —        | yes       | yes             |
/// | Stage2c   | —        | —         | yes             |
#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq, PartialOrd, Ord)]
enum FromStage {
    /// Full pipeline: Stage 0 (ADS-B TAR decode) onward. Default.
    Stage0,
    /// Skip Stage 0; reuse `<work-dir>/flights/<day>.arrow`.
    Stage1,
    /// Skip Stage 0+1; reuse `<work-dir>/segments/<day>.arrow`.
    Shuffle,
    /// Skip Stage 0+1+shuffle; reuse `<work-dir>/{segments,segments_by_r4}/`.
    Stage1_5,
    /// Skip everything before Stage 2A; reuse `<work-dir>/{segments,segments_by_r4}/`.
    Stage2a,
    /// Skip everything before Stage 2B; reuse `<work-dir>/{segments,segments_by_r4}/`.
    /// Stage 2B reads per-day cruise shards from `segments/`; Stage 2C reads `segments_by_r4/`.
    Stage2b,
    /// Skip everything before Stage 2C; reuse `<work-dir>/segments_by_r4/`.
    Stage2c,
}

/// Which ADS-B network the `--adsb-cache` holds. Both ship the identical readsb
/// `trace_full` TAR format, so this only stamps the provenance `source_id` (and
/// dedup identity). The per-feed default cache PATH lives in
/// `run-aircraft-extract.sh`.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum Feed {
    Adsblol,
    Adsbexchange,
}

impl Feed {
    fn source_id(self) -> u8 {
        use aircraft_extract::flight::source_id;
        match self {
            Feed::Adsblol => source_id::ADSB_LOL_TAR,
            Feed::Adsbexchange => source_id::ADSB_EXCHANGE,
        }
    }
}

/// CLI surface (`all|ga|non-ga`) for the hybrid Stage-0 class-window
/// filter. `all` keeps every trace —
/// byte-identical to the pre-hybrid single-window extract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum ClassFilterArg {
    All,
    Ga,
    NonGa,
}

impl ClassFilterArg {
    fn window(self) -> ClassWindowFilter {
        match self {
            ClassFilterArg::All => ClassWindowFilter::All,
            ClassFilterArg::Ga => ClassWindowFilter::GaOnly,
            ClassFilterArg::NonGa => ClassWindowFilter::NonGa,
        }
    }

    /// Stage 0/1 per-day RAM estimate (GB) for `max_concurrent_days`.
    /// GA-filtered days decode to a small fraction of a full day (only
    /// PROP_C172 + HELICOPTER traces survive the prefix probe), so the
    /// full-day 28 GB calibration would throttle the full-year GA pass
    /// to 2 concurrent days for no RAM benefit
    fn stage01_peak_per_day_gb(self) -> f64 {
        match self {
            ClassFilterArg::Ga => 6.0,
            ClassFilterArg::All | ClassFilterArg::NonGa => 28.0,
        }
    }
}

#[derive(Subcommand)]
enum Cmd {
    /// Stage 0: ADS-B TAR → flights/<day>.arrow
    Stage0 {
        #[arg(long)]
        adsb_cache: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        day: String,
        /// Hybrid class-window pass.
        #[arg(long, value_enum, default_value_t = ClassFilterArg::All)]
        class_filter: ClassFilterArg,
    },
    /// Stage 1: flights → segments/<day>.arrow
    Stage1 {
        #[arg(long)]
        flights_dir: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        day: String,
        #[arg(long)]
        prepared_dir: PathBuf,
    },
    /// Shuffle: segments/<day>.arrow → per-R4 airborne/ground shards
    Shuffle {
        /// Dir(s) containing Stage 1's `segments/<day>.arrow` outputs
        /// (repeatable; the primary/airline window — counted into the
        /// `n_days` manifest).
        #[arg(long, required = true)]
        segments_dir: Vec<PathBuf>,
        /// GA-pass `segments/<day>.arrow` dir(s) for hybrid extracts —
        /// shuffled under a distinct pass key and counted into the
        /// `ga_n_days` manifest.
        #[arg(long)]
        ga_segments_dir: Vec<PathBuf>,
        /// Output dir for `<R4>/{airborne,ground}.arrow` per-R4 shards.
        #[arg(long)]
        out_dir: PathBuf,
        #[arg(long)]
        scope_bbox: Option<String>,
    },
    /// Stage 1.5: per-R4 airfield discovery — DBSCAN-clusters ground
    /// vertices that don't snap to existing OSM aeroway lines, writes
    /// `<R4>/synth_airport_lines.arrow` + `synth_airport_areas.arrow`.
    /// Consumed by Stage 2C; runs BEFORE Stage 2A in the orchestrator.
    #[command(name = "stage1-5")]
    Stage1_5 {
        /// Dir containing the shuffle output `<R4>/ground.arrow`.
        #[arg(long)]
        segments_by_r4: PathBuf,
        /// h3r4 dir holding the global `airport_areas.arrow` /
        /// `airport_lines.arrow` (same dir written by `osm-extract`).
        #[arg(long)]
        h3r4_dir: PathBuf,
        #[arg(long)]
        scope_bbox: Option<String>,
    },
    /// Stage 2A: per-R4 airborne shards → per-R4 airborne.arrow
    Stage2a {
        /// Dir containing the shuffle output `<R4>/airborne.arrow`.
        #[arg(long)]
        segments_by_r4: PathBuf,
        #[arg(long)]
        h3r4_dir: PathBuf,
        /// `data/prepared` root — Stage 2A samples per-sub-segment
        /// midpoint DEM elevation here (v15 Opt A). Same path Stage 1
        /// receives; reuse to share the tile cache when calling stages
        /// in sequence (`run-all` does this implicitly).
        #[arg(long)]
        prepared_dir: PathBuf,
        #[arg(long, default_value_t = 1)]
        n_days: u16,
        /// Optional `min_lat,min_lon,max_lat,max_lon` bbox — required
        /// when the upstream segments came from a bbox/radius subset
        /// cache. See `RunAll::scope_bbox` for full rationale.
        #[arg(long)]
        scope_bbox: Option<String>,
    },
    /// Stage 2B: per-day segments shards → per-R4 cruise.arrow
    Stage2b {
        /// Dir containing Stage 1's `segments/<day>.arrow` outputs.
        #[arg(long)]
        segments_dir: PathBuf,
        #[arg(long)]
        h3r4_dir: PathBuf,
        #[arg(long, default_value_t = 1)]
        n_days: u16,
        #[arg(long)]
        scope_bbox: Option<String>,
        /// Hard-fail when GA-class segments reach cruise (default: warn only).
        #[arg(long, default_value_t = false)]
        fail_on_ga_cruise: bool,
    },
    /// Stage 2C: per-R4 ground shards → per-R4 airport_traffic.arrow
    Stage2c {
        /// Dir containing the shuffle output `<R4>/ground.arrow`.
        #[arg(long)]
        segments_by_r4: PathBuf,
        #[arg(long)]
        h3r4_dir: PathBuf,
        #[arg(long, default_value_t = 1)]
        n_days: u16,
        #[arg(long)]
        scope_bbox: Option<String>,
    },
    /// Run every stage end-to-end for a list of days. REFUSES to start
    /// when `--work-dir` already contains populated stage artifacts
    /// from a previous run unless the operator passes `--from-stage`
    /// explicitly — the safety check exists because re-running the
    /// orchestrator with default `--from-stage stage0` would silently
    /// overwrite 1-3 hours of cached upstream work that the operator
    /// almost certainly meant to reuse. Prefer the per-stage
    /// subcommands (`stage0`, `stage1`, `shuffle`, `stage1-5`,
    /// `stage2a`, `stage2b`, `stage2c`) when iterating on one stage,
    /// and pass `--from-stage stageX` to keep using the orchestrator
    /// from a chosen entry point.
    RunAll {
        #[arg(long)]
        adsb_cache: PathBuf,
        #[arg(long)]
        h3r4_dir: PathBuf,
        #[arg(long)]
        prepared_dir: PathBuf,
        /// Working directory for intermediate Stage 0/1 artifacts.
        #[arg(long)]
        work_dir: PathBuf,
        #[arg(long, value_delimiter = ',')]
        days: Vec<String>,
        /// Optional `min_lat,min_lon,max_lat,max_lon` bbox.
        /// Required when `--adsb-cache` points at a bbox/radius
        /// subset (Canary, Praha-150km, etc.) — those caches keep
        /// full daily traces for any flight that entered the
        /// filter, so without scope filtering Stage 2A/2B/2C would
        /// overwrite global R4 files with those out-of-scope
        /// trajectories.
        #[arg(long)]
        scope_bbox: Option<String>,
        /// Skip every phase before `<stage>` and reuse its persisted
        /// input artifact from `--work-dir`. One of:
        /// `stage0` (default — full pipeline), `stage1`, `shuffle`,
        /// `stage1-5`, `stage2a`, `stage2b`, `stage2c`. Use after an
        /// earlier `run-all` populated `--work-dir` to iterate on a
        /// downstream stage without re-running upstream work.
        #[arg(long, value_enum, default_value_t = FromStage::Stage0)]
        from_stage: FromStage,
        /// Stop after the named stage (inclusive; default `stage2c` =
        /// run to the end). The hybrid flow's per-pass invocations end
        /// at `--until-stage stage1`; a later merge invocation resumes
        /// with `--from-stage shuffle`.
        #[arg(long, value_enum, default_value_t = FromStage::Stage2c)]
        until_stage: FromStage,
        /// Which network `--adsb-cache` holds; stamps the provenance source_id
        /// (identical TAR format either way).
        #[arg(long, value_enum, default_value_t = Feed::Adsblol)]
        feed: Feed,
        /// Hybrid class-window pass for Stage 0 ingest: `ga` keeps only
        /// the full-year-sampled GA/heli classes, `non-ga` the complement
        /// (incl. GSE). Default `all` = byte-identical single-window
        /// extract.
        #[arg(long, value_enum, default_value_t = ClassFilterArg::All)]
        class_filter: ClassFilterArg,
        /// Hybrid merge: the GA pass's `segments/` dir (per-day Stage 1
        /// shards). Shuffle unions both windows and writes the
        /// `ga_n_days` manifest next to `n_days`.
        #[arg(long)]
        ga_segments_dir: Option<PathBuf>,
        /// Hard-fail when GA-class segments reach Stage 2B / cruise
        /// (default: warn only).
        #[arg(long, default_value_t = false)]
        fail_on_ga_cruise: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_rayon_pool(cli.max_threads)?;
    match cli.cmd {
        Cmd::Stage0 {
            adsb_cache,
            out,
            day,
            class_filter,
        } => cli_runners::run_subcmd_stage0(adsb_cache, out, day, class_filter)?,
        Cmd::Stage1 {
            flights_dir,
            out,
            day,
            prepared_dir,
        } => cli_runners::run_subcmd_stage1(flights_dir, out, day, prepared_dir)?,
        Cmd::Shuffle {
            segments_dir,
            ga_segments_dir,
            out_dir,
            scope_bbox,
        } => cli_runners::run_subcmd_shuffle(segments_dir, ga_segments_dir, out_dir, scope_bbox)?,
        Cmd::Stage1_5 {
            segments_by_r4,
            h3r4_dir,
            scope_bbox,
        } => cli_runners::run_subcmd_stage1_5(segments_by_r4, h3r4_dir, scope_bbox)?,
        Cmd::Stage2a {
            segments_by_r4,
            h3r4_dir,
            prepared_dir,
            n_days,
            scope_bbox,
        } => cli_runners::run_subcmd_stage2a(
            segments_by_r4,
            h3r4_dir,
            prepared_dir,
            n_days,
            scope_bbox,
        )?,
        Cmd::Stage2b {
            segments_dir,
            h3r4_dir,
            n_days,
            scope_bbox,
            fail_on_ga_cruise,
        } => cli_runners::run_subcmd_stage2b(
            segments_dir,
            h3r4_dir,
            n_days,
            scope_bbox,
            fail_on_ga_cruise,
        )?,
        Cmd::Stage2c {
            segments_by_r4,
            h3r4_dir,
            n_days,
            scope_bbox,
        } => cli_runners::run_subcmd_stage2c(segments_by_r4, h3r4_dir, n_days, scope_bbox)?,
        Cmd::RunAll {
            adsb_cache,
            h3r4_dir,
            prepared_dir,
            work_dir,
            days,
            scope_bbox,
            from_stage,
            until_stage,
            feed,
            class_filter,
            ga_segments_dir,
            fail_on_ga_cruise,
        } => cli_runners::run_all(
            adsb_cache,
            h3r4_dir,
            prepared_dir,
            work_dir,
            days,
            scope_bbox,
            from_stage,
            until_stage,
            feed,
            class_filter,
            ga_segments_dir,
            fail_on_ga_cruise,
        )?,
    }
    Ok(())
}

/// Render `FromStage` using its clap-side CLI value (`stage1-5`),
/// not the Rust variant name (`Stage1_5`). Used in operator-facing
/// log lines so the displayed value matches what they passed on the
/// command line.
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

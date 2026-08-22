//! Stage-dispatch bodies for the `aircraft-extract` bin. The bin's
//! `main` (`aircraft_extract.rs`) parses args then hands each subcommand
//! here; this module holds the per-subcommand runners plus the `run-all`
//! orchestrator. `run_all` keeps the pipeline's stage ORDER, the
//! `[from_stage, until_stage]` gating window, and every until-stage early
//! return verbatim — the individual phase bodies are factored into the
//! `run_stage_*` helpers below, but the control flow that sequences and
//! gates them stays in `run_all` so the CLI's behavior is unchanged.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};

use aircraft_extract::airport_index::AerodromeIndex;
use aircraft_extract::airport_io::{
    read_global_airport_lines, read_global_airports, AirportLineRow,
};
use aircraft_extract::progress::ts;
use aircraft_extract::scope::ScopeBbox;
use aircraft_extract::source::FlightSource;
use aircraft_extract::source_adsb_tar::AdsbTarSource;
use aircraft_extract::stage_0::run_stage_0;
use aircraft_extract::stage_1::run_stage_1;
use aircraft_extract::stage_2a::run_stage_2a;
use aircraft_extract::stage_2b::run_stage_2b;
use aircraft_extract::stage_2c::run_stage_2c;
use aircraft_extract::stage_airport_discover_runner::run_stage_airport_discover;
use noise_compute::types::AirportArea;
use raster_reader::RealRasters;
use rayon::iter::Either;
use rayon::prelude::*;

use super::{from_stage_name, ClassFilterArg, Feed, FromStage};
use crate::cli_validate::{
    bail_on_populated_work_dir, list_segments_day_paths, list_segments_day_paths_multi,
    parse_scope, read_ga_n_days, read_window_n_days, require_input_dir_exists,
    require_scope_for_subset_cache,
};
use crate::mem::max_concurrent_days;

// ── Per-stage subcommands (each maps a `Cmd` arm to a standalone run) ──

/// `Cmd::Stage0` — ADS-B TAR decode → `flights/<day>.arrow`.
pub fn run_subcmd_stage0(
    adsb_cache: PathBuf,
    out: PathBuf,
    day: String,
    class_filter: ClassFilterArg,
) -> Result<()> {
    std::fs::create_dir_all(&out)?;
    let sources: Vec<Box<dyn FlightSource>> = vec![Box::new(
        AdsbTarSource::new(adsb_cache).with_class_filter(class_filter.window()),
    )];
    let n = run_stage_0(&sources, &day, &out)?;
    eprintln!("{} [stage0] {day}: {n} flights", ts());
    Ok(())
}

/// `Cmd::Stage1` — `flights/<day>.arrow` → `segments/<day>.arrow`.
pub fn run_subcmd_stage1(
    flights_dir: PathBuf,
    out: PathBuf,
    day: String,
    prepared_dir: PathBuf,
) -> Result<()> {
    std::fs::create_dir_all(&out)?;
    let rasters = RealRasters::new(&prepared_dir);
    let n = run_stage_1(&flights_dir, &out, &day, &rasters)?;
    eprintln!("{} [stage1] {day}: {n} segments", ts());
    Ok(())
}

/// `Cmd::Shuffle` — per-day `segments/` (airline + GA) → per-R4
/// `<R4>/{airborne,ground}.arrow` shards.
pub fn run_subcmd_shuffle(
    segments_dir: Vec<PathBuf>,
    ga_segments_dir: Vec<PathBuf>,
    out_dir: PathBuf,
    scope_bbox: Option<String>,
) -> Result<()> {
    let scope = parse_scope(scope_bbox.as_deref())?;
    let day_paths = list_segments_day_paths_multi(&segments_dir)?;
    let ga_day_paths = list_segments_day_paths_multi(&ga_segments_dir)?;
    aircraft_extract::shuffle::shuffle_per_r4(&day_paths, &ga_day_paths, &out_dir, scope.as_ref())?;
    eprintln!(
        "{} [shuffle] {} airline + {} GA day shards → {}",
        ts(),
        day_paths.len(),
        ga_day_paths.len(),
        out_dir.display()
    );
    Ok(())
}

/// `Cmd::Stage1_5` — per-R4 airfield discovery (DBSCAN over ground
/// vertices that miss OSM aeroway lines).
pub fn run_subcmd_stage1_5(
    segments_by_r4: PathBuf,
    h3r4_dir: PathBuf,
    scope_bbox: Option<String>,
) -> Result<()> {
    let scope = parse_scope(scope_bbox.as_deref())?;
    require_input_dir_exists("--segments-by-r4", &segments_by_r4)?;
    let areas = read_global_airports(&h3r4_dir)
        .with_context(|| format!("read airport_areas.arrow from {}", h3r4_dir.display()))?;
    let lines = read_global_airport_lines(&h3r4_dir)
        .with_context(|| format!("read airport_lines.arrow from {}", h3r4_dir.display()))?;
    eprintln!(
        "{} [stage1.5] loaded {} aerodromes + {} airport lines globally",
        ts(),
        areas.len(),
        lines.len()
    );
    let aerodrome_index = AerodromeIndex::build(&areas);
    let n = run_stage_airport_discover(
        &segments_by_r4,
        &aerodrome_index,
        &lines,
        &h3r4_dir,
        scope.as_ref(),
    )?;
    eprintln!(
        "{} [stage1.5] {n} R4s populated with synth airport_lines",
        ts()
    );
    Ok(())
}

/// `Cmd::Stage2a` — per-R4 airborne shards → per-R4 `airborne.arrow`.
pub fn run_subcmd_stage2a(
    segments_by_r4: PathBuf,
    h3r4_dir: PathBuf,
    prepared_dir: PathBuf,
    n_days: u16,
    scope_bbox: Option<String>,
) -> Result<()> {
    let scope = parse_scope(scope_bbox.as_deref())?;
    require_input_dir_exists("--segments-by-r4", &segments_by_r4)?;
    let rasters = RealRasters::new(&prepared_dir);
    // GA-class window from the shuffle manifest (0 = single-window).
    let ga_n_days = read_ga_n_days(&segments_by_r4)?;
    let n = run_stage_2a(
        &segments_by_r4,
        &h3r4_dir,
        n_days,
        ga_n_days,
        scope.as_ref(),
        &rasters,
    )?;
    eprintln!("{} [stage2a] {n} R4 hexes written", ts());
    Ok(())
}

/// `Cmd::Stage2b` — per-day segments shards → per-R4 `cruise.arrow`.
pub fn run_subcmd_stage2b(
    segments_dir: PathBuf,
    h3r4_dir: PathBuf,
    n_days: u16,
    scope_bbox: Option<String>,
    fail_on_ga_cruise: bool,
) -> Result<()> {
    let scope = parse_scope(scope_bbox.as_deref())?;
    require_input_dir_exists("--segments-dir", &segments_dir)?;
    let day_paths = list_segments_day_paths(&segments_dir)?;
    let n = run_stage_2b(
        &day_paths,
        &h3r4_dir,
        n_days,
        scope.as_ref(),
        fail_on_ga_cruise,
    )?;
    eprintln!("{} [stage2b] {n} R4 hexes written", ts());
    Ok(())
}

/// `Cmd::Stage2c` — per-R4 ground shards → per-R4 `airport_traffic.arrow`.
pub fn run_subcmd_stage2c(
    segments_by_r4: PathBuf,
    h3r4_dir: PathBuf,
    n_days: u16,
    scope_bbox: Option<String>,
) -> Result<()> {
    let scope = parse_scope(scope_bbox.as_deref())?;
    require_input_dir_exists("--segments-by-r4", &segments_by_r4)?;
    let areas = read_global_airports(&h3r4_dir)
        .with_context(|| format!("read airport_areas.arrow from {}", h3r4_dir.display()))?;
    eprintln!(
        "{} [stage2c] loaded {} aerodrome polygons globally",
        ts(),
        areas.len()
    );
    let ga_n_days = read_ga_n_days(&segments_by_r4)?;
    let n = run_stage_2c(
        &segments_by_r4,
        &areas,
        &h3r4_dir,
        n_days,
        ga_n_days,
        scope.as_ref(),
    )?;
    eprintln!("{} [stage2c] {n} R4 hexes written", ts());
    Ok(())
}

// ── `run-all` orchestrator ──

/// `Cmd::RunAll` — run every stage end-to-end for a list of days.
///
/// This keeps the dispatcher's stage ORDER, the `[from_stage,
/// until_stage]` gating window (`runs(stage)`), and every until-stage
/// early `return Ok(())` exactly as the inline `main` arm had them; the
/// per-phase work is delegated to the `run_stage_*` helpers below. See
/// the `Cmd::RunAll` doc comment in `aircraft_extract.rs` for the full
/// safety rationale of each guard.
#[allow(clippy::too_many_arguments)]
pub fn run_all(
    adsb_cache: PathBuf,
    h3r4_dir: PathBuf,
    prepared_dir: PathBuf,
    work_dir: PathBuf,
    days: Vec<String>,
    scope_bbox: Option<String>,
    from_stage: FromStage,
    until_stage: FromStage,
    feed: Feed,
    class_filter: ClassFilterArg,
    ga_segments_dir: Option<PathBuf>,
    fail_on_ga_cruise: bool,
) -> Result<()> {
    let scope = parse_scope(scope_bbox.as_deref())?;
    require_scope_for_subset_cache(&adsb_cache, scope.as_ref())?;
    if until_stage < from_stage {
        anyhow::bail!(
            "--until-stage {} precedes --from-stage {} — nothing would run",
            from_stage_name(until_stage),
            from_stage_name(from_stage),
        );
    }
    // Whether a pipeline phase executes this invocation: inside
    // the [from_stage, until_stage] window (both inclusive).
    let runs = |stage: FromStage| from_stage <= stage && stage <= until_stage;
    // Without any days, run-all would emit zero ok_paths and
    // proceed to shuffle, which wipes `segments_by_r4/` before
    // writing nothing. Bail loud to catch the typo / forgot-
    // --days case before shuffle erases existing cache.
    if days.is_empty() {
        anyhow::bail!(
            "--days is empty — refusing to start. Pass at least one day, \
             e.g. `--days 2025-01-01` or comma-separated list",
        );
    }
    // Refuse to silently overwrite a populated work_dir when
    // `--from-stage` resolves to stage0 (default OR explicit).
    // If the operator passed a later entry point, the populated
    // dir IS the input they want to reuse and we proceed.
    // Clean full run requires `rm -rf $WORK_DIR` first — data
    // loss must be intentional, not accidental.
    if from_stage == FromStage::Stage0 {
        bail_on_populated_work_dir(&work_dir)?;
    }
    if let Some(s) = scope.as_ref() {
        eprintln!(
            "{} [run-all] scope bbox: lat {}..{}, lon {}..{}",
            ts(),
            s.min_lat,
            s.max_lat,
            s.min_lon,
            s.max_lon
        );
    }
    if from_stage != FromStage::Stage0 {
        let name = from_stage_name(from_stage);
        eprintln!(
            "{} [run-all] --from-stage {name}: skipping every phase before {name}",
            ts()
        );
    }
    if until_stage != FromStage::Stage2c {
        let name = from_stage_name(until_stage);
        eprintln!(
            "{} [run-all] --until-stage {name}: stopping after {name}",
            ts()
        );
    }
    if class_filter != ClassFilterArg::All {
        eprintln!(
            "{} [run-all] --class-filter {:?}: Stage 0 ingests only that \
             hybrid window's classes (ga-365d-hybrid-plan.md §3)",
            ts(),
            class_filter
        );
    }
    let flights_dir = work_dir.join("flights");
    let segments_dir = work_dir.join("segments");
    let by_r4_dir = work_dir.join("segments_by_r4");

    // Hybrid merge input — resolved up front so a wrong path
    // fails before hours of Stage 0/1, and only when shuffle
    // actually consumes it.
    let ga_day_paths: Vec<PathBuf> = match &ga_segments_dir {
        None => Vec::new(),
        Some(_) if !runs(FromStage::Shuffle) => {
            eprintln!(
                "{} [run-all] --ga-segments-dir ignored: shuffle is outside \
                 the from/until window, segments_by_r4 + its manifests are \
                 reused as-is",
                ts()
            );
            Vec::new()
        }
        Some(dir) => {
            require_input_dir_exists("--ga-segments-dir", dir)?;
            // Same physical dir as the airline segments would feed
            // every airline day into BOTH passes (double energy
            // under two pass keys) — refuse.
            if dir
                .canonicalize()
                .ok()
                .is_some_and(|ga| segments_dir.canonicalize().ok() == Some(ga))
            {
                anyhow::bail!(
                    "--ga-segments-dir {} is the airline segments dir itself; \
                     point it at the GA pass's work dir (e.g. <ga-work>/segments)",
                    dir.display()
                );
            }
            let paths = list_segments_day_paths(dir)?;
            if paths.is_empty() {
                anyhow::bail!(
                    "--ga-segments-dir {} contains no .arrow day shards — did \
                     the GA pass (--class-filter ga --until-stage stage1) run?",
                    dir.display()
                );
            }
            eprintln!(
                "{} [run-all] hybrid merge: {} GA day shard(s) from {}",
                ts(),
                paths.len(),
                dir.display()
            );
            paths
        }
    };

    // Dedup before par_iter — Stage 0/1 write fixed paths
    // (flights/<day>.arrow, segments/<day>.arrow) per day, so
    // duplicates would race on the same output file.
    let mut days_dedup = days.clone();
    days_dedup.sort();
    days_dedup.dedup();
    if days_dedup.len() != days.len() {
        eprintln!(
            "{} [run-all] duplicate days dropped: {} → {} unique",
            ts(),
            days.len(),
            days_dedup.len()
        );
    }
    let days = days_dedup;

    // Shared instance — TileStore is per-slot Mutex +
    // Arc<RawTile>, safe to fan out under par_iter.
    let rasters = RealRasters::new(&prepared_dir);

    // Per-day phase. `ok_paths` is the input list for both
    // `shuffle_per_r4` and `run_stage_2b`; either consumer empty
    // means the corresponding output is silently wiped, so we
    // must populate it for every variant that still runs them.
    // After this block, Stage 2B (when it runs) gets a non-empty
    // list — or we fail loud before nuking anything.
    let ok_paths = compute_ok_paths(
        &days,
        &adsb_cache,
        &work_dir,
        &flights_dir,
        &segments_dir,
        &rasters,
        from_stage,
        until_stage,
        feed,
        class_filter,
        &runs,
    )?;

    if until_stage <= FromStage::Stage1 {
        eprintln!(
            "{} [run-all] stopped after {} (--until-stage): {} day artifact(s) \
             under {}",
            ts(),
            from_stage_name(until_stage),
            ok_paths.len(),
            work_dir.display()
        );
        return Ok(());
    }

    // Read the global aerodrome set once. Stage 1.5
    // (`run_stage_airport_discover`) uses it for the
    // polygon-radius-aware re-attribution / reject pass on
    // DBSCAN clusters; Stage 2C reuses the same vec for its
    // `nearest_aerodrome_within` resolver. Airport identity
    // must stay global — aerodromes straddle R4 boundaries.
    // Skipped entirely when neither consumer is in the
    // from/until window.
    let (areas, global_lines) = load_global_airports(&h3r4_dir, &runs)?;

    // Shuffle Stage 1 per-day shards into per-R4
    // `segments_by_r4/<R4>/{airborne,ground}.arrow`. Stages 1.5
    // / 2A / 2C all consume these; Stage 2B reads the per-day
    // shards directly (cruise straddles R4 boundaries) plus the
    // `n_days` manifest written here. The GA-window shards merge
    // into the same per-R4 pool under a distinct pass key.
    if runs(FromStage::Shuffle) {
        run_stage_shuffle(&ok_paths, &ga_day_paths, &by_r4_dir, scope.as_ref())?;
    } else {
        // Stages 1.5 / 2A / 2C read `<by_r4_dir>/<R4>/…`, Stage 2B
        // its `n_days` manifest; fail loud before any of them
        // silently no-ops on a missing dir.
        require_input_dir_exists(
            "--work-dir/segments_by_r4 (required by Stage 1.5 / 2A / 2B / 2C)",
            &by_r4_dir,
        )?;
    }
    if until_stage <= FromStage::Shuffle {
        eprintln!(
            "{} [run-all] stopped after shuffle (--until-stage): per-R4 shards in {}",
            ts(),
            by_r4_dir.display()
        );
        return Ok(());
    }

    // Stage 1.5 — DBSCAN auto-discovery of OSM-missing
    // airfields. Runs BEFORE Stage 2C so its synth sidecars
    // are visible when Stage 2C loads each R4's airport_lines
    // cache. Writes empty arrows for in-scope R4s with no
    // current clusters so a stale strip cannot leak through.
    if runs(FromStage::Stage1_5) {
        run_stage_1_5(&by_r4_dir, &areas, &global_lines, &h3r4_dir, scope.as_ref())?;
    }
    if until_stage <= FromStage::Stage1_5 {
        eprintln!("{} [run-all] stopped after stage1-5 (--until-stage)", ts());
        return Ok(());
    }

    // n_days for Lden, from the shuffle's day-count manifest (see
    // `read_window_n_days`). All three Stage-2 outputs stamp this
    // one window, so the popup's max-n_days collapse
    // (source-reader/lib.rs:150) stays consistent across co-loaded
    // layers and can't drift to the 2026-05-24 n_days=7-on-full-year
    // mislabel (~17 dB off).
    let window_n_days = read_window_n_days(&by_r4_dir)?;
    let ga_n_days = read_ga_n_days(&by_r4_dir)?;
    if ga_n_days > 0 {
        eprintln!(
            "{} [run-all] hybrid windows: n_days={window_n_days} (airline) + \
             ga_n_days={ga_n_days} (GA classes, ga-365d-hybrid-plan.md)",
            ts()
        );
    }

    if runs(FromStage::Stage2a) {
        run_stage_2a_phase(
            &by_r4_dir,
            &h3r4_dir,
            window_n_days,
            ga_n_days,
            scope.as_ref(),
            &rasters,
        )?;
    }
    if until_stage <= FromStage::Stage2a {
        eprintln!("{} [run-all] stopped after stage2a (--until-stage)", ts());
        return Ok(());
    }

    if runs(FromStage::Stage2b) {
        run_stage_2b_phase(
            &ok_paths,
            &h3r4_dir,
            window_n_days,
            scope.as_ref(),
            fail_on_ga_cruise,
        )?;
    }
    if until_stage <= FromStage::Stage2b {
        eprintln!("{} [run-all] stopped after stage2b (--until-stage)", ts());
        return Ok(());
    }

    // Stage 2C is the last stage; reaching here means
    // `until_stage == Stage2c`, so it always runs.
    run_stage_2c_phase(
        &by_r4_dir,
        &areas,
        &h3r4_dir,
        window_n_days,
        ga_n_days,
        scope.as_ref(),
    )?;

    // `by_r4_dir` is left on disk so `--from-stage stage1-5/2a/2c`
    // can iterate on the same scratch dir without re-running
    // shuffle. The next shuffle wipes it before recreating;
    // operators clear `--work-dir` manually between major runs.
    Ok(())
}

/// Stage 0/1 per-day phase of `run-all`: returns the per-day artifact
/// list (`ok_paths`) that feeds shuffle and Stage 2B. Either runs Stage
/// 0/1 for every requested day (`from_stage <= Stage1`), reuses the
/// `--days`-matching shards already in `segments_dir` (a later
/// `--from-stage` that still needs the list), or returns empty (Stage 2C
/// only, where `ok_paths` is unused). Bails loud rather than returning an
/// empty list when a consumer would otherwise wipe cached output.
#[allow(clippy::too_many_arguments)]
fn compute_ok_paths(
    days: &[String],
    adsb_cache: &Path,
    work_dir: &Path,
    flights_dir: &Path,
    segments_dir: &Path,
    rasters: &RealRasters,
    from_stage: FromStage,
    until_stage: FromStage,
    feed: Feed,
    class_filter: ClassFilterArg,
    runs: &impl Fn(FromStage) -> bool,
) -> Result<Vec<PathBuf>> {
    let needs_ok_paths = runs(FromStage::Shuffle) || runs(FromStage::Stage2b);
    let ok_paths: Vec<PathBuf> = if from_stage <= FromStage::Stage1 {
        std::fs::create_dir_all(flights_dir)?;
        std::fs::create_dir_all(segments_dir)?;
        let sources: Vec<Box<dyn FlightSource>> = vec![Box::new(
            AdsbTarSource::new(adsb_cache)
                .with_source_id(feed.source_id())
                .with_class_filter(class_filter.window()),
        )];
        // Per-day error tolerance: one corrupted TAR or DEM miss
        // must not throw away the other days' Stage 0+1 work.
        // Failed days are listed at the end so the operator can
        // rerun with `--days <failed,…>`.
        // Process days in RAM-bounded chunks. Each concurrent day holds
        // its full flight + segment working set in memory; within-day
        // work is already rayon-parallel over flights, so even one chunk
        // saturates every core. Running ALL days at once only multiplies
        // RAM with no throughput gain — it OOM-killed the 2026-05 global
        // TTM extract (7 dense days ≈ 16 GB each > 110 GB cgroup).
        let max_concurrent =
            max_concurrent_days(days.len(), class_filter.stage01_peak_per_day_gb());
        eprintln!(
            "{} [run-all] Stage 0/1: {} day(s), <={} concurrent (RAM-bounded; within-day fills every core)",
            ts(),
            days.len(),
            max_concurrent
        );
        // A run stopping at Stage 0 produces flights, not
        // segments — success is judged on the artifact the last
        // executed stage writes.
        let done_dir = if until_stage == FromStage::Stage0 {
            flights_dir
        } else {
            segments_dir
        };
        let mut ok_paths: Vec<PathBuf> = Vec::new();
        let mut failed_days: Vec<String> = Vec::new();
        for chunk in days.chunks(max_concurrent) {
            let (mut ok, mut fail): (Vec<PathBuf>, Vec<String>) =
                chunk.par_iter().partition_map(|day| {
                    let done_path = done_dir.join(format!("{day}.arrow"));
                    match run_day(
                        day,
                        &sources,
                        flights_dir,
                        segments_dir,
                        rasters,
                        from_stage,
                        until_stage,
                    ) {
                        Ok(()) if done_path.exists() => Either::Left(done_path),
                        Ok(()) => {
                            eprintln!("{} [run-all] {day}: FAILED — no output file produced", ts());
                            Either::Right(day.clone())
                        }
                        Err(e) => {
                            eprintln!("{} [run-all] {day}: FAILED stage0/1 — {e}, skipping", ts());
                            Either::Right(day.clone())
                        }
                    }
                });
            ok_paths.append(&mut ok);
            failed_days.append(&mut fail);
        }

        if !failed_days.is_empty() {
            eprintln!(
                "{} [run-all] {} day(s) failed: {} — Stage 2 runs on the rest; rerun with --days {} to retry",
                ts(),
                failed_days.len(),
                failed_days.join(","),
                failed_days.join(",")
            );
        }
        // Every day failed → ok_paths empty → shuffle would
        // proceed and wipe `segments_by_r4/` before writing
        // nothing, destroying the cached partition from any
        // earlier successful run. Bail loud instead. Same bail
        // when this is a per-pass invocation stopping at Stage
        // 0/1 — an all-failed pass must not look successful.
        if ok_paths.is_empty() {
            return Err(anyhow::anyhow!(
                "every requested day failed Stage 0/1 — nothing produced under \
                 {}. Check upstream errors and rerun with --days <surviving-list>",
                work_dir.display(),
            ));
        }
        ok_paths
    } else if needs_ok_paths {
        // Skipped Stage 0+1; reuse whatever Stage 1 left in
        // `segments_dir`. Filter by requested days so a stale
        // shard from a prior wider run doesn't sneak into
        // Stage 2B. Empty result = previous Stage 1 never ran
        // or `--days` doesn't match the cache; fail loud
        // because Stage 2B would otherwise wipe in-scope
        // `cruise.arrow` then write nothing.
        require_input_dir_exists("--work-dir/segments (--from-stage)", segments_dir)?;
        let requested: std::collections::HashSet<&str> = days.iter().map(String::as_str).collect();
        let mut paths: Vec<PathBuf> = list_segments_day_paths(segments_dir)?
            .into_iter()
            .filter(|p| {
                p.file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| requested.contains(s))
                    .unwrap_or(false)
            })
            .collect();
        paths.sort();
        if paths.is_empty() {
            return Err(anyhow::anyhow!(
                "--work-dir/segments {} contains no shard matching --days; rerun \
                 from an earlier stage or fix --days",
                segments_dir.display()
            ));
        }
        eprintln!(
            "{} [run-all] reusing {} segment shard(s) from {}",
            ts(),
            paths.len(),
            segments_dir.display()
        );
        paths
    } else {
        // Stage 2C only — neither shuffle nor Stage 2B runs,
        // so `ok_paths` is unused.
        Vec::new()
    };
    Ok(ok_paths)
}

/// Load the global aerodrome polygons + airport lines once, when Stage
/// 1.5 or Stage 2C is in the from/until window (else empty). Both stamp a
/// single global airport identity — aerodromes straddle R4 boundaries —
/// and a zero-aerodrome load is a fatal `--h3r4-dir` mistake (Stage 1.5
/// would treat every ground segment as a new-airport candidate).
fn load_global_airports(
    h3r4_dir: &Path,
    runs: &impl Fn(FromStage) -> bool,
) -> Result<(Vec<AirportArea>, Vec<AirportLineRow>)> {
    let needs_airports = runs(FromStage::Stage1_5) || runs(FromStage::Stage2c);
    if needs_airports {
        let areas = read_global_airports(h3r4_dir)?;
        eprintln!(
            "{} [run-all] global aerodromes: {} polygons",
            ts(),
            areas.len()
        );
        // 0 global aerodromes ⇒ the OSM airport data isn't in --h3r4-dir (e.g. an
        // empty staging dir). Both Stage 1.5 gates then no-op → every ground segment
        // becomes a DBSCAN candidate (hours/R4 + garbage). Fail fast; cost a run once.
        if areas.is_empty() {
            anyhow::bail!(
                "0 global aerodromes loaded from {} — the OSM airport_areas.arrow data \
                 is missing there. Stage 1.5 would then treat every ground segment as a \
                 new-airport candidate (DBSCAN over millions of points → hours per R4 + \
                 garbage synth airports). Point --h3r4-dir at a prepared h3r4 that HAS \
                 the OSM airport data, not an empty or staging dir.",
                h3r4_dir.display()
            );
        }
        let global_lines = read_global_airport_lines(h3r4_dir)?;
        eprintln!(
            "{} [run-all] global airport lines: {} microsegments",
            ts(),
            global_lines.len()
        );
        Ok((areas, global_lines))
    } else {
        Ok((Vec::new(), Vec::new()))
    }
}

/// Shuffle phase of `run-all` (the `runs(Shuffle)` branch body): unions
/// the airline + GA per-day shards into the per-R4 pool and writes the
/// day-count manifests.
fn run_stage_shuffle(
    ok_paths: &[PathBuf],
    ga_day_paths: &[PathBuf],
    by_r4_dir: &Path,
    scope: Option<&ScopeBbox>,
) -> Result<()> {
    let t_shuf = Instant::now();
    aircraft_extract::shuffle::shuffle_per_r4(ok_paths, ga_day_paths, by_r4_dir, scope)?;
    eprintln!("{} [run-all] shuffle done ({:?})", ts(), t_shuf.elapsed());
    Ok(())
}

/// Stage 1.5 phase of `run-all` (the `runs(Stage1_5)` branch body):
/// grid-indexes the global aerodromes then runs DBSCAN airfield
/// discovery over the per-R4 ground shards.
fn run_stage_1_5(
    by_r4_dir: &Path,
    areas: &[AirportArea],
    global_lines: &[AirportLineRow],
    h3r4_dir: &Path,
    scope: Option<&ScopeBbox>,
) -> Result<()> {
    let t1_5 = Instant::now();
    // Grid-index the global aerodromes so the per-ground-segment gate is
    // O(few-nearby), not O(45443) — else a mega-hub R4 is ~30 min/core.
    let aerodrome_index = AerodromeIndex::build(areas);
    let r1_5 =
        run_stage_airport_discover(by_r4_dir, &aerodrome_index, global_lines, h3r4_dir, scope)?;
    eprintln!(
        "{} [run-all] stage1.5 R4s with synth lines={r1_5} ({:?})",
        ts(),
        t1_5.elapsed()
    );
    Ok(())
}

/// Stage 2A phase of `run-all` (the `runs(Stage2a)` branch body).
fn run_stage_2a_phase(
    by_r4_dir: &Path,
    h3r4_dir: &Path,
    window_n_days: u16,
    ga_n_days: u16,
    scope: Option<&ScopeBbox>,
    rasters: &RealRasters,
) -> Result<()> {
    let t2a = Instant::now();
    let r2a = run_stage_2a(
        by_r4_dir,
        h3r4_dir,
        window_n_days,
        ga_n_days,
        scope,
        rasters,
    )?;
    eprintln!("{} [run-all] stage2a={r2a} ({:?})", ts(), t2a.elapsed());
    Ok(())
}

/// Stage 2B phase of `run-all` (the `runs(Stage2b)` branch body),
/// including the day-count guard: cruise output R4 derives from each
/// touched R7's parent, so the input window MUST equal the shuffled
/// window it is stamped with.
fn run_stage_2b_phase(
    ok_paths: &[PathBuf],
    h3r4_dir: &Path,
    window_n_days: u16,
    scope: Option<&ScopeBbox>,
    fail_on_ga_cruise: bool,
) -> Result<()> {
    let t2b = Instant::now();
    // Stage 2B reads per-day cruise shards (`ok_paths`), NOT the
    // shuffled per-R4 ones — cruise output R4 derives from each
    // touched R7's parent (`stage_2b.rs:cell.parent(R4)`). Its
    // input window must therefore equal the shuffled window it is
    // stamped with; a `--from-stage` re-run with a shrunk `--days`
    // would aggregate fewer days than `segments_by_r4` holds and
    // mis-normalize cruise (while wiping the full-window files).
    // Refuse it — re-shuffle for the requested days instead.
    //
    // Hybrid runs change nothing here: `ok_paths` is the AIRLINE
    // pass only (GA shards enter solely via --ga-segments-dir →
    // shuffle), so cruise keeps plain `n_days` semantics and this
    // guard still compares airline days to the airline manifest.
    if ok_paths.len() as u16 != window_n_days {
        anyhow::bail!(
            "Stage 2B input is {} day(s) but the shuffled window is {} \
             (segments_by_r4/n_days). Re-run with `--from-stage shuffle` to \
             re-shuffle for the requested --days, or pass the full day set.",
            ok_paths.len(),
            window_n_days,
        );
    }
    let r2b = run_stage_2b(ok_paths, h3r4_dir, window_n_days, scope, fail_on_ga_cruise)?;
    eprintln!("{} [run-all] stage2b={r2b} ({:?})", ts(), t2b.elapsed());
    Ok(())
}

/// Stage 2C phase of `run-all` (the final stage's body — always runs once
/// reached, since arrival implies `until_stage == Stage2c`).
fn run_stage_2c_phase(
    by_r4_dir: &Path,
    areas: &[AirportArea],
    h3r4_dir: &Path,
    window_n_days: u16,
    ga_n_days: u16,
    scope: Option<&ScopeBbox>,
) -> Result<()> {
    let t2c = Instant::now();
    let r2c = run_stage_2c(by_r4_dir, areas, h3r4_dir, window_n_days, ga_n_days, scope)?;
    eprintln!("{} [run-all] stage2c={r2c} ({:?})", ts(), t2c.elapsed());
    Ok(())
}

/// Run Stage 0 + Stage 1 for a single day. Lifted out of `RunAll` so
/// the per-day try/log/continue caller stays a flat `match` instead of
/// a `Result`-returning IIFE. `from_stage` selects whether to execute
/// Stage 0 — when set to `Stage1`, the caller is reusing a populated
/// `flights_dir` from a previous run and we go straight to Stage 1.
/// `until_stage == Stage0` stops before Stage 1 (per-pass hybrid runs
/// that only need the Stage 0 cache).
fn run_day(
    day: &str,
    sources: &[Box<dyn FlightSource>],
    flights_dir: &Path,
    segments_dir: &Path,
    rasters: &RealRasters,
    from_stage: FromStage,
    until_stage: FromStage,
) -> Result<()> {
    let t0 = Instant::now();
    let stage0_log = if from_stage <= FromStage::Stage0 {
        let n0 = run_stage_0(sources, day, flights_dir)?;
        format!("stage0={n0} ({:?})", t0.elapsed())
    } else {
        // Distinguish "skipped" from a real but empty Stage 0 result —
        // the latter has its own diagnostic (no flights for the day).
        "stage0=skipped".to_string()
    };
    if until_stage == FromStage::Stage0 {
        eprintln!(
            "{} [run-all] {day}: {stage0_log} stage1=skipped (--until-stage stage0)",
            ts()
        );
        return Ok(());
    }
    let t_stage1 = Instant::now();
    let n1 = run_stage_1(flights_dir, segments_dir, day, rasters)?;
    eprintln!(
        "{} [run-all] {day}: {stage0_log} stage1={n1} ({:?})",
        ts(),
        t_stage1.elapsed()
    );
    Ok(())
}

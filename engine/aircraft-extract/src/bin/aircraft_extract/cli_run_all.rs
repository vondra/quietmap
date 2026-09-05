//! Ordered aircraft orchestration refuses partial day sets before downstream publication.

use crate::{
    ClassFilterArg, Feed, FromStage, cli_days::compute_ok_paths, cli_validate::*, from_stage_name,
};
use aircraft_extract::{
    airport_index::AerodromeIndex,
    airport_io::{AirportLineRow, read_global_airport_lines, read_global_airports},
    progress::ts,
    stage_2a::run_stage_2a,
    stage_2b::run_stage_2b,
    stage_2c::run_stage_2c,
    stage_airport_discover_runner::run_stage_airport_discover,
};
use anyhow::Result;
use noise_compute::types::AirportArea;
use raster_reader::RealRasters;
use std::path::{Path, PathBuf};

#[allow(clippy::too_many_arguments)]
pub fn run_all(
    adsb_cache: PathBuf,
    prepared_year_dir: PathBuf,
    prepared_dir: PathBuf,
    work_dir: PathBuf,
    days: Vec<String>,
    scope_bbox: Option<String>,
    from_stage: FromStage,
    until_stage: FromStage,
    feed: Feed,
    class_filter: ClassFilterArg,
    ga_segments_dir: Option<PathBuf>,
    ga_adsb_cache: Option<PathBuf>,
    fail_on_ga_cruise: bool,
) -> Result<()> {
    crate::source_cache::validate_ga_merge(
        &ga_segments_dir.iter().cloned().collect::<Vec<_>>(),
        ga_adsb_cache.as_deref(),
    )?;
    let scope = parse_scope(scope_bbox.as_deref())?;
    require_scope_for_subset_cache(&adsb_cache, scope.as_ref())?;
    if until_stage < from_stage {
        anyhow::bail!(
            "--until-stage {} precedes --from-stage {} — nothing would run",
            from_stage_name(until_stage),
            from_stage_name(from_stage),
        );
    }
    let runs = |stage: FromStage| from_stage <= stage && stage <= until_stage;
    if days.is_empty() {
        anyhow::bail!(
            "--days is empty — refusing to start. Pass at least one day, \
             e.g. `--days 2025-01-01` or comma-separated list",
        );
    }
    if from_stage == FromStage::Stage0 {
        let cache = matches!(feed, Feed::Adsblol)
            .then(|| crate::source_cache::SourceCache::new(&adsb_cache, &work_dir, class_filter));
        validate_fresh_stage0_work(&work_dir, &days, until_stage, cache.as_ref())?;
    }
    let days = if matches!(feed, Feed::Adsblol) {
        crate::source_cache::SourceCache::new(&adsb_cache, &work_dir, class_filter)
            .validate(Some(&days), None)?
            .into_keys()
            .collect()
    } else {
        days
    };
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
             hybrid window's classes (class window)",
            ts(),
            class_filter
        );
    }
    let flights_dir = work_dir.join("flights");
    let segments_dir = work_dir.join("segments");
    let by_square_dir = work_dir.join("segments_by_square");
    if from_stage > FromStage::Shuffle && !read_window_days(&by_square_dir, "ga_days")?.is_empty() {
        anyhow::ensure!(
            ga_adsb_cache.is_some(),
            "hybrid stage reuse requires --ga-adsb-cache and --ga-segments-dir"
        );
    }

    let ga_day_paths: Vec<PathBuf> = match &ga_segments_dir {
        None => Vec::new(),
        Some(_) if !runs(FromStage::Shuffle) => {
            eprintln!(
                "{} [run-all] --ga-segments-dir ignored: shuffle is outside \
                 the from/until window, segments_by_square + its manifests are \
                 reused as-is",
                ts()
            );
            Vec::new()
        }
        Some(dir) => {
            require_input_dir_exists("--ga-segments-dir", dir)?;
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
    for day in &days {
        aircraft_extract::period::parse_date_id(day)?;
    }

    let rasters = RealRasters::new(&prepared_dir);

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

    let (areas, global_lines) = load_global_airports(&prepared_year_dir, &runs)?;

    if runs(FromStage::Shuffle) {
        aircraft_extract::shuffle::shuffle_per_square(
            &ok_paths,
            &ga_day_paths,
            &by_square_dir,
            scope.as_ref(),
        )?;
    } else {
        require_input_dir_exists(
            "--work-dir/segments_by_square (required by Stage 1.5 / 2A / 2B / 2C)",
            &by_square_dir,
        )?;
    }
    if until_stage <= FromStage::Shuffle {
        eprintln!(
            "{} [run-all] stopped after shuffle (--until-stage): per-z9 shards in {}",
            ts(),
            by_square_dir.display()
        );
        return Ok(());
    }

    if runs(FromStage::Stage2b) {
        require_matching_window_days(&by_square_dir, &ok_paths)?;
    }

    if runs(FromStage::Stage1_5) {
        run_stage_airport_discover(
            &by_square_dir,
            &AerodromeIndex::build(&areas),
            &global_lines,
            &prepared_year_dir,
            scope.as_ref(),
        )?;
    }
    if until_stage <= FromStage::Stage1_5 {
        eprintln!("{} [run-all] stopped after stage1-5 (--until-stage)", ts());
        return Ok(());
    }

    let window_n_days = read_window_n_days(&by_square_dir)?;
    let ga_n_days = read_ga_n_days(&by_square_dir)?;
    if ga_n_days > 0 {
        eprintln!(
            "{} [run-all] hybrid windows: n_days={window_n_days} (airline) + \
             ga_n_days={ga_n_days} (GA classes)",
            ts()
        );
    }

    if runs(FromStage::Stage2a) {
        run_stage_2a(
            &by_square_dir,
            &prepared_year_dir,
            window_n_days,
            ga_n_days,
            scope.as_ref(),
        )?;
    }
    if until_stage <= FromStage::Stage2a {
        eprintln!("{} [run-all] stopped after stage2a (--until-stage)", ts());
        return Ok(());
    }

    if runs(FromStage::Stage2b) {
        run_stage_2b(
            &ok_paths,
            &prepared_year_dir,
            window_n_days,
            scope.as_ref(),
            fail_on_ga_cruise,
        )?;
    }
    if until_stage <= FromStage::Stage2b {
        eprintln!("{} [run-all] stopped after stage2b (--until-stage)", ts());
        return Ok(());
    }

    run_stage_2c(
        &by_square_dir,
        &areas,
        &prepared_year_dir,
        window_n_days,
        ga_n_days,
        scope.as_ref(),
    )?;

    Ok(())
}

fn load_global_airports(
    prepared_year_dir: &Path,
    runs: &impl Fn(FromStage) -> bool,
) -> Result<(Vec<AirportArea>, Vec<AirportLineRow>)> {
    let needs_airports = runs(FromStage::Stage1_5) || runs(FromStage::Stage2c);
    if needs_airports {
        let areas = read_global_airports(prepared_year_dir)?;
        eprintln!(
            "{} [run-all] global aerodromes: {} polygons",
            ts(),
            areas.len()
        );
        if areas.is_empty() {
            anyhow::bail!(
                "0 global aerodromes loaded from {} — the OSM airport_areas.arrow data \
                 is missing there. Stage 1.5 would then treat every ground segment as a \
                 new-airport candidate (DBSCAN over millions of points → hours per z9 + \
                 garbage synth airports). Point --prepared-year-dir at the prepared year directory that HAS \
                 the OSM airport data, not an empty or staging dir.",
                prepared_year_dir.display()
            );
        }
        let global_lines = read_global_airport_lines(prepared_year_dir)?;
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

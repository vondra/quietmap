//! Ordered aircraft orchestration refuses partial day sets before downstream publication.

use crate::{
    cli_days::compute_ok_paths, cli_validate::*, from_stage_name, ClassFilterArg, Feed, FromStage,
};
use aircraft_extract::{
    airport_index::AerodromeIndex,
    airport_io::{read_global_airport_lines, read_global_airports, AirportLineRow},
    progress::ts,
    stage_2a::run_stage_2a,
    stage_2b::{run_stage_2b_phase, CruisePhase},
    stage_2c::run_stage_2c,
    stage_airport_discover_runner::run_stage_airport_discover,
};
use anyhow::{Context, Result};
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
    primary_segments_dirs: Vec<PathBuf>,
    cruise_phase: CruisePhase,
    cruise_spill_disk_budget_bytes: Option<u64>,
) -> Result<()> {
    anyhow::ensure!(
        primary_segments_dirs.is_empty() || from_stage >= FromStage::Shuffle,
        "--segments-dir reuses completed inputs; choose --from-stage shuffle or later"
    );
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
    anyhow::ensure!(
        cruise_phase == CruisePhase::All
            || (from_stage == FromStage::Stage2b && until_stage == FromStage::Stage2b),
        "--cruise-phase requires a stage2b-only window"
    );
    anyhow::ensure!(
        cruise_spill_disk_budget_bytes.is_none() || cruise_phase == CruisePhase::Spill,
        "--cruise-spill-disk-budget-bytes requires --cruise-phase spill"
    );
    let needs_shuffled =
        runs(FromStage::Stage1_5) || runs(FromStage::Stage2a) || runs(FromStage::Stage2c);
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
    let external_segments = !primary_segments_dirs.is_empty();
    let primary_segments_dirs = if external_segments {
        primary_segments_dirs
    } else {
        vec![segments_dir.clone()]
    };
    let by_square_dir = work_dir.join("segments_by_square");
    if runs(FromStage::Shuffle) {
        crate::source_cache::validate_ga_merge(
            &ga_segments_dir.iter().cloned().collect::<Vec<_>>(),
            ga_adsb_cache.as_deref(),
        )?;
    } else if needs_shuffled {
        anyhow::ensure!(ga_segments_dir.is_none(), "--ga-segments-dir is consumed only by shuffle; downstream uses its saved source receipts");
        aircraft_extract::shuffle::completion::validate(&by_square_dir, scope.as_ref())?;
        crate::source_cache::validate_shuffled_sources(
            &by_square_dir,
            ga_adsb_cache.as_deref(),
            &adsb_cache,
            feed,
            class_filter,
        )?;
    } else {
        anyhow::ensure!(
            ga_segments_dir.is_none() && ga_adsb_cache.is_none(),
            "GA merge inputs apply only to shuffle or its downstream stages"
        );
    }

    let ga_day_paths: Vec<PathBuf> = match &ga_segments_dir {
        None => Vec::new(),
        Some(dir) => {
            require_input_dir_exists("--ga-segments-dir", dir)?;
            if dir.canonicalize().ok().is_some_and(|ga| {
                primary_segments_dirs
                    .iter()
                    .any(|dir| dir.canonicalize().ok().as_ref() == Some(&ga))
            }) {
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

    let reads_day_segments = runs(FromStage::Shuffle) || runs(FromStage::Stage2b);
    let ok_paths = if from_stage > FromStage::Stage1 && !reads_day_segments {
        Vec::new()
    } else if external_segments {
        reuse_segments_from_directories(
            &primary_segments_dirs,
            &days,
            class_filter,
            feed,
            &adsb_cache,
        )?
    } else {
        compute_ok_paths(
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
        )?
    };

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
    } else if needs_shuffled {
        require_input_dir_exists(
            "--work-dir/segments_by_square (required by Stage 1.5 / 2A / 2C)",
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

    // Cruise reads validated day inputs directly and may precede shuffle.
    // Reusing an existing shuffled tree must still retain its exact day window.
    if by_square_dir.try_exists()? {
        require_matching_window_days(&by_square_dir, &days)?;
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

    let window_n_days = u16::try_from(days.len())?;
    let ga_n_days = if needs_shuffled {
        read_ga_n_days(&by_square_dir)?
    } else {
        0
    };
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
        let _disk_reservation = cruise_spill_disk_budget_bytes
            .map(|bytes| -> Result<_> {
                Ok(aircraft_extract::arrow_io::SpillDiskReservation::new(
                    prepared_year_dir
                        .parent()
                        .context("missing prepared parent")?,
                    bytes,
                    ok_paths.len(),
                )?)
            })
            .transpose()?;
        run_stage_2b_phase(
            &ok_paths,
            &prepared_year_dir,
            window_n_days,
            scope.as_ref(),
            fail_on_ga_cruise,
            cruise_phase,
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

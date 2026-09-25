//! Ordered aircraft orchestration: provider days, their admission, then shuffle and Stage 2 under one sampling window.

use crate::{
    cli_days::{day_segment_paths, extract_days, Providers},
    cli_validate::*,
    from_stage_name,
    source_cache::SourceCache,
    FromStage,
};
use aircraft_extract::{
    airport_index::AerodromeIndex,
    airport_io::{read_global_airport_lines, read_global_airports, AirportLineRow},
    progress::ts,
    provider_receipt::{admit, read_receipts, write_admission, AdmittedDay, Admission},
    shuffle::completion,
    stage_2a::run_stage_2a,
    stage_2b::{run_stage_2b_phase, CruisePhase},
    stage_2c::run_stage_2c,
    stage_airport_discover_runner::run_stage_airport_discover,
};
use anyhow::{Context, Result};
use noise_compute::types::AirportArea;
use raster_reader::RealRasters;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub struct RunAllRequest {
    pub primary_cache: PathBuf,
    pub secondary_cache: Option<PathBuf>,
    pub prepared_year_dir: PathBuf,
    pub prepared_dir: PathBuf,
    pub work_dir: PathBuf,
    /// Completed day shards reused in place (shuffle onward).
    pub reused_segments_dirs: Vec<PathBuf>,
    pub days: Vec<String>,
    pub increment_days: Vec<String>,
    pub scope_bbox: Option<String>,
    pub from_stage: FromStage,
    pub until_stage: FromStage,
    pub cruise_phase: CruisePhase,
    pub cruise_spill_disk_budget_bytes: Option<u64>,
}

pub fn run_all(request: RunAllRequest) -> Result<()> {
    let RunAllRequest {
        primary_cache,
        secondary_cache,
        prepared_year_dir,
        prepared_dir,
        work_dir,
        reused_segments_dirs,
        days,
        increment_days,
        scope_bbox,
        from_stage,
        until_stage,
        cruise_phase,
        cruise_spill_disk_budget_bytes,
    } = request;
    anyhow::ensure!(
        reused_segments_dirs.is_empty() || from_stage >= FromStage::Shuffle,
        "--segments-dir reuses completed inputs; choose --from-stage shuffle or later"
    );
    let scope = parse_scope(scope_bbox.as_deref())?;
    require_scope_for_subset_cache(&primary_cache, scope.as_ref())?;
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
    anyhow::ensure!(
        !days.is_empty(),
        "--days is empty — refusing to start. Pass at least one day, \
         e.g. `--days 2025-01-01` or comma-separated list"
    );
    let requested = validated_days(days, false)?;
    let increment_candidates = validated_days(increment_days, true)?;
    anyhow::ensure!(
        increment_candidates.is_subset(&requested),
        "--increment-days must be requested days"
    );
    anyhow::ensure!(
        increment_candidates.is_empty() || secondary_cache.is_some(),
        "--increment-days needs --secondary-adsb-cache"
    );
    let primary_receipts = SourceCache::open(&primary_cache, &work_dir)?;
    let providers = Providers {
        scope: scope.as_ref(),
        primary_root: &primary_cache,
        primary_receipts: primary_receipts.as_ref(),
        secondary_root: secondary_cache.as_deref(),
        increment_candidates: &increment_candidates,
    };
    if let Some(s) = scope.as_ref() {
        eprintln!("{} [run-all] scope boxes: {}", ts(), s.key());
    }
    let rasters = RealRasters::new(&prepared_dir);
    let external_segments = !reused_segments_dirs.is_empty();
    let segments_dirs = if external_segments {
        reused_segments_dirs
    } else {
        vec![work_dir.join("segments")]
    };

    if from_stage <= FromStage::Stage1 {
        if from_stage == FromStage::Stage0 {
            let list: Vec<String> = requested.iter().cloned().collect();
            validate_fresh_stage0_work(&work_dir, &list, until_stage, primary_receipts.as_ref())?;
        }
        extract_days(
            &requested,
            &providers,
            &work_dir,
            &rasters,
            from_stage,
            until_stage,
        )?;
    }
    if until_stage <= FromStage::Stage1 {
        eprintln!(
            "{} [run-all] stopped after {} (--until-stage) under {}",
            ts(),
            from_stage_name(until_stage),
            work_dir.display()
        );
        return Ok(());
    }

    let by_square_dir = work_dir.join("segments_by_square");
    let needs_shuffled =
        runs(FromStage::Stage1_5) || runs(FromStage::Stage2a) || runs(FromStage::Stage2c);
    let reads_day_segments = runs(FromStage::Shuffle) || runs(FromStage::Stage2b);
    let admitted = if reads_day_segments {
        let admission =
            admit_from_receipts(&segments_dirs, &requested, &increment_candidates)?;
        report_admission(&admission);
        let paths = day_segment_paths(&segments_dirs, &admission.baseline_days)?;
        if external_segments {
            reuse_segments_from_directories(
                &segments_dirs,
                &admission.baseline_days,
                primary_receipts.as_ref(),
                providers.source_ids(),
            )?;
        } else if from_stage > FromStage::Stage1 {
            let dir = &segments_dirs[0];
            require_input_dir_exists("--work-dir/segments (--from-stage)", dir)?;
            let list: Vec<String> = admission.baseline_days.iter().cloned().collect();
            validate_segments(dir, &list, primary_receipts.as_ref(), providers.source_ids())?;
        }
        let days: Vec<AdmittedDay> = paths
            .into_iter()
            .zip(admission.baseline_days.iter())
            .map(|(segments, day)| AdmittedDay {
                segments,
                increment: admission.increment_days.contains(day),
            })
            .collect();
        Some((admission, days))
    } else {
        None
    };

    let (areas, global_lines) = load_global_airports(&prepared_year_dir, &runs)?;
    if runs(FromStage::Shuffle) {
        let (admission, days) = admitted.as_ref().context("shuffle needs admitted days")?;
        aircraft_extract::shuffle::shuffle_per_square(days, &by_square_dir, scope.as_ref())?;
        // Every provider-day status beside the window it produced.
        write_admission(&by_square_dir, admission)?;
    } else if needs_shuffled {
        require_input_dir_exists(
            "--work-dir/segments_by_square (required by Stage 1.5 / 2A / 2C)",
            &by_square_dir,
        )?;
    }
    if runs(FromStage::Shuffle) || needs_shuffled {
        completion::validate(&by_square_dir, scope.as_ref())?;
        crate::source_cache::validate_shuffled_sources(&by_square_dir, &primary_cache)?;
    }
    let window = if by_square_dir.try_exists()? {
        let window = completion::sampling_window(&by_square_dir)?;
        match &admitted {
            Some((admission, _)) => anyhow::ensure!(
                admission.sampling_window()? == window,
                "admitted days differ from the shuffled sampling window; rerun shuffle"
            ),
            None => {
                // Admission only drops requested days, never adds one.
                let (baseline, increment) = completion::sampling_days(&by_square_dir)?;
                anyhow::ensure!(
                    baseline.is_subset(&requested) && increment.is_subset(&increment_candidates),
                    "shuffled sampling days lie outside the requested days; rerun shuffle"
                );
            }
        }
        window
    } else {
        admitted
            .as_ref()
            .context("no shuffle and no admitted days: nothing defines the sampling window")?
            .0
            .sampling_window()?
    };
    eprintln!(
        "{} [run-all] sampling window: {} baseline day(s), {} increment day(s)",
        ts(),
        window.baseline_days,
        window.increment_days
    );
    if until_stage <= FromStage::Shuffle {
        eprintln!(
            "{} [run-all] stopped after shuffle (--until-stage): per-z9 shards in {}",
            ts(),
            by_square_dir.display()
        );
        return Ok(());
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
    if runs(FromStage::Stage2a) {
        run_stage_2a(&by_square_dir, &prepared_year_dir, &window, scope.as_ref())?;
    }
    if until_stage <= FromStage::Stage2a {
        eprintln!("{} [run-all] stopped after stage2a (--until-stage)", ts());
        return Ok(());
    }
    if runs(FromStage::Stage2b) {
        let (_, days) = admitted.as_ref().context("cruise needs admitted days")?;
        let spill_dir = work_dir.join("spill_cruise");
        let _disk_reservation = cruise_spill_disk_budget_bytes
            .map(|bytes| -> Result<_> {
                Ok(aircraft_extract::arrow_io::SpillDiskReservation::new(
                    &work_dir,
                    bytes,
                    days.len(),
                )?)
            })
            .transpose()?;
        run_stage_2b_phase(
            days,
            &prepared_year_dir,
            &spill_dir,
            &window,
            scope.as_ref(),
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
        &window,
        scope.as_ref(),
    )?;
    Ok(())
}

/// Day receipts live beside each segments directory, in its work dir.
fn admit_from_receipts(
    segments_dirs: &[PathBuf],
    requested: &BTreeSet<String>,
    increment_candidates: &BTreeSet<String>,
) -> Result<Admission> {
    let mut receipts = std::collections::BTreeMap::new();
    let mut works: Vec<&Path> = segments_dirs
        .iter()
        .map(|dir| dir.parent().context("segments directory has no work parent"))
        .collect::<Result<_>>()?;
    works.sort_unstable();
    works.dedup();
    for work in works {
        for (day, receipt) in read_receipts(work, requested)? {
            anyhow::ensure!(
                receipts.insert(day.clone(), receipt).is_none(),
                "{day}: provider receipts in more than one work directory"
            );
        }
    }
    admit(requested, increment_candidates, &receipts)
}

fn report_admission(admission: &Admission) {
    use aircraft_extract::provider_receipt::ProviderDayStatus;
    for (provider, statuses) in [("primary", &admission.primary), ("secondary", &admission.secondary)] {
        for (day, status) in statuses {
            if *status != ProviderDayStatus::Complete {
                eprintln!("{} [admission] {provider} {day}: {status:?}", ts());
            }
        }
    }
    eprintln!(
        "{} [admission] {} baseline day(s), {} increment day(s)",
        ts(),
        admission.baseline_days.len(),
        admission.increment_days.len()
    );
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

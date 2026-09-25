//! RAM-bounded day extraction of both providers preserves successful work but fails if any available day fails.

use crate::{source_cache::SourceCache, FromStage, STAGE01_PEAK_PER_DAY_GB};
use aircraft_extract::flight::source_id;
use aircraft_extract::memory::max_concurrent_days;
use aircraft_extract::{
    progress::ts, source::FlightSource, source_adsb_tar::AdsbTarSource, stage_0::run_stage_0,
    stage_1::run_stage_1,
};
use anyhow::{Context, Result};
use raster_reader::RealRasters;
use rayon::{iter::Either, prelude::*};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The providers of one run: the primary archive (publisher receipts when it
/// has a catalog) and the optional secondary archive for increment days.
pub struct Providers<'a> {
    /// Stage 0 keeps only traces that can reach a scope square.
    pub scope: Option<&'a aircraft_extract::scope::ScopeBbox>,
    pub primary_root: &'a Path,
    pub primary_receipts: Option<&'a SourceCache>,
    pub secondary_root: Option<&'a Path>,
    pub increment_candidates: &'a BTreeSet<String>,
}

impl Providers<'_> {
    /// Provider ids a completed day shard may carry.
    pub fn source_ids(&self) -> [u8; 2] {
        [source_id::ADSB_LOL_TAR, source_id::ADSB_EXCHANGE]
    }

    fn secondary_for(&self, day: &str) -> Option<AdsbTarSource> {
        let root = self.secondary_root?;
        let source = AdsbTarSource::new(root).with_source_id(source_id::ADSB_EXCHANGE);
        (self.increment_candidates.contains(day) && source.has_day(day)).then_some(source)
    }
}

/// Run Stage 0/1 for every requested day the primary provider holds; a day
/// it lacks gets a missing receipt for the admission. Returns the days run.
pub fn extract_days(
    days: &BTreeSet<String>,
    providers: &Providers<'_>,
    work_dir: &Path,
    rasters: &RealRasters,
    from_stage: FromStage,
    until_stage: FromStage,
) -> Result<Vec<String>> {
    let flights_dir = work_dir.join("flights");
    let segments_dir = work_dir.join("segments");
    std::fs::create_dir_all(&flights_dir)?;
    std::fs::create_dir_all(&segments_dir)?;
    let requested: Vec<String> = days.iter().cloned().collect();
    let available: Vec<String> = match providers.primary_receipts {
        Some(cache) => cache.validate(Some(&requested), None)?.into_keys().collect(),
        None => {
            let source = AdsbTarSource::new(providers.primary_root);
            requested
                .iter()
                .filter(|day| source.has_day(day))
                .cloned()
                .collect()
        }
    };
    let missing: Vec<&String> = requested
        .iter()
        .filter(|day| !available.contains(day))
        .collect();
    for day in &missing {
        aircraft_extract::provider_receipt::write_day_receipt(
            work_dir,
            &aircraft_extract::provider_receipt::DayReceipt::primary_missing(day),
        )?;
    }
    if !missing.is_empty() {
        eprintln!(
            "{} [run-all] {} requested day(s) without a primary archive stay missing: {}",
            ts(),
            missing.len(),
            missing
                .iter()
                .map(|day| day.as_str())
                .collect::<Vec<_>>()
                .join(",")
        );
    }
    let max_concurrent = max_concurrent_days(available.len(), STAGE01_PEAK_PER_DAY_GB);
    eprintln!(
        "{} [run-all] Stage 0/1: {} day(s), <={} concurrent (RAM-bounded; within-day fills every core)",
        ts(),
        available.len(),
        max_concurrent
    );
    let done_dir = if until_stage == FromStage::Stage0 {
        &flights_dir
    } else {
        &segments_dir
    };
    let mut ok_days: Vec<String> = Vec::new();
    let mut failed_days: Vec<String> = Vec::new();
    for chunk in available.chunks(max_concurrent.max(1)) {
        let (mut ok, mut fail): (Vec<String>, Vec<String>) =
            chunk.par_iter().partition_map(|day| {
                let done_path = done_dir.join(format!("{day}.arrow"));
                match run_day(
                    day,
                    providers,
                    work_dir,
                    &flights_dir,
                    &segments_dir,
                    rasters,
                    from_stage,
                    until_stage,
                ) {
                    Ok(()) if done_path.exists() => Either::Left(day.clone()),
                    Ok(()) => {
                        eprintln!("{} [run-all] {day}: FAILED — no output file produced", ts());
                        Either::Right(day.clone())
                    }
                    Err(e) => {
                        eprintln!("{} [run-all] {day}: FAILED stage0/1 — {e:#}, skipping", ts());
                        Either::Right(day.clone())
                    }
                }
            });
        ok_days.append(&mut ok);
        failed_days.append(&mut fail);
    }
    anyhow::ensure!(
        failed_days.is_empty(),
        "incomplete extraction: failed days {}; successful artifacts preserved in {}",
        failed_days.join(","),
        work_dir.display()
    );
    anyhow::ensure!(
        !ok_days.is_empty(),
        "no requested day has a primary archive under {}",
        providers.primary_root.display()
    );
    ok_days.sort();
    Ok(ok_days)
}

#[allow(clippy::too_many_arguments)]
fn run_day(
    day: &str,
    providers: &Providers<'_>,
    work_dir: &Path,
    flights_dir: &Path,
    segments_dir: &Path,
    rasters: &RealRasters,
    from_stage: FromStage,
    until_stage: FromStage,
) -> Result<()> {
    let t0 = Instant::now();
    let receipts = providers.primary_receipts;
    let stage0_log = if from_stage <= FromStage::Stage0 {
        let primary = match receipts {
            Some(cache) => {
                AdsbTarSource::new("").with_selected_archives(cache.begin(day, "flights")?)
            }
            None => AdsbTarSource::new(providers.primary_root),
        };
        let secondary = providers.secondary_for(day);
        let n0 = run_stage_0(
            &primary,
            secondary.as_ref().map(|s| s as &dyn FlightSource),
            day,
            flights_dir,
            work_dir,
            providers.scope,
        )?;
        if let Some(cache) = receipts {
            cache.complete(day, "flights")?;
        }
        format!("stage0={n0} ({:?})", t0.elapsed())
    } else {
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
    if let Some(cache) = receipts {
        cache.begin(day, "segments")?;
    }
    let n1 = run_stage_1(flights_dir, segments_dir, day, rasters)?;
    if let Some(cache) = receipts {
        cache.complete(day, "segments")?;
    }
    eprintln!(
        "{} [run-all] {day}: {stage0_log} stage1={n1} ({:?})",
        ts(),
        t_stage1.elapsed()
    );
    Ok(())
}

/// Day shard paths of `days` across the segment directories; each day lives in exactly one.
pub fn day_segment_paths(dirs: &[PathBuf], days: &BTreeSet<String>) -> Result<Vec<PathBuf>> {
    let by_day: std::collections::BTreeMap<String, PathBuf> =
        crate::cli_validate::list_segments_day_paths_multi(dirs)?
            .into_iter()
            .map(|path| (path.file_stem().unwrap().to_string_lossy().into_owned(), path))
            .collect();
    days.iter()
        .map(|day| {
            by_day
                .get(day)
                .cloned()
                .with_context(|| format!("admitted day {day} has no completed day shard"))
        })
        .collect()
}

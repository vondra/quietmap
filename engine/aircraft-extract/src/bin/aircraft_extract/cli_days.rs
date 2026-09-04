//! RAM-bounded day extraction preserves successful work but fails if any requested day is missing.

use crate::{cli_validate::*, ClassFilterArg, Feed, FromStage};
use aircraft_extract::memory::max_concurrent_days;
use aircraft_extract::{
    progress::ts, source::FlightSource, source_adsb_tar::AdsbTarSource, stage_0::run_stage_0,
    stage_1::run_stage_1,
};
use anyhow::Result;
use raster_reader::RealRasters;
use rayon::{iter::Either, prelude::*};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[allow(clippy::too_many_arguments)]
pub fn compute_ok_paths(
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
        let max_concurrent =
            max_concurrent_days(days.len(), class_filter.stage01_peak_per_day_gb());
        eprintln!(
            "{} [run-all] Stage 0/1: {} day(s), <={} concurrent (RAM-bounded; within-day fills every core)",
            ts(),
            days.len(),
            max_concurrent
        );
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

        anyhow::ensure!(
            failed_days.is_empty(),
            "incomplete extraction: failed days {}; successful artifacts preserved in {}",
            failed_days.join(","),
            work_dir.display()
        );
        if ok_paths.is_empty() {
            return Err(anyhow::anyhow!(
                "every requested day failed Stage 0/1 — nothing produced under \
                 {}. Check upstream errors and rerun with --days <surviving-list>",
                work_dir.display(),
            ));
        }
        ok_paths
    } else if needs_ok_paths {
        require_input_dir_exists("--work-dir/segments (--from-stage)", segments_dir)?;
        validate_segments(segments_dir, days, class_filter, feed)?;
        let paths = list_segments_day_paths(segments_dir)?;
        eprintln!(
            "{} [run-all] reusing {} segment shard(s) from {}",
            ts(),
            paths.len(),
            segments_dir.display()
        );
        paths
    } else {
        Vec::new()
    };
    Ok(ok_paths)
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resumed_day_shards_must_match_requested_feed_and_class() {
        use aircraft_extract::flight::{FlightSegment, Phase};
        let temp = tempfile::tempdir().unwrap();
        let segments = temp.path().join("segments");
        let day = "2025-01-01";
        let segment = FlightSegment {
            flight_id: 1,
            callsign: "TEST".into(),
            aircraft_type: *b"B738",
            profile_idx: aircraft_extract::profile::profile_idx("B738"),
            source_id: Feed::Adsblol.source_id(),
            origin: 0,
            veh_kind: 0,
            gse_class: 0,
            period: 0,
            date_id: aircraft_extract::period::parse_date_id(day).unwrap(),
            phase: Phase::Airborne,
            flags: 0,
            start_lat: 50.0,
            start_lon: 14.0,
            start_alt_m: 1000.0,
            end_lat: 50.001,
            end_lon: 14.001,
            end_alt_m: 1000.0,
            speed_kt: 250.0,
            length_m: 100.0,
            agl_avg_m: 1000.0,
            start_elev_m: 0.0,
            end_elev_m: 0.0,
        };
        aircraft_extract::arrow_io::write_segments(
            &segments.join(format!("{day}.arrow")),
            &[segment],
        )
        .unwrap();
        for (feed, class, accepted) in [
            (Feed::Adsblol, ClassFilterArg::NonGa, true),
            (Feed::Adsbexchange, ClassFilterArg::NonGa, false),
            (Feed::Adsblol, ClassFilterArg::Ga, false),
        ] {
            let result = compute_ok_paths(
                &[day.into()],
                temp.path(),
                temp.path(),
                &temp.path().join("flights"),
                &segments,
                &RealRasters::new(temp.path()),
                FromStage::Shuffle,
                FromStage::Shuffle,
                feed,
                class,
                &|stage| stage == FromStage::Shuffle,
            );
            assert_eq!(result.is_ok(), accepted, "feed={feed:?} class={class:?}");
        }
    }
}

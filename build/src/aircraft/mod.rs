//! Aircraft Stage 0/1 (ported from dev4 `aircraft-extract`): ADS-B traces of the primary provider,
//! plus the secondary provider on increment days, to per-day flight segments and flight tables with
//! altitudes above EGM2008, then the admission of the sampling days. One day at a time; a day with
//! a receipt is complete and is not redone.

mod altitude;
mod archive;
mod catalog;
mod day;
mod dem;
mod echoes;
mod filters;
pub mod flat;
mod flight_table;
mod flights;
pub mod geoid;
mod ground;
mod merge;
mod output;
mod phases;
mod readsb;
mod receipt;
mod scope;
mod segments;
mod trace;
pub mod tracks;

/// The flags of a written segment, for the readers of the day files (the aircraft boxes).
pub use filters::MIN_SEGMENT_LENGTH_M;
pub use segments::{HELICOPTER_DESCENT, IS_DEPARTURE, ON_GROUND, SECONDARY_ONLY};

use crate::dev4::Dev4;
use day::{Inputs, extract_day, read_receipt, write_receipt};
use receipt::{DayReceipt, admit};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;

/// The run's days.
pub struct Days {
    pub requested: BTreeSet<String>,
    /// Days whose secondary provider is read (they must also be requested).
    pub increment_candidates: BTreeSet<String>,
}

impl Days {
    pub fn listed(days: &[String], increments: &[String]) -> Result<Self, String> {
        let parse = |list: &[String]| -> Result<BTreeSet<String>, String> {
            list.iter()
                .map(|day| crate::period::date_id(day.trim()).map(|_| day.trim().to_string()))
                .collect()
        };
        let days = Days {
            requested: parse(days)?,
            increment_candidates: parse(increments)?,
        };
        if !days.increment_candidates.is_subset(&days.requested) {
            return Err("increment days must be requested days".into());
        }
        Ok(days)
    }
}

/// A field of `/proc/self/status` in GB: `VmHWM` (peak resident) or `RssAnon` (heap).
fn resident_gb(field: &str) -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            let line = status.lines().find(|line| line.starts_with(field))?;
            line.split_whitespace().nth(1)?.parse::<f64>().ok()
        })
        .map_or(0.0, |kb| kb / 1024.0 / 1024.0)
}

/// The largest heap (`RssAnon`) seen every 100 ms while a day runs.
struct HeapWatch {
    peak_kb: std::sync::Arc<std::sync::atomic::AtomicU64>,
    running: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl HeapWatch {
    fn start() -> Self {
        use std::sync::atomic::Ordering::Relaxed;
        let watch = HeapWatch {
            peak_kb: Default::default(),
            running: std::sync::Arc::new(true.into()),
        };
        let (peak, running) = (watch.peak_kb.clone(), watch.running.clone());
        std::thread::spawn(move || {
            while running.load(Relaxed) {
                peak.fetch_max((resident_gb("RssAnon:") * 1024.0 * 1024.0) as u64, Relaxed);
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        });
        watch
    }

    fn peak_gb(&self) -> f64 {
        self.peak_kb.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1024.0 / 1024.0
    }
}

impl Drop for HeapWatch {
    fn drop(&mut self) {
        self.running
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

pub struct Run<'a> {
    pub days: Days,
    pub primary: &'a Path,
    pub secondary: Option<&'a Path>,
    pub rasters: &'a Path,
    pub geoid: &'a Path,
    pub boxes: Option<&'a str>,
    pub out: &'a Path,
    /// National terrain models laid over the dev4 heights.
    pub national: Vec<crate::terrain::national::NationalHeights>,
}

/// `qm-build aircraft-segments`: extract every requested day not yet complete, then admit.
pub fn run(run: Run) -> Result<(), String> {
    let scope = run.boxes.map(scope::Scope::parse).transpose()?;
    let inputs = Inputs {
        primary_root: run.primary.to_path_buf(),
        secondary_root: run.secondary.map(Path::to_path_buf),
        terrain: dem::TerrainHeights::new(Dev4 {
            prepared: Default::default(),
            rasters: run.rasters.to_path_buf(),
        })
        .with_national(run.national),
        geoid: geoid::Geoid::read(run.geoid)?,
        scope,
    };
    let scope_key = inputs.scope.as_ref().map(scope::Scope::key);
    let mut failed = Vec::new();
    for day in &run.days.requested {
        if let Some(receipt) = read_receipt(run.out, day)? {
            if receipt.scope != scope_key {
                return Err(format!(
                    "{day} was extracted under scope {:?}; rerun under one scope",
                    receipt.scope
                ));
            }
            continue;
        }
        let started = Instant::now();
        let heap = HeapWatch::start();
        match extract_day(
            &inputs,
            day,
            run.days.increment_candidates.contains(day),
            run.out,
        ) {
            Ok(summary) => eprintln!(
                "aircraft {day}: {} traces, {} flights, {} segments, {} regional offset cells in {:.0} s \
                 ({:.0} s reading the archives); peak heap {:.1} GB, peak resident {:.1} GB",
                summary.traces,
                summary.flights,
                summary.segments,
                summary.regional_cells,
                started.elapsed().as_secs_f64(),
                summary.read_s,
                heap.peak_gb(),
                resident_gb("VmHWM:")
            ),
            Err(error) => {
                eprintln!("aircraft {day}: FAILED {error}");
                failed.push(day.clone());
            }
        }
    }
    if !failed.is_empty() {
        return Err(format!(
            "failed days {}; the others are complete",
            failed.join(",")
        ));
    }
    let mut receipts = BTreeMap::new();
    for day in &run.days.requested {
        let receipt = read_receipt(run.out, day)?.ok_or_else(|| format!("{day}: no receipt"))?;
        receipts.insert(day.clone(), receipt);
    }
    let admission = admit(
        &run.days.requested,
        &run.days.increment_candidates,
        &receipts,
    )?;
    for (day, receipt) in &receipts {
        if receipt.needs_primary_only_rewrite(&admission.increment_days) {
            eprintln!("aircraft {day}: secondary provider not admitted, rewriting primary-only");
            extract_day(&inputs, day, false, run.out)?;
            let rewritten =
                read_receipt(run.out, day)?.ok_or_else(|| format!("{day}: no receipt"))?;
            let kept = DayReceipt {
                secondary: receipt.secondary.clone(),
                ..rewritten
            };
            write_receipt(run.out, &kept)?;
        }
    }
    let path = run.out.join("admission.json");
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&admission).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("{}: {e}", path.display()))?;
    eprintln!(
        "aircraft: {} baseline and {} increment days admitted of {} requested",
        admission.baseline_days.len(),
        admission.increment_days.len(),
        run.days.requested.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listed_days_are_checked_and_increments_must_be_requested() {
        let days = Days::listed(
            &["2025-09-02".into(), " 2025-10-01".into()],
            &["2025-10-01".into()],
        )
        .unwrap();
        assert_eq!(days.requested.len(), 2);
        assert!(Days::listed(&["2025-09-02".into()], &["2025-10-01".into()]).is_err());
        assert!(Days::listed(&["2025-02-30".into()], &[]).is_err());
    }
}

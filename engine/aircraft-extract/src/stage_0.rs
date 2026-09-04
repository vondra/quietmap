//! Stage 0 — orchestrate every [`crate::source::FlightSource`] for one
//! day, dedupe, and write `flights/<day>.arrow`.
//!
//! Inputs: list of sources + a YYYY-MM-DD day string.
//! Output: per-day Arrow file with one row per surviving flight.

use std::path::Path;

use anyhow::{Context, Result};

use crate::arrow_io::{write_flights, FlightRow};
use crate::dedup;
use crate::flight::Flight;
use crate::progress::{finished, started};
use crate::source::FlightSource;

/// Read every source for `day_str`, dedupe, and write the per-day
/// Stage 0 Arrow file under `output_root/<day>.arrow`.
///
/// Returns the number of surviving flights so the caller can log it.
pub fn run_stage_0(
    sources: &[Box<dyn FlightSource>],
    day_str: &str,
    output_root: &Path,
) -> Result<usize> {
    started(
        "stage0",
        &format!("day={day_str}, sources={}", sources.len()),
    );
    let mut all = Vec::new();
    for s in sources {
        let mut flights = s
            .read_day(day_str)
            .with_context(|| format!("source {} day {day_str}", s.source_id()))?;
        all.append(&mut flights);
    }
    let n_raw = all.len();
    let unique = dedup::dedup(all);
    let path = output_root.join(format!("{day_str}.arrow"));
    write_flights_at(&path, &unique)?;
    finished(
        "stage0",
        &format!(
            "day={day_str}, {n_raw} raw → {} unique flights",
            unique.len()
        ),
    );
    Ok(unique.len())
}

/// Lower-level: write a `Vec<Flight>` to `path`. Exposed for callers
/// that already have an in-memory slice (tests, CLI subcommands).
pub fn write_flights_at(path: &Path, flights: &[Flight]) -> Result<()> {
    // FlightRow borrows the typecode as `&[u8; 4]`, so we collect the
    // 4-byte fixed-size codes into a parallel Vec and lend each row
    // a slot — keeps the row construction allocation-free per flight.
    let typecodes: Vec<[u8; 4]> = flights
        .iter()
        .map(|f| {
            let mut atype = [0u8; 4];
            let bytes = f.aircraft_type.as_bytes();
            let n = bytes.len().min(4);
            atype[..n].copy_from_slice(&bytes[..n]);
            atype
        })
        .collect();
    let rows: Vec<FlightRow<'_>> = flights
        .iter()
        .zip(typecodes.iter())
        .map(|(f, atype)| FlightRow {
            flight_id: f.flight_id,
            callsign: &f.callsign,
            aircraft_type: atype,
            profile_idx: f.profile_idx,
            source_id: f.source_id,
            origin: f.origin,
            veh_kind: f.veh_kind,
            gse_class: f.gse_class,
            base_timestamp: f.points.first().map(|p| p.timestamp).unwrap_or(0.0),
            points: &f.points,
        })
        .collect();
    write_flights(path, &rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source_adsb_tar::AdsbTarSource;
    use tempfile::tempdir;

    /// Skips unless QM_FLIGHTS_CACHE points at a radius cache root containing
    /// 2025/2025-01-21 (the same cache as ADSB_CACHE in scripts/run-aircraft-extract.sh).
    #[test]
    fn run_stage_0_smoke_against_praha_cache() {
        let Ok(root) = std::env::var("QM_FLIGHTS_CACHE") else {
            return;
        };
        if !std::path::Path::new(&root).join("2025/2025-01-21").exists() {
            return;
        }
        let out = tempdir().unwrap();
        let sources: Vec<Box<dyn FlightSource>> = vec![Box::new(AdsbTarSource::new(root))];
        let n = run_stage_0(&sources, "2025-01-21", out.path()).unwrap();
        assert!(n > 100, "got {n}");
        let path = out.path().join("2025-01-21.arrow");
        assert!(path.exists());
        let meta = std::fs::metadata(&path).unwrap();
        assert!(meta.len() > 1024);
    }
}

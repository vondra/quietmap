//! Stage 0 — read the primary provider-day and, on increment days, the
//! secondary one; merge them into one trace per aircraft address; write
//! `flights/<day>.arrow` and the day's provider receipts.

use std::path::Path;

use anyhow::{Context, Result};
use rayon::prelude::*;

use crate::arrow_io::{write_flights, FlightRow};
use crate::filters::point_is_sane;
use crate::flight::Flight;
use crate::progress::{finished, started};
use crate::provider_merge::{merge_provider_traces, MergeCounts};
use crate::provider_receipt::{write_day_receipt, DayReceipt};
use crate::scope::ScopeBbox;
use crate::source::FlightSource;
use crate::source_adsb_tar::trace_to_flight;
use crate::trace::AircraftTrace;

/// Merge `primary` with `secondary` (when given) for `day_str`, write
/// `flights_dir/<day>.arrow` and the day receipt under `work_dir`. Receipts
/// always describe the whole provider-day; a `scope` then keeps only merged
/// traces whose extent can reach a written square. Returns the number of flights.
pub fn run_stage_0(
    primary: &dyn FlightSource,
    secondary: Option<&dyn FlightSource>,
    day_str: &str,
    flights_dir: &Path,
    work_dir: &Path,
    scope: Option<&ScopeBbox>,
) -> Result<usize> {
    started(
        "stage0",
        &format!("day={day_str}, providers={}", 1 + usize::from(secondary.is_some())),
    );
    let primary_day = primary
        .read_provider_day(day_str)
        .with_context(|| format!("primary source {} day {day_str}", primary.source_id()))?;
    let secondary_day = secondary
        .map(|source| {
            source
                .read_provider_day(day_str)
                .with_context(|| format!("secondary source {} day {day_str}", source.source_id()))
        })
        .transpose()?;
    let secondary_source = secondary.map_or(primary.source_id(), |s| s.source_id());
    let (traces, merge, secondary_receipt) = match secondary_day {
        Some(day) => {
            let (traces, counts) = merge_provider_traces(primary_day.traces, day.traces);
            (traces, counts, Some(day.receipt))
        }
        None => {
            let (traces, counts) = merge_provider_traces(primary_day.traces, Vec::new());
            (traces, counts, None)
        }
    };
    let mut traces = traces;
    if let Some(scope) = scope {
        traces.retain(|trace| trace_may_touch(trace, scope));
    }
    let flights: Vec<Flight> = traces
        .into_par_iter()
        .flat_map_iter(|trace| trace_to_flight(trace, primary.source_id(), secondary_source))
        .collect();
    let path = flights_dir.join(format!("{day_str}.arrow"));
    write_flights_at(&path, &flights)?;
    write_day_receipt(
        work_dir,
        &DayReceipt {
            day: day_str.to_owned(),
            primary: Some(primary_day.receipt),
            secondary: secondary_receipt,
            merge,
        },
    )?;
    let MergeCounts {
        secondary_points_kept,
        anonymous_points_suppressed,
        ..
    } = merge;
    finished(
        "stage0",
        &format!(
            "day={day_str}, {} flights; {secondary_points_kept} secondary samples kept, \
             {anonymous_points_suppressed} anonymous echo samples suppressed",
            flights.len()
        ),
    );
    Ok(flights.len())
}

fn trace_may_touch(trace: &AircraftTrace, scope: &ScopeBbox) -> bool {
    let mut extent: Option<[f64; 4]> = None;
    for point in trace.points.iter().filter(|p| point_is_sane(p)) {
        let (lat, lon) = (f64::from(point.lat), f64::from(point.lon));
        let e = extent.get_or_insert([lat, lon, lat, lon]);
        e[0] = e[0].min(lat);
        e[1] = e[1].min(lon);
        e[2] = e[2].max(lat);
        e[3] = e[3].max(lon);
    }
    extent.is_some_and(|[south, west, north, east]| scope.may_touch_extent(south, west, north, east))
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
    /// 2025/2025-01-21.
    #[test]
    fn run_stage_0_smoke_against_praha_cache() {
        let Ok(root) = std::env::var("QM_FLIGHTS_CACHE") else {
            return;
        };
        if !std::path::Path::new(&root).join("2025/2025-01-21").exists() {
            return;
        }
        let out = tempdir().unwrap();
        let n = run_stage_0(
            &AdsbTarSource::new(root),
            None,
            "2025-01-21",
            out.path(),
            out.path(),
            None,
        )
        .unwrap();
        assert!(n > 100, "got {n}");
        let path = out.path().join("2025-01-21.arrow");
        assert!(path.exists());
        let meta = std::fs::metadata(&path).unwrap();
        assert!(meta.len() > 1024);
    }

    fn day_dir(root: &Path, day: &str, traces: &[(&str, &str)]) {
        let dir = root.join(&day[..4]).join(day);
        std::fs::create_dir_all(&dir).unwrap();
        let mut builder = tar::Builder::new(std::fs::File::create(dir.join("subset.tar")).unwrap());
        for (address, json) in traces {
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            std::io::Write::write_all(&mut encoder, json.as_bytes()).unwrap();
            let body = encoder.finish().unwrap();
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, format!("traces/00/trace_full_{address}.json"), body.as_slice())
                .unwrap();
        }
        builder.finish().unwrap();
    }

    /// The invariant of the provider union: a second provider holding a copy
    /// of the primary archive leaves the flights file identical (every column
    /// byte and every stamp), so every downstream energy and movement is
    /// unchanged. Files are compared decoded: IPC orders schema metadata by
    /// hash iteration.
    #[test]
    fn a_duplicate_provider_leaves_the_flights_file_identical() {
        let temp = tempdir().unwrap();
        let day = "2026-05-01";
        let traces = [
            (
                "4ca001",
                r#"{"icao":"4ca001","t":"B738","timestamp":1777600000,"trace":[
                    [0,50.10,14.20,3000,180,90,0,1200,{"flight":"CSA1"}],
                    [10,50.10,14.21,3200,180,90,0,1200],[20,50.10,14.22,3400,180,90,1,1200],
                    [400,50.10,14.30,9000,300,90,0,0]]}"#,
            ),
            (
                "~aa0001",
                r#"{"icao":"~aa0001","t":"C172","timestamp":1777600000,"trace":[
                    [0,49.0,13.0,2000,100,0,0,0],[30,49.01,13.0,2000,100,0,0,0]]}"#,
            ),
        ];
        for provider in ["primary", "copy"] {
            day_dir(&temp.path().join(provider), day, &traces);
        }
        let primary = AdsbTarSource::new(temp.path().join("primary"));
        let copy = AdsbTarSource::new(temp.path().join("copy"))
            .with_source_id(crate::flight::source_id::ADSB_EXCHANGE);
        let alone = temp.path().join("alone");
        let merged = temp.path().join("merged");
        let n = run_stage_0(&primary, None, day, &alone, &alone, None).unwrap();
        assert_eq!(n, 2, "4ca001 (an airborne gap keeps its identity) and the anonymous C172");
        assert_eq!(
            run_stage_0(&primary, Some(&copy), day, &merged, &merged, None).unwrap(),
            n
        );
        let file = format!("{day}.arrow");
        assert_eq!(
            crate::arrow_io::read_record_batches(&merged.join(&file)).unwrap(),
            crate::arrow_io::read_record_batches(&alone.join(&file)).unwrap()
        );
        let receipt = crate::provider_receipt::read_day_receipt(&merged, day).unwrap().unwrap();
        assert_eq!(receipt.merge.secondary_points_kept, 0);
        assert_eq!(receipt.merge.secondary_points_covered, 6);
        // A scope far from both traces keeps the receipts whole but no flight.
        let scoped = temp.path().join("scoped");
        let scope = ScopeBbox::parse("27,-18.5,29.5,-13").unwrap();
        assert_eq!(run_stage_0(&primary, None, day, &scoped, &scoped, Some(&scope)).unwrap(), 0);
        let whole = crate::provider_receipt::read_day_receipt(&scoped, day).unwrap().unwrap();
        assert_eq!(whole.primary.unwrap().traces, 2);
    }
}

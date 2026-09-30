//! One UTC day, Stage 0 and 1 in memory: read the providers, merge, keep the scope, split into
//! flights, correct altitudes, classify and segment, then write the day's files and its receipt
//! (last, so a day with a receipt is complete).

use super::altitude::{GeometricDatum, RegionalOffsets, geometric_datum, samples, terrain_m};
use super::archive::read_archive;
use super::catalog::{primary_day_parts, secondary_day_parts};
use super::dem::TerrainHeights;
use super::filters::{low_level_speed_cap_kt, point_is_sane, validate_trajectory};
use super::flight_table::FlightRow;
use super::flights::{ADSB_EXCHANGE, ADSB_LOL, trace_to_flights};
use super::geoid::Geoid;
use super::ground::ground_flags;
use super::merge::{MergeCounts, merge_providers};
use super::output::DayWriter;
use super::period::{date_id, day_start};
use super::phases::classify;
use super::receipt::{DayReceipt, ProviderDayReceipt};
use super::scope::Scope;
use super::segments::{Segment, build_segments, departure_field_m, suppress_covered_secondary};
use super::trace::AircraftTrace;
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Traces processed together: bounds the flights and segments held beside the day's traces.
const TRACES_PER_CHUNK: usize = 2048;

/// Everything a day reads besides its archives.
pub struct Inputs {
    pub primary_root: PathBuf,
    pub secondary_root: Option<PathBuf>,
    pub terrain: TerrainHeights,
    pub geoid: Geoid,
    pub scope: Option<Scope>,
}

pub fn receipt_path(out: &Path, day: &str) -> PathBuf {
    out.join("receipts").join(format!("{day}.json"))
}

pub fn read_receipt(out: &Path, day: &str) -> Result<Option<DayReceipt>, String> {
    let path = receipt_path(out, day);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("{}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

pub fn write_receipt(out: &Path, receipt: &DayReceipt) -> Result<(), String> {
    let path = receipt_path(out, &receipt.day);
    let temporary = path.with_extension("json.partial");
    let json = serde_json::to_vec_pretty(receipt).map_err(|error| error.to_string())?;
    std::fs::write(&temporary, json)
        .map_err(|error| format!("{}: {error}", temporary.display()))?;
    std::fs::rename(&temporary, &path).map_err(|error| format!("{}: {error}", path.display()))
}

/// What one day extraction produced, for the log.
pub struct DaySummary {
    /// Seconds spent reading and parsing the archives.
    pub read_s: f64,
    pub traces: usize,
    pub flights: u64,
    pub segments: u64,
    pub regional_cells: usize,
}

/// Extract `day`, reading the secondary provider only when `with_secondary`.
pub fn extract_day(
    inputs: &Inputs,
    day: &str,
    with_secondary: bool,
    out: &Path,
) -> Result<DaySummary, String> {
    let (date, start) = (date_id(day)?, day_start(day)?);
    for directory in ["segments", "flights", "receipts"] {
        std::fs::create_dir_all(out.join(directory))
            .map_err(|error| format!("{}: {error}", out.display()))?;
    }
    let scope = inputs.scope.as_ref().map(Scope::key);
    let Some(primary_parts) = primary_day_parts(&inputs.primary_root, day)? else {
        write_receipt(out, &DayReceipt::primary_missing(day, scope))?;
        return Ok(DaySummary {
            read_s: 0.0,
            traces: 0,
            flights: 0,
            segments: 0,
            regional_cells: 0,
        });
    };
    let reading = std::time::Instant::now();
    let primary =
        read_archive(&primary_parts).map_err(|error| format!("{day} primary: {error}"))?;
    let primary_receipt =
        ProviderDayReceipt::of(ADSB_LOL, start, &primary.traces, primary.corrupt_members);
    let mut secondary_receipt = None;
    let mut secondary_traces = Vec::new();
    if with_secondary
        && let Some(root) = &inputs.secondary_root
        && let Some(parts) = secondary_day_parts(root, day)?
    {
        let secondary =
            read_archive(&parts).map_err(|error| format!("{day} secondary: {error}"))?;
        secondary_receipt = Some(ProviderDayReceipt::of(
            ADSB_EXCHANGE,
            start,
            &secondary.traces,
            secondary.corrupt_members,
        ));
        secondary_traces = secondary.traces;
    }
    let read_s = reading.elapsed().as_secs_f64();
    let secondary_source = if secondary_receipt.is_some() {
        ADSB_EXCHANGE
    } else {
        ADSB_LOL
    };
    let (mut traces, merge): (Vec<AircraftTrace>, MergeCounts) =
        merge_providers(primary.traces, secondary_traces);
    if let Some(scope) = &inputs.scope {
        traces.retain(|trace| scope.touches(trace.points.iter().filter(|p| point_is_sane(p))));
    }
    let datums: Vec<GeometricDatum> = traces
        .par_iter()
        .map(|trace| {
            let mut last = None;
            let sane: Vec<_> = trace
                .points
                .iter()
                .filter(|p| point_is_sane(p))
                .copied()
                .collect();
            geometric_datum(&sane, &inputs.geoid, |point| {
                terrain_m(&inputs.terrain, point, &mut last)
            })
        })
        .collect::<Result<_, String>>()?;
    let regional = RegionalOffsets::build(
        &traces
            .iter()
            .zip(&datums)
            .map(|(t, &d)| (t.points.as_slice(), d))
            .collect::<Vec<_>>(),
        &inputs.geoid,
    );
    let metadata = HashMap::from([
        ("day".to_string(), day.to_string()),
        (
            "scope".to_string(),
            scope.clone().unwrap_or_else(|| "world".into()),
        ),
    ]);
    let mut writer = DayWriter::create(
        &out.join("segments").join(format!("{day}.arrow")),
        &out.join("flights").join(format!("{day}.arrow")),
        metadata,
    )?;
    let trace_count = traces.len();
    let mut rest: Vec<(AircraftTrace, GeometricDatum)> = traces.into_iter().zip(datums).collect();
    while !rest.is_empty() {
        let chunk: Vec<_> = rest.drain(..TRACES_PER_CHUNK.min(rest.len())).collect();
        let rows: Vec<Vec<(FlightRow, Vec<Segment>)>> = chunk
            .into_par_iter()
            .map(|(trace, datum)| {
                trace_to_flights(trace, ADSB_LOL, secondary_source)
                    .into_iter()
                    .map(|flight| flight_rows(inputs, &regional, flight, datum))
                    .collect::<Result<Vec<_>, String>>()
            })
            .collect::<Result<_, String>>()?;
        for (row, segments) in rows.iter().flatten() {
            writer.push(row, segments, date)?;
        }
    }
    let summary = DaySummary {
        read_s,
        traces: trace_count,
        flights: writer.flight_rows,
        segments: writer.segment_rows,
        regional_cells: regional.cells(),
    };
    writer.finish()?;
    write_receipt(
        out,
        &DayReceipt {
            day: day.into(),
            primary: Some(primary_receipt),
            secondary: secondary_receipt,
            merge,
            scope,
        },
    )?;
    Ok(summary)
}

/// Stage 1 of one flight: terrain and altitude per sample, the bogus tail cut, ground, departure
/// field, phases, covered secondary samples dropped, segments.
fn flight_rows(
    inputs: &Inputs,
    regional: &RegionalOffsets,
    flight: super::flights::Flight,
    datum: GeometricDatum,
) -> Result<(FlightRow, Vec<Segment>), String> {
    let mut last = None;
    let terrain = flight
        .points
        .iter()
        .map(|point| terrain_m(&inputs.terrain, point, &mut last))
        .collect::<Result<Vec<f32>, String>>()?;
    let (mut samples, source) = samples(&flight.points, &terrain, datum, &inputs.geoid, regional);
    validate_trajectory(
        &mut samples,
        low_level_speed_cap_kt(flight.airframe, flight.profile),
    );
    if samples.len() < 2 {
        return Ok((
            FlightRow::of(&flight, source as u8, f32::NAN, 0),
            Vec::new(),
        ));
    }
    let on_ground = ground_flags(&samples);
    let field = departure_field_m(&samples, &on_ground);
    let mut phases = classify(&on_ground, |i| samples[i].height_m);
    suppress_covered_secondary(&mut samples, &mut phases, flight.airframe);
    let segments = build_segments(&samples, &phases, flight.airframe);
    Ok((
        FlightRow::of(&flight, source as u8, field, segments.len()),
        segments,
    ))
}

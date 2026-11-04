//! Test-only replay of the audited frozen busiest-service-day selections, never a production option.
use super::{calendar, gtfs};
use crate::railways::movements::{Record, write_record};
use serde::Deserialize;
use std::{
    fs::File,
    io::{BufWriter, Write},
    path::PathBuf,
};

#[derive(Deserialize)]
struct Selection {
    input: String,
    source_id: u16,
    country: String,
    rail_route_types: Vec<u16>,
    tram_route_types: Vec<u16>,
    excluded_route_names: Vec<String>,
    calendar_window: bool,
    directories: Vec<Directory>,
}
#[derive(Deserialize)]
struct Directory {
    directory: PathBuf,
    dates: std::collections::BTreeMap<String, String>,
}
#[derive(Deserialize)]
struct Plan {
    selections: PathBuf,
    output: PathBuf,
    start: String,
    end: String,
    manifest: PathBuf,
    sources: PathBuf,
    cz: Option<CzechFixture>,
}
#[derive(Deserialize)]
struct CzechFixture {
    sequences: PathBuf,
    stations: PathBuf,
    source: u16,
    country: String,
}

#[test]
#[ignore = "requires an explicitly supplied frozen service-day audit and retained publisher objects"]
fn replay_frozen_service_days() {
    let file = std::env::var("QM_RAIL_TIMETABLE_FIXTURE").unwrap();
    let plan: Plan = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    assert!(!plan.output.exists());
    let selections: Vec<Selection> =
        serde_json::from_slice(&std::fs::read(plan.selections).unwrap()).unwrap();
    let mut writer = BufWriter::new(File::create(plan.output).unwrap());
    write_record(
        &mut writer,
        &Record::Window {
            version: crate::railways::movements::STREAM_VERSION,
            start: plan.start,
            end: plan.end,
        },
    )
    .unwrap();
    for selection in selections {
        for directory in selection.directories {
            for (family, date) in directory.dates {
                if date.is_empty() {
                    continue;
                }
                let start = calendar::compact_date(&date).unwrap();
                let end = start.succ_opt().unwrap();
                let options = gtfs::Options {
                    identity: &selection.input,
                    source_id: selection.source_id,
                    country: &selection.country,
                    rail_types: if family == "rail" {
                        &selection.rail_route_types
                    } else {
                        &[]
                    },
                    tram_types: if family == "tram" {
                        &selection.tram_route_types
                    } else {
                        &[]
                    },
                    excluded_names: &selection.excluded_route_names,
                    excluded_agency_names: &[],
                    ignore_feed_window: selection.calendar_window,
                    frozen_comparison: true,
                };
                for movement in gtfs::convert(&directory.directory, &options, start, end).unwrap() {
                    write_record(&mut writer, &Record::Movement(movement.into())).unwrap();
                }
            }
        }
    }
    if let Some(cz) = plan.cz {
        for movement in
            super::czptt::frozen_sequences(&cz.sequences, &cz.stations, cz.source, &cz.country)
                .unwrap()
        {
            write_record(&mut writer, &Record::Movement(movement.into())).unwrap();
        }
    }
    let inputs = crate::year::YearInputs::read(&plan.manifest, &plan.sources).unwrap();
    let start =
        chrono::NaiveDate::parse_from_str(&inputs.manifest.flows.start, "%Y-%m-%d").unwrap();
    let end = chrono::NaiveDate::parse_from_str(&inputs.manifest.flows.end, "%Y-%m-%d").unwrap();
    let scratch = tempfile::tempdir().unwrap();
    for object in &inputs.manifest.railways.as_ref().unwrap().objects {
        if !matches!(
            object,
            crate::railways::config::RailObject::Gtfs { .. }
                | crate::railways::config::RailObject::Czptt { .. }
        ) {
            super::convert_object(&inputs, object, start, end, scratch.path(), &mut writer)
                .unwrap();
        }
    }
    writer.flush().unwrap();
}

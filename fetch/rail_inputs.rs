//! Convert only manifest-selected railway publisher objects into the shared movement stream.
#[path = "rail_inputs/calendar.rs"]
mod calendar;
#[path = "rail_inputs/crossings.rs"]
pub(crate) mod crossings;
#[path = "rail_inputs/czptt.rs"]
mod czptt;
#[path = "rail_inputs/gtfs.rs"]
mod gtfs;
#[path = "rail_inputs/gtfs_movements.rs"]
mod gtfs_movements;
#[path = "rail_inputs/gtfs_tables.rs"]
mod gtfs_tables;
#[path = "rail_inputs/spatial.rs"]
mod spatial;
#[path = "rail_inputs/station_names.rs"]
mod station_names;

use crate::{
    railways::{
        config::RailObject,
        movements::{Movement, Record, write_record},
    },
    year::YearInputs,
};
use std::{
    fs::File,
    io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::Path,
};

fn write_movement(writer: &mut impl Write, mut movement: Movement) -> Result<(), String> {
    movement.validate()?;
    movement.estimate_missing_calendar();
    movement.validate()?;
    write_record(writer, &Record::Movement(movement.into()))
}

fn unpack(path: &Path, directory: &Path) -> Result<(), String> {
    let mut input = File::open(path).map_err(|error| error.to_string())?;
    let mut magic = [0; 2];
    input
        .read_exact(&mut magic)
        .map_err(|error| error.to_string())?;
    input
        .seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    let input = if magic == [0x1f, 0x8b] {
        let mut decoded = tempfile::tempfile_in(directory).map_err(|error| error.to_string())?;
        std::io::copy(&mut flate2::read::GzDecoder::new(input), &mut decoded)
            .map_err(|error| error.to_string())?;
        decoded
            .seek(SeekFrom::Start(0))
            .map_err(|error| error.to_string())?;
        decoded
    } else {
        input
    };
    let mut archive = zip::ZipArchive::new(input).map_err(|error| error.to_string())?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
        let relative = entry
            .enclosed_name()
            .ok_or("GTFS archive path escapes its object")?;
        let output = directory.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&output).map_err(|error| error.to_string())?;
        } else {
            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            let mut file = File::create(output).map_err(|error| error.to_string())?;
            std::io::copy(&mut entry, &mut file).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

pub fn convert(inputs: &YearInputs, output: &Path, scratch: &Path) -> Result<(), String> {
    let plan = inputs
        .manifest
        .railways
        .as_ref()
        .ok_or("manifest lacks its railway source plan")?;
    if output.exists() || scratch.exists() {
        return Err("rail movement output and scratch must be new paths".into());
    }
    std::fs::create_dir(scratch).map_err(|error| error.to_string())?;
    let start = chrono::NaiveDate::parse_from_str(&inputs.manifest.flows.start, "%Y-%m-%d")
        .map_err(|error| error.to_string())?;
    let end = chrono::NaiveDate::parse_from_str(&inputs.manifest.flows.end, "%Y-%m-%d")
        .map_err(|error| error.to_string())?;
    let temporary = tempfile::NamedTempFile::new_in(output.parent().unwrap_or(Path::new(".")))
        .map_err(|error| error.to_string())?;
    let mut writer = BufWriter::new(temporary.as_file());
    write_record(
        &mut writer,
        &Record::Window {
            version: crate::railways::movements::STREAM_VERSION,
            start: start.to_string(),
            end: end.to_string(),
        },
    )?;
    for object in &plan.objects {
        convert_object(inputs, object, start, end, scratch, &mut writer)?;
    }
    writer.flush().map_err(|error| error.to_string())?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|error| error.to_string())?;
    drop(writer);
    std::fs::remove_dir(scratch).map_err(|error| error.to_string())?;
    temporary
        .persist_noclobber(output)
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn convert_object(
    inputs: &YearInputs,
    object: &RailObject,
    start: chrono::NaiveDate,
    end: chrono::NaiveDate,
    scratch: &Path,
    writer: &mut impl Write,
) -> Result<(), String> {
    let (id, _) = object.input();
    if !inputs.manifest.admitted(id)? {
        eprintln!("railways: {id}: not admitted by this year; declared fallback applies");
        return Ok(());
    }
    let input = inputs.by_id(id)?;
    match object {
        RailObject::Gtfs {
            source_id,
            country,
            directories,
            rail_route_types,
            tram_route_types,
            excluded_route_names,
            excluded_agency_names,
            ignore_feed_window,
            ..
        } => {
            let extracted = tempfile::tempdir_in(scratch).map_err(|error| error.to_string())?;
            let root = if input.is_dir() {
                input.as_path()
            } else {
                unpack(&input, extracted.path())?;
                extracted.path()
            };
            for directory in directories {
                let selected = root.join(directory);
                let nested = tempfile::tempdir_in(scratch).map_err(|error| error.to_string())?;
                let directory = if selected.is_dir() {
                    selected.as_path()
                } else {
                    unpack(&selected, nested.path())?;
                    nested.path()
                };
                let options = gtfs::Options {
                    #[cfg(test)]
                    frozen_comparison: false,
                    identity: id,
                    source_id: *source_id,
                    country,
                    rail_types: rail_route_types,
                    tram_types: tram_route_types,
                    excluded_names: excluded_route_names,
                    excluded_agency_names,
                    ignore_feed_window: *ignore_feed_window,
                };
                for movement in gtfs::convert(directory, &options, start, end)? {
                    write_movement(writer, movement)?;
                }
            }
        }
        RailObject::FraCrossings { source_id, .. } => {
            for crossing in crossings::parse_fra(&input, *source_id)? {
                write_record(writer, &Record::Crossing(crossing))?;
            }
        }
        RailObject::TcCrossings { source_id, .. } => {
            for crossing in crossings::parse_tc(&input, *source_id)? {
                write_record(writer, &Record::Crossing(crossing))?;
            }
        }
        RailObject::Czptt {
            stations,
            source_id,
            country,
            ..
        } => {
            let stations = inputs.by_id(stations)?;
            for movement in czptt::convert(&input, &stations, start, end, *source_id, country)? {
                write_movement(writer, movement)?;
            }
        }
        RailObject::Geojson { .. } => {
            for movement in spatial::convert(&input, object)? {
                write_movement(writer, movement)?;
            }
        }
        RailObject::PreparedMovements { .. } => {
            for line in
                BufReader::new(File::open(input).map_err(|error| error.to_string())?).lines()
            {
                let record: Record =
                    serde_json::from_str(&line.map_err(|error| error.to_string())?)
                        .map_err(|error| error.to_string())?;
                let Record::Movement(movement) = record else {
                    return Err("prior observations must contain normalized movements".into());
                };
                write_movement(writer, *movement)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "rail_inputs/fixtures.rs"]
mod fixtures;

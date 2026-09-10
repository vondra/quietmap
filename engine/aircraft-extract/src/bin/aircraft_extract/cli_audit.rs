//! Validate prepared aircraft schemas and both sampling windows against shuffle manifests.

use crate::cli_validate::{read_ga_n_days, read_window_n_days};
use aircraft_extract::{arrow_schemas as schemas, spatial::square_directories};
use anyhow::{Context, Result};
use arrow::{array::ListArray, datatypes::Schema, ipc::reader::FileReader};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

pub fn audit_prepared(prepared_year: &Path, shuffled: &Path) -> Result<()> {
    let primary = read_window_n_days(shuffled)?;
    let ga = read_ga_n_days(shuffled)?;
    let kinds = [
        (
            "airborne.arrow",
            schemas::with_n_days_and_windows(schemas::airborne_schema(), primary, ga),
        ),
        (
            "airport_traffic.arrow",
            schemas::with_n_days_and_windows(schemas::airport_traffic_schema(), primary, ga),
        ),
        (
            "cruise.arrow",
            schemas::with_n_days(schemas::cruise_schema(), primary),
        ),
        (
            "synth_airport_lines.arrow",
            schemas::synth_airport_lines_schema(),
        ),
        (
            "synth_airport_areas.arrow",
            schemas::synth_airport_areas_schema(),
        ),
    ];
    let (expected_airborne_files, expected_airborne_sub_segments) =
        aircraft_extract::shuffle::completion::inventory_counts(
            shuffled,
            aircraft_extract::flight::Phase::Airborne,
        )?;
    let mut files = 0usize;
    let mut airborne_files = 0_u64;
    let mut airborne_events = 0_u64;
    let mut airborne_sub_segments = 0_u64;
    let mut summaries = HashMap::new();
    for (_, square) in square_directories(prepared_year)? {
        for (name, schema) in &kinds {
            let path = square.join(name);
            if path.exists() {
                let counts = audit_file(
                    &path,
                    schema,
                    (*name == "airborne.arrow").then_some("sub_segments"),
                )
                .with_context(|| format!("audit {}", path.display()))?;
                if *name == "airborne.arrow" {
                    airborne_files += 1;
                    airborne_events = airborne_events
                        .checked_add(counts.rows)
                        .context("airborne event count overflow")?;
                    airborne_sub_segments = airborne_sub_segments
                        .checked_add(counts.nested_values)
                        .context("airborne sub-segment count overflow")?;
                }
                files += 1;
            }
        }
        if square.join("airport_traffic.arrow").try_exists()? {
            audit_airport_summary(&square, &mut summaries)?;
        }
    }
    anyhow::ensure!(
        files > 0,
        "no prepared aircraft files under {}",
        prepared_year.display()
    );
    anyhow::ensure!(
        (airborne_files, airborne_sub_segments)
            == (expected_airborne_files, expected_airborne_sub_segments),
        "prepared airborne census differs from sealed shuffle: files {airborne_files}/{expected_airborne_files}, sub-segments {airborne_sub_segments}/{expected_airborne_sub_segments}"
    );
    eprintln!(
        "aircraft audit: {files} files; {airborne_files} airborne files; {airborne_events} events; {airborne_sub_segments} sub-segments; airline days={primary}, GA days={ga}"
    );
    Ok(())
}

#[derive(Default)]
struct FileCounts {
    rows: u64,
    nested_values: u64,
}

fn audit_airport_summary(
    square: &Path,
    global: &mut HashMap<String, aircraft_extract::arrow_io::AirportSummaryRow>,
) -> Result<()> {
    use arrow::array::StringArray;
    let path = square.join("airport_summary.arrow");
    audit_file(&path, &schemas::airport_summary_schema(), None)
        .with_context(|| format!("audit required {}", path.display()))?;
    let mut keys = HashSet::new();
    for row in aircraft_extract::arrow_io::read_airport_summary(&path)? {
        anyhow::ensure!(
            keys.insert(row.airport_key.clone()),
            "duplicate airport summary at {}",
            path.display()
        );
        if let Some(previous) = global.get(&row.airport_key) {
            anyhow::ensure!(
                *previous == row,
                "airport summary disagrees for {} at {}",
                row.airport_key,
                path.display()
            );
        } else {
            global.insert(row.airport_key.clone(), row);
        }
    }
    for batch in FileReader::try_new(File::open(square.join("airport_traffic.arrow"))?, None)? {
        let batch = batch?;
        let airports = batch
            .column_by_name("airport_key")
            .and_then(|array| array.as_any().downcast_ref::<StringArray>())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "airport_traffic missing airport_key at {}",
                    square.display()
                )
            })?;
        for key in airports.iter() {
            anyhow::ensure!(
                key.is_some_and(|key| keys.contains(key)),
                "airport_summary.arrow missing airport {key:?} at {}",
                square.display()
            );
        }
    }
    Ok(())
}

fn audit_file(
    path: &Path,
    expected: &Arc<Schema>,
    nested_column: Option<&str>,
) -> Result<FileCounts> {
    let reader = FileReader::try_new(File::open(path)?, None)?;
    let schema = reader.schema();
    anyhow::ensure!(
        schema.fields() == expected.fields(),
        "incompatible aircraft columns"
    );
    for (key, value) in expected.metadata() {
        anyhow::ensure!(
            schema.metadata().get(key) == Some(value),
            "wrong {key}: expected {value}, got {:?}",
            schema.metadata().get(key)
        );
    }
    let mut counts = FileCounts::default();
    for batch in reader {
        let batch = batch?;
        counts.rows = counts
            .rows
            .checked_add(u64::try_from(batch.num_rows())?)
            .context("record row count overflow")?;
        if let Some(name) = nested_column {
            let values = batch
                .column_by_name(name)
                .and_then(|array| array.as_any().downcast_ref::<ListArray>())
                .with_context(|| format!("missing List column {name}"))?;
            let offsets = values.value_offsets();
            let nested = offsets
                .last()
                .zip(offsets.first())
                .and_then(|(last, first)| last.checked_sub(*first))
                .context("invalid List offsets")?;
            counts.nested_values = counts
                .nested_values
                .checked_add(u64::try_from(nested)?)
                .context("nested value count overflow")?;
        }
    }
    Ok(counts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::ipc::writer::FileWriter;

    fn write_airborne_inventory(shuffled: &Path, rows: u64) {
        let database = rusqlite::Connection::open(shuffled.join("complete.sqlite")).unwrap();
        database
            .execute_batch("CREATE TABLE files(path TEXT PRIMARY KEY, rows INTEGER NOT NULL);")
            .unwrap();
        database
            .execute(
                "INSERT INTO files VALUES (?1,?2)",
                ("z9/276/173/airborne.arrow", rows),
            )
            .unwrap();
    }

    #[test]
    fn publish_audit_requires_summary_beside_traffic_not_a_global_file() {
        let temp = tempfile::tempdir().unwrap();
        let year = temp.path().join("prepared");
        let shuffled = temp.path().join("shuffled");
        std::fs::create_dir_all(year.join("z9/276/173")).unwrap();
        std::fs::create_dir_all(&shuffled).unwrap();
        std::fs::write(shuffled.join("days"), "2025-01-01\n").unwrap();
        std::fs::write(shuffled.join("ga_days"), "").unwrap();
        write_airborne_inventory(&shuffled, 0);
        aircraft_extract::arrow_io::write_airborne(
            &year.join("z9/276/173/airborne.arrow"),
            &[],
            1,
            0,
        )
        .unwrap();
        audit_prepared(&year, &shuffled).unwrap();
        aircraft_extract::arrow_io::write_airport_traffic(
            &year.join("z9/276/173/airport_traffic.arrow"),
            &[],
            1,
            0,
        )
        .unwrap();
        let error = audit_prepared(&year, &shuffled).unwrap_err();
        assert!(error.to_string().contains("airport_summary.arrow"));
        let summary = year.join("z9/276/173/airport_summary.arrow");
        std::fs::create_dir_all(summary.parent().unwrap()).unwrap();
        FileWriter::try_new(File::create(&summary).unwrap(), &schemas::cruise_schema())
            .unwrap()
            .finish()
            .unwrap();
        assert!(audit_prepared(&year, &shuffled).is_err());
        aircraft_extract::arrow_io::write_airport_summary(&summary, &[]).unwrap();
        audit_prepared(&year, &shuffled).unwrap();
    }

    #[test]
    fn prepared_airborne_must_preserve_sealed_sub_segment_count() {
        let temp = tempfile::tempdir().unwrap();
        let year = temp.path().join("prepared");
        let shuffled = temp.path().join("shuffled");
        std::fs::create_dir_all(year.join("z9/276/173")).unwrap();
        std::fs::create_dir_all(&shuffled).unwrap();
        std::fs::write(shuffled.join("days"), "2025-01-01\n").unwrap();
        std::fs::write(shuffled.join("ga_days"), "").unwrap();
        write_airborne_inventory(&shuffled, 1);
        aircraft_extract::arrow_io::write_airborne(
            &year.join("z9/276/173/airborne.arrow"),
            &[],
            1,
            0,
        )
        .unwrap();
        let error = audit_prepared(&year, &shuffled).unwrap_err();
        assert!(error.to_string().contains("sub-segments 0/1"));
    }

    #[test]
    fn both_class_windows_must_match_even_when_no_rows() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("airborne.arrow");
        let actual = schemas::with_n_days_and_windows(schemas::airborne_schema(), 12, 365);
        FileWriter::try_new(File::create(&path).unwrap(), &actual)
            .unwrap()
            .finish()
            .unwrap();
        assert!(audit_file(&path, &actual, Some("sub_segments")).is_ok());
        for wrong in [
            schemas::with_n_days_and_windows(schemas::airborne_schema(), 11, 365),
            schemas::with_n_days_and_windows(schemas::airborne_schema(), 12, 364),
        ] {
            assert!(audit_file(&path, &wrong, Some("sub_segments")).is_err());
        }
    }
}

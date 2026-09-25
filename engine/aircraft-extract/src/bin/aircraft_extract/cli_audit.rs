//! Validate prepared aircraft schemas and the sampling window against shuffle manifests.

use aircraft_extract::{arrow_schemas as schemas, spatial::square_directories};
use anyhow::{Context, Result};
use arrow::{datatypes::Schema, ipc::reader::FileReader};
use noise_compute::compute::aircraft_v6::airport_traffic::AirportSummaryEntry;
use square_store::aircraft_contract::AIRPORT_SUMMARIES_KEY;
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

pub fn audit_prepared(prepared_year: &Path, shuffled: &Path) -> Result<()> {
    let window = aircraft_extract::shuffle::completion::sampling_window(shuffled)?;
    let kinds = [
        (
            "airborne.arrow",
            schemas::with_sampling_window(schemas::airborne_schema(), &window),
        ),
        (
            "airport_traffic.arrow",
            schemas::with_sampling_window(schemas::airport_traffic_schema(), &window),
        ),
        (
            "cruise.arrow",
            schemas::with_sampling_window(schemas::cruise_schema(), &window),
        ),
        (
            "synth_airport_lines.arrow",
            schemas::synth_airport_lines_schema(),
        ),
    ];
    // The sealed shuffle counts owned pieces; Stage 2A writes one row each.
    let (expected_airborne_files, expected_airborne_rows) =
        aircraft_extract::shuffle::completion::inventory_counts(
            shuffled,
            aircraft_extract::flight::Phase::Airborne,
        )?;
    let mut files = 0usize;
    let mut airborne_files = 0_u64;
    let mut airborne_rows = 0_u64;
    let mut summaries = HashMap::new();
    for (_, square) in square_directories(prepared_year)? {
        for (name, schema) in &kinds {
            let path = square.join(name);
            if path.exists() {
                let rows = audit_file(&path, schema)
                    .with_context(|| format!("audit {}", path.display()))?;
                if *name == "airborne.arrow" {
                    airborne_files += 1;
                    airborne_rows = airborne_rows
                        .checked_add(rows)
                        .context("airborne row count overflow")?;
                }
                files += 1;
            }
        }
        if square.join("airport_traffic.arrow").try_exists()? {
            audit_airport_summaries(&square, &mut summaries)?;
        }
    }
    anyhow::ensure!(
        files > 0,
        "no prepared aircraft files under {}",
        prepared_year.display()
    );
    anyhow::ensure!(
        (airborne_files, airborne_rows) == (expected_airborne_files, expected_airborne_rows),
        "prepared airborne census differs from sealed shuffle: files {airborne_files}/{expected_airborne_files}, rows {airborne_rows}/{expected_airborne_rows}"
    );
    eprintln!(
        "aircraft audit: {files} files; {airborne_files} airborne files; {airborne_rows} sub-segment rows; baseline days={}, increment days={}",
        window.baseline_days,
        window.increment_days
    );
    Ok(())
}

/// Every traffic file carries identical global unions for the airports of its rows.
fn audit_airport_summaries(
    square: &Path,
    global: &mut HashMap<String, AirportSummaryEntry>,
) -> Result<()> {
    use arrow::array::StringArray;
    let path = square.join("airport_traffic.arrow");
    let summaries = aircraft_extract::arrow_io::read_airport_summaries(&path)?;
    for (key, entry) in &summaries {
        if let Some(previous) = global.get(key) {
            anyhow::ensure!(
                previous == entry,
                "airport summary disagrees for {key} at {}",
                path.display()
            );
        } else {
            global.insert(key.clone(), *entry);
        }
    }
    for batch in FileReader::try_new(File::open(&path)?, None)? {
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
                key.is_some_and(|key| summaries.contains_key(key)),
                "{AIRPORT_SUMMARIES_KEY} missing airport {key:?} at {}",
                path.display()
            );
        }
    }
    Ok(())
}

/// Row count of a prepared aircraft file whose columns and stamps match `expected`.
fn audit_file(path: &Path, expected: &Arc<Schema>) -> Result<u64> {
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
    let mut rows = 0_u64;
    for batch in reader {
        rows = rows
            .checked_add(u64::try_from(batch?.num_rows())?)
            .context("record row count overflow")?;
    }
    Ok(rows)
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
    fn publish_audit_requires_stamped_summaries_in_every_traffic_footer() {
        let temp = tempfile::tempdir().unwrap();
        let year = temp.path().join("prepared");
        let shuffled = temp.path().join("shuffled");
        std::fs::create_dir_all(year.join("z9/276/173")).unwrap();
        std::fs::create_dir_all(&shuffled).unwrap();
        let window = write_manifests(&shuffled);
        write_airborne_inventory(&shuffled, 0);
        aircraft_extract::arrow_io::write_airborne(
            &year.join("z9/276/173/airborne.arrow"),
            &[],
            &window,
        )
        .unwrap();
        audit_prepared(&year, &shuffled).unwrap();
        let traffic = year.join("z9/276/173/airport_traffic.arrow");
        aircraft_extract::arrow_io::write_airport_traffic(&traffic, &[], &window).unwrap();
        let error = audit_prepared(&year, &shuffled).unwrap_err();
        assert!(error.to_string().contains(AIRPORT_SUMMARIES_KEY));
        aircraft_extract::arrow_io::stamp_airport_summaries(&traffic, &Default::default()).unwrap();
        audit_prepared(&year, &shuffled).unwrap();
    }

    #[test]
    fn prepared_airborne_must_preserve_sealed_row_count() {
        let temp = tempfile::tempdir().unwrap();
        let year = temp.path().join("prepared");
        let shuffled = temp.path().join("shuffled");
        std::fs::create_dir_all(year.join("z9/276/173")).unwrap();
        std::fs::create_dir_all(&shuffled).unwrap();
        let window = write_manifests(&shuffled);
        write_airborne_inventory(&shuffled, 1);
        aircraft_extract::arrow_io::write_airborne(
            &year.join("z9/276/173/airborne.arrow"),
            &[],
            &window,
        )
        .unwrap();
        let error = audit_prepared(&year, &shuffled).unwrap_err();
        assert!(error.to_string().contains("rows 0/1"));
    }

    #[test]
    fn the_sampling_window_must_match_even_when_no_rows() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("airborne.arrow");
        let shuffled = temp.path().join("shuffled");
        std::fs::create_dir_all(&shuffled).unwrap();
        let window = write_manifests(&shuffled);
        let actual = schemas::with_sampling_window(schemas::airborne_schema(), &window);
        FileWriter::try_new(File::create(&path).unwrap(), &actual)
            .unwrap()
            .finish()
            .unwrap();
        assert!(audit_file(&path, &actual).is_ok());
        let mut other_increment = window.clone();
        other_increment.increment_days_sha256 = "another day list".into();
        let mut other_baseline = window;
        other_baseline.baseline_days += 1;
        for wrong in [other_increment, other_baseline] {
            let expected = schemas::with_sampling_window(schemas::airborne_schema(), &wrong);
            assert!(audit_file(&path, &expected).is_err());
        }
    }

    fn write_manifests(shuffled: &Path) -> noise_compute::emission::aircraft::SamplingWindow {
        use aircraft_extract::shuffle::completion::*;
        std::fs::write(shuffled.join(BASELINE_DAYS_MANIFEST), "2025-01-01\n2025-01-02").unwrap();
        std::fs::write(shuffled.join(INCREMENT_DAYS_MANIFEST), "2025-01-01").unwrap();
        sampling_window(shuffled).unwrap()
    }
}

//! Validate prepared aircraft schemas and both sampling windows against shuffle manifests.

use crate::cli_validate::{read_ga_n_days, read_window_n_days};
use aircraft_extract::{arrow_schemas as schemas, spatial::square_directories};
use anyhow::{Context, Result};
use arrow::{datatypes::Schema, ipc::reader::FileReader};
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
    let summary = prepared_year.join("aircraft/airport_summary.arrow");
    audit_file(&summary, &schemas::airport_summary_schema())
        .with_context(|| format!("audit required {}", summary.display()))?;
    let mut files = 0usize;
    for (_, square) in square_directories(prepared_year)? {
        for (name, schema) in &kinds {
            let path = square.join(name);
            if path.exists() {
                audit_file(&path, schema).with_context(|| format!("audit {}", path.display()))?;
                files += 1;
            }
        }
    }
    anyhow::ensure!(
        files > 0,
        "no prepared aircraft files under {}",
        prepared_year.display()
    );
    eprintln!("aircraft audit: {files} files; airline days={primary}, GA days={ga}");
    Ok(())
}

fn audit_file(path: &Path, expected: &Arc<Schema>) -> Result<()> {
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
    for batch in reader {
        batch?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::ipc::writer::FileWriter;

    #[test]
    fn publish_audit_requires_current_global_summary() {
        let temp = tempfile::tempdir().unwrap();
        let year = temp.path().join("prepared");
        let shuffled = temp.path().join("shuffled");
        std::fs::create_dir_all(year.join("z9/276/173")).unwrap();
        std::fs::create_dir_all(&shuffled).unwrap();
        std::fs::write(shuffled.join("days"), "2025-01-01\n").unwrap();
        std::fs::write(shuffled.join("ga_days"), "").unwrap();
        aircraft_extract::arrow_io::write_airborne(
            &year.join("z9/276/173/airborne.arrow"),
            &[],
            1,
            0,
        )
        .unwrap();
        let error = audit_prepared(&year, &shuffled).unwrap_err();
        assert!(error.to_string().contains("airport_summary.arrow"));
        let summary = year.join("aircraft/airport_summary.arrow");
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
    fn both_class_windows_must_match_even_when_no_rows() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("airborne.arrow");
        let actual = schemas::with_n_days_and_windows(schemas::airborne_schema(), 12, 365);
        FileWriter::try_new(File::create(&path).unwrap(), &actual)
            .unwrap()
            .finish()
            .unwrap();
        assert!(audit_file(&path, &actual).is_ok());
        for wrong in [
            schemas::with_n_days_and_windows(schemas::airborne_schema(), 11, 365),
            schemas::with_n_days_and_windows(schemas::airborne_schema(), 12, 364),
        ] {
            assert!(audit_file(&path, &wrong).is_err());
        }
    }
}

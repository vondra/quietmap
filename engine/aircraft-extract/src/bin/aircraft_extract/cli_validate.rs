//! Exact day sets, typed segment validation, and prerequisites for aircraft stage reuse.

use crate::{source_cache::SourceCache, FromStage};
use aircraft_extract::{arrow_schemas, period::parse_date_id, scope::ScopeBbox};
use anyhow::{Context, Result};
use arrow::{
    array::{Array, Int16Array, UInt8Array},
    ipc::reader::FileReader,
};
use std::collections::BTreeSet;
use std::fs::File;
use std::path::{Path, PathBuf};

pub fn validate_fresh_stage0_work(
    work_dir: &Path,
    days: &[String],
    until_stage: FromStage,
    cache: Option<&SourceCache>,
) -> Result<()> {
    for day in days {
        parse_date_id(day)?;
        for stage in ["flights", "segments"] {
            let path = work_dir.join(stage).join(format!("{day}.arrow"));
            anyhow::ensure!(
                !path.try_exists()? && !path.is_symlink(),
                "requested fresh Stage0 output already exists: {}",
                path.display()
            );
        }
    }
    for stage in ["segments_by_square", "flights", "segments"] {
        let dir = work_dir.join(stage);
        if !dir.try_exists()? || std::fs::read_dir(&dir)?.next().transpose()?.is_none() {
            continue;
        }
        anyhow::ensure!(
            stage != "segments_by_square" && until_stage <= FromStage::Stage1,
            "cannot append Stage0 through shuffle or publication: {}",
            dir.display()
        );
        let cache =
            cache.context("populated Stage0 work requires publisher-bound source receipts")?;
        let paths = list_segments_day_paths(&dir)?;
        anyhow::ensure!(
            paths.len()
                == std::fs::read_dir(&dir)?
                    .collect::<std::io::Result<Vec<_>>>()?
                    .len(),
            "unexpected entry in Stage0 work directory: {}",
            dir.display()
        );
        let existing_days: Vec<_> = paths
            .iter()
            .map(|path| path.file_stem().unwrap().to_str().unwrap().to_owned())
            .collect();
        cache.validate(Some(&existing_days), Some(stage))?;
    }
    Ok(())
}

pub fn require_input_dir_exists(flag: &str, dir: &Path) -> Result<()> {
    anyhow::ensure!(
        dir.is_dir(),
        "{flag} is not an existing directory: {}",
        dir.display()
    );
    Ok(())
}

pub fn list_segments_day_paths(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "arrow") {
            anyhow::ensure!(
                entry.file_type()?.is_file(),
                "not a day file: {}",
                path.display()
            );
            parse_date_id(
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .context("invalid day filename")?,
            )?;
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

pub fn list_segments_day_paths_multi(dirs: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for dir in dirs {
        paths.extend(list_segments_day_paths(dir)?);
    }
    paths.sort();
    let names: BTreeSet<_> = paths.iter().map(|p| p.file_stem()).collect();
    anyhow::ensure!(
        names.len() == paths.len(),
        "duplicate segment day across input directories"
    );
    Ok(paths)
}

pub(crate) fn validated_days(
    days: impl IntoIterator<Item = String>,
    allow_empty: bool,
) -> Result<BTreeSet<String>> {
    let mut set = BTreeSet::new();
    for day in days {
        parse_date_id(&day)?;
        anyhow::ensure!(set.insert(day.clone()), "duplicate sampling day {day}");
    }
    anyhow::ensure!(
        allow_empty || !set.is_empty(),
        "empty primary sampling window"
    );
    u16::try_from(set.len()).context("too many sampling days")?;
    Ok(set)
}

/// Completed day shards: schema, day and provider provenance, plus the
/// primary publisher receipts when the primary archive has a catalog.
pub fn validate_segments(
    dir: &Path,
    days: &[String],
    primary: Option<&SourceCache>,
    sources: [u8; 2],
) -> Result<()> {
    if let Some(cache) = primary {
        let work = dir
            .parent()
            .context("segments directory has no work parent")?;
        SourceCache::new(cache.root(), work).validate(Some(days), Some("segments"))?;
    }
    let expected = validated_days(days.iter().cloned(), false)?;
    let paths = list_segments_day_paths(dir)?;
    let present: BTreeSet<_> = paths
        .iter()
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    anyhow::ensure!(
        present.is_superset(&expected),
        "segment day set misses {:?}",
        expected.difference(&present).collect::<Vec<_>>()
    );
    for path in paths {
        let day = path.file_stem().unwrap().to_str().unwrap().to_owned();
        if !expected.contains(&day) {
            continue;
        }
        let date_id = parse_date_id(&day)?;
        let reader = FileReader::try_new(File::open(&path)?, None)?;
        let schema = reader.schema();
        let expected_schema = arrow_schemas::segments_schema();
        anyhow::ensure!(
            schema.fields() == expected_schema.fields(),
            "incompatible segment schema: {}",
            path.display()
        );
        for (key, value) in expected_schema.metadata() {
            anyhow::ensure!(
                schema.metadata().get(key) == Some(value),
                "incompatible {key}: {}",
                path.display()
            );
        }
        for batch in reader {
            let batch = batch?;
            anyhow::ensure!(
                batch.columns().iter().all(|c| c.null_count() == 0),
                "null segment field: {}",
                path.display()
            );
            let column = |name: &str| batch.column_by_name(name).unwrap();
            let dates = column("date_id")
                .as_any()
                .downcast_ref::<Int16Array>()
                .unwrap();
            let providers = column("source_id")
                .as_any()
                .downcast_ref::<UInt8Array>()
                .unwrap();
            for i in 0..batch.num_rows() {
                anyhow::ensure!(
                    dates.value(i) == date_id && sources.contains(&providers.value(i)),
                    "wrong date or provider at {} row {i}",
                    path.display()
                );
            }
        }
    }
    Ok(())
}

/// Validate one exact primary window while preserving each source work directory.
pub fn reuse_segments_from_directories(
    dirs: &[PathBuf],
    days: &BTreeSet<String>,
    primary: Option<&SourceCache>,
    sources: [u8; 2],
) -> Result<Vec<PathBuf>> {
    let paths = list_segments_day_paths_multi(dirs)?;
    let present: BTreeSet<_> = paths
        .iter()
        .map(|path| path.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    anyhow::ensure!(
        present == *days,
        "segment day set mismatch: missing {:?}, unexpected {:?}",
        days.difference(&present).collect::<Vec<_>>(),
        present.difference(days).collect::<Vec<_>>()
    );
    for dir in dirs {
        let selected: Vec<_> = paths
            .iter()
            .filter(|path| path.parent() == Some(dir.as_path()))
            .map(|path| path.file_stem().unwrap().to_string_lossy().into_owned())
            .collect();
        if !selected.is_empty() {
            validate_segments(dir, &selected, primary, sources)?;
        }
    }
    Ok(paths)
}

pub fn parse_scope(s: Option<&str>) -> Result<Option<ScopeBbox>> {
    s.map(ScopeBbox::parse)
        .transpose()
        .map_err(|e| anyhow::anyhow!("--scope-bbox: {e}"))
}

pub fn require_scope_for_subset_cache(cache: &Path, scope: Option<&ScopeBbox>) -> Result<()> {
    let subset = cache
        .components()
        .any(|part| part.as_os_str() == "bbox" || part.as_os_str() == "radius");
    anyhow::ensure!(
        !subset || scope.is_some(),
        "subset ADS-B cache requires --scope-bbox: {}",
        cache.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_day_shards_must_hold_every_requested_day_of_known_providers() {
        use aircraft_extract::flight::source_id;
        let temp = tempfile::tempdir().unwrap();
        let day = temp.path().join("2025-01-01.arrow");
        aircraft_extract::arrow_io::write_segments(&day, &[]).unwrap();
        let providers = [source_id::ADSB_LOL_TAR, source_id::ADSB_EXCHANGE];
        assert!(validate_segments(temp.path(), &["2025-01-01".into()], None, providers).is_ok());
        assert!(validate_segments(temp.path(), &["2025-02-01".into()], None, providers).is_err());
        let mut segment = aircraft_extract::flight::FlightSegment {
            flight_id: 1,
            callsign: "TEST".into(),
            aircraft_type: *b"B738",
            profile_idx: aircraft_extract::profile::profile_idx("B738"),
            source_id: source_id::SCHEDULE_SYNTH,
            origin: 0,
            veh_kind: 0,
            gse_class: 0,
            period: 0,
            date_id: parse_date_id("2025-01-01").unwrap(),
            phase: aircraft_extract::flight::Phase::Airborne,
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
        aircraft_extract::arrow_io::write_segments(&day, std::slice::from_ref(&segment)).unwrap();
        assert!(validate_segments(temp.path(), &["2025-01-01".into()], None, providers).is_err());
        segment.source_id = source_id::ADSB_EXCHANGE;
        aircraft_extract::arrow_io::write_segments(&day, &[segment]).unwrap();
        assert!(validate_segments(temp.path(), &["2025-01-01".into()], None, providers).is_ok());
        std::fs::write(day, "broken Arrow").unwrap();
        assert!(validate_segments(temp.path(), &["2025-01-01".into()], None, providers).is_err());
    }
}

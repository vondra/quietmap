//! Exact day sets, typed segment validation, and prerequisites for aircraft stage reuse.

use crate::{ClassFilterArg, Feed, FromStage, source_cache::SourceCache};
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

fn validated_days(
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

pub fn read_window_days(dir: &Path, name: &str) -> Result<BTreeSet<String>> {
    let path = dir.join(name);
    let contents = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "missing sampling manifest {}; rerun shuffle",
            path.display()
        )
    })?;
    validated_days(contents.lines().map(str::to_owned), name == "ga_days")
}

pub fn read_window_n_days(dir: &Path) -> Result<u16> {
    Ok(u16::try_from(read_window_days(dir, "days")?.len())?)
}

pub fn read_ga_n_days(dir: &Path) -> Result<u16> {
    Ok(u16::try_from(read_window_days(dir, "ga_days")?.len())?)
}

pub fn require_matching_window_days(dir: &Path, paths: &[PathBuf]) -> Result<()> {
    let supplied: BTreeSet<_> = paths
        .iter()
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    anyhow::ensure!(
        supplied == read_window_days(dir, "days")?,
        "cruise days differ from shuffled days; rerun shuffle for the requested day set"
    );
    Ok(())
}

pub fn validate_segments(
    dir: &Path,
    days: &[String],
    filter: ClassFilterArg,
    feed: Feed,
    adsb_cache: &Path,
) -> Result<()> {
    let selected_days;
    let days = if matches!(feed, Feed::Adsblol) {
        let work = dir
            .parent()
            .context("segments directory has no work parent")?;
        let cache = SourceCache::new(adsb_cache, work, filter);
        selected_days = cache
            .validate(Some(days), None)?
            .into_keys()
            .collect::<Vec<_>>();
        cache.validate(Some(&selected_days), Some("segments"))?;
        &selected_days
    } else {
        days
    };
    let expected = validated_days(days.iter().cloned(), false)?;
    let paths = list_segments_day_paths(dir)?;
    let present: BTreeSet<_> = paths
        .iter()
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    anyhow::ensure!(
        present == expected,
        "segment day set mismatch: missing {:?}, unexpected {:?}",
        expected.difference(&present).collect::<Vec<_>>(),
        present.difference(&expected).collect::<Vec<_>>()
    );
    for path in paths {
        let date_id = parse_date_id(path.file_stem().unwrap().to_str().unwrap())?;
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
            let dates = batch
                .column_by_name("date_id")
                .unwrap()
                .as_any()
                .downcast_ref::<Int16Array>()
                .unwrap();
            let profiles = batch
                .column_by_name("profile_idx")
                .unwrap()
                .as_any()
                .downcast_ref::<UInt8Array>()
                .unwrap();
            let vehicles = batch
                .column_by_name("veh_kind")
                .unwrap()
                .as_any()
                .downcast_ref::<UInt8Array>()
                .unwrap();
            let sources = batch
                .column_by_name("source_id")
                .unwrap()
                .as_any()
                .downcast_ref::<UInt8Array>()
                .unwrap();
            for i in 0..batch.num_rows() {
                let ga = vehicles.value(i) == 0
                    && aircraft_extract::profile::is_ga_sampled_profile(profiles.value(i));
                let allowed = match filter {
                    ClassFilterArg::All => true,
                    ClassFilterArg::Ga => ga,
                    ClassFilterArg::NonGa => !ga,
                };
                anyhow::ensure!(
                    dates.value(i) == date_id && sources.value(i) == feed.source_id() && allowed,
                    "wrong date, feed, or hybrid class at {} row {i}",
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
    days: &[String],
    filter: ClassFilterArg,
    feed: Feed,
    adsb_cache: &Path,
) -> Result<Vec<PathBuf>> {
    let paths = list_segments_day_paths_multi(dirs)?;
    let expected = validated_days(days.iter().cloned(), false)?;
    let present: BTreeSet<_> = paths
        .iter()
        .map(|path| path.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    anyhow::ensure!(
        present == expected,
        "segment day set mismatch: missing {:?}, unexpected {:?}",
        expected.difference(&present).collect::<Vec<_>>(),
        present.difference(&expected).collect::<Vec<_>>()
    );
    for dir in dirs {
        let selected: Vec<_> = paths
            .iter()
            .filter(|path| path.parent() == Some(dir.as_path()))
            .map(|path| path.file_stem().unwrap().to_string_lossy().into_owned())
            .collect();
        validate_segments(dir, &selected, filter, feed, adsb_cache)?;
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
    fn sampling_windows_require_exact_valid_day_lists() {
        let temp = tempfile::tempdir().unwrap();
        assert!(read_ga_n_days(temp.path()).is_err());
        std::fs::write(temp.path().join("days"), "2025-01-01\n2025-02-01\n").unwrap();
        std::fs::write(
            temp.path().join("ga_days"),
            "2025-01-01\n2025-01-02\n2025-01-03\n",
        )
        .unwrap();
        assert_eq!(read_window_n_days(temp.path()).unwrap(), 2);
        assert_eq!(read_ga_n_days(temp.path()).unwrap(), 3);
        assert!(
            require_matching_window_days(
                temp.path(),
                &["2025-01-01.arrow".into(), "2025-03-01.arrow".into()]
            )
            .is_err()
        );
        std::fs::write(temp.path().join("ga_days"), "").unwrap();
        assert_eq!(read_ga_n_days(temp.path()).unwrap(), 0);
        for invalid in ["2025-01-01\n2025-01-01\n", "2025-02-30\n", ""] {
            std::fs::write(temp.path().join("days"), invalid).unwrap();
            assert!(read_window_n_days(temp.path()).is_err());
        }
    }

    #[test]
    fn same_count_wrong_days_and_truncated_pass_cannot_resume() {
        let temp = tempfile::tempdir().unwrap();
        let day = temp.path().join("2025-01-01.arrow");
        aircraft_extract::arrow_io::write_segments(&day, &[]).unwrap();
        assert!(
            validate_segments(
                temp.path(),
                &["2025-01-01".into()],
                ClassFilterArg::Ga,
                Feed::Adsbexchange,
                temp.path()
            )
            .is_ok()
        );
        assert!(
            validate_segments(
                temp.path(),
                &["2025-02-01".into()],
                ClassFilterArg::Ga,
                Feed::Adsbexchange,
                temp.path()
            )
            .is_err()
        );
        assert!(
            validate_segments(
                temp.path(),
                &["2025-01-01".into(), "2025-02-01".into()],
                ClassFilterArg::Ga,
                Feed::Adsbexchange,
                temp.path()
            )
            .is_err()
        );
        std::fs::write(day, "broken Arrow").unwrap();
        assert!(
            validate_segments(
                temp.path(),
                &["2025-01-01".into()],
                ClassFilterArg::Ga,
                Feed::Adsbexchange,
                temp.path()
            )
            .is_err()
        );
    }
}

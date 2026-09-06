//! The existing Python publisher authority binds native GA inputs and completed work receipts.

use crate::ClassFilterArg;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct SourceCache {
    root: PathBuf,
    work: PathBuf,
    filter: ClassFilterArg,
}

impl SourceCache {
    pub fn new(root: &Path, work: &Path, filter: ClassFilterArg) -> Self {
        Self {
            root: root.into(),
            work: work.into(),
            filter,
        }
    }

    pub fn class_filter(&self) -> aircraft_extract::source_adsb_tar::ClassWindowFilter {
        self.filter.window()
    }

    pub fn validate(
        &self,
        days: Option<&[String]>,
        stage: Option<&str>,
    ) -> Result<BTreeMap<String, Vec<PathBuf>>> {
        self.invoke(days, stage.map(|s| (s, "check")))
    }

    pub fn begin(&self, day: &str, stage: &str) -> Result<BTreeMap<String, Vec<PathBuf>>> {
        self.invoke(Some(&[day.to_owned()]), Some((stage, "begin")))
    }

    pub fn complete(&self, day: &str, stage: &str) -> Result<()> {
        self.invoke(Some(&[day.to_owned()]), Some((stage, "complete")))?;
        Ok(())
    }

    fn invoke(
        &self,
        days: Option<&[String]>,
        receipt: Option<(&str, &str)>,
    ) -> Result<BTreeMap<String, Vec<PathBuf>>> {
        let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut command = Command::new("python3");
        command
            .arg(project.join("scripts/download-adsblol.py"))
            .arg("validate")
            .arg("--source-root")
            .arg(&self.root)
            .arg("--class-filter")
            .arg(match self.filter {
                ClassFilterArg::All => "all",
                ClassFilterArg::Ga => "ga",
                ClassFilterArg::NonGa => "non-ga",
            });
        if let Some(days) = days {
            command.arg("--days").arg(days.join(","));
        }
        if let Some((stage, action)) = receipt {
            command
                .arg("--work-dir")
                .arg(&self.work)
                .arg("--stage")
                .arg(stage)
                .arg("--action")
                .arg(action);
        }
        let output = command
            .output()
            .context("run existing publisher/source-cache validator")?;
        anyhow::ensure!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        let fields = output
            .stdout
            .strip_suffix(&[0])
            .context("empty source validator response")?
            .split(|&byte| byte == 0)
            .collect::<Vec<_>>();
        anyhow::ensure!(fields.len() % 2 == 0, "invalid source path transport");
        let mut selected = BTreeMap::<String, Vec<PathBuf>>::new();
        for pair in fields.chunks_exact(2) {
            let day = std::str::from_utf8(pair[0])?;
            aircraft_extract::period::parse_date_id(day)?;
            let path = PathBuf::from(std::str::from_utf8(pair[1])?);
            selected.entry(day.to_owned()).or_default().push(path);
        }
        Ok(selected)
    }
}

pub fn validate_ga_merge(dirs: &[PathBuf], root: Option<&Path>) -> Result<()> {
    anyhow::ensure!(
        dirs.is_empty() == root.is_none(),
        "GA segments and --ga-adsb-cache are required together"
    );
    let Some(root) = root else {
        return Ok(());
    };
    let expected =
        SourceCache::new(root, Path::new("."), ClassFilterArg::Ga).validate(None, None)?;
    let paths = crate::cli_validate::list_segments_day_paths_multi(dirs)?;
    let present: std::collections::BTreeSet<_> = paths
        .iter()
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    anyhow::ensure!(
        present == expected.keys().cloned().collect(),
        "GA merge must contain every selected complete-source sampling day"
    );
    for dir in dirs {
        let days: Vec<_> = crate::cli_validate::list_segments_day_paths(dir)?
            .iter()
            .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
            .collect();
        crate::cli_validate::validate_segments(
            dir,
            &days,
            ClassFilterArg::Ga,
            crate::Feed::Adsblol,
            root,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Feed, FromStage};

    fn catalog(root: &Path, kind: &str, days: &[&str], mlat_days: &[&str]) {
        let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let output = Command::new("python3").current_dir(&project)
            .args(["-c", "import runpy,sys; sys.path.insert(0,'scripts'); m=runpy.run_path('scripts/test_download_adsblol.py'); m['create_selected_catalog'](sys.argv[1],kind=sys.argv[2],days=sys.argv[3].split(','),mlat_days=sys.argv[4].split(','))"])
            .arg(root).arg(kind).arg(days.join(",")).arg(mlat_days.join(",")).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn native_empty_ipc_requires_completed_source_and_parent_receipts_at_every_ga_entry() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("source");
        catalog(&root, "staging", &["2026-06-06"], &[]);
        let work = temp.path().join("work");
        let day = "2026-06-06".to_owned();
        let run = |from, until| {
            crate::cli_run_all::run_all(
                root.clone(),
                temp.path().join("prepared-year"),
                temp.path().join("prepared"),
                work.clone(),
                vec![day.clone()],
                None,
                from,
                until,
                Feed::Adsblol,
                ClassFilterArg::Ga,
                None,
                None,
                false,
            )
        };
        run(FromStage::Stage0, FromStage::Stage0).unwrap();
        let cache = SourceCache::new(&root, &work, ClassFilterArg::Ga);
        cache
            .validate(Some(std::slice::from_ref(&day)), Some("flights"))
            .unwrap();
        assert!(
            aircraft_extract::stage_1::read_flights(&work.join("flights/2026-06-06.arrow"))
                .unwrap()
                .is_empty()
        );
        run(FromStage::Stage1, FromStage::Stage1).unwrap();
        let segments = work.join("segments");
        crate::cli_validate::validate_segments(
            &segments,
            std::slice::from_ref(&day),
            ClassFilterArg::Ga,
            Feed::Adsblol,
            &root,
        )
        .unwrap();
        validate_ga_merge(std::slice::from_ref(&segments), Some(&root)).unwrap();
        assert!(validate_ga_merge(std::slice::from_ref(&segments), None).is_err());
        assert!(
            aircraft_extract::arrow_io::read_segments(&segments.join("2026-06-06.arrow"))
                .unwrap()
                .is_empty()
        );
        let other = temp.path().join("other-export");
        catalog(&other, "prod", &["2026-06-06"], &[]);
        assert!(validate_ga_merge(std::slice::from_ref(&segments), Some(&other)).is_err());
        let unreceipted = temp.path().join("unreceipted");
        std::fs::create_dir_all(unreceipted.join("segments")).unwrap();
        std::fs::copy(
            segments.join("2026-06-06.arrow"),
            unreceipted.join("segments/2026-06-06.arrow"),
        )
        .unwrap();
        assert!(
            crate::cli_validate::validate_segments(
                &unreceipted.join("segments"),
                std::slice::from_ref(&day),
                ClassFilterArg::Ga,
                Feed::Adsblol,
                &root
            )
            .is_err()
        );
        std::fs::write(work.join("flights/2026-06-06.arrow"), b"changed parent").unwrap();
        assert!(run(FromStage::Stage1, FromStage::Stage1).is_err());
        assert!(validate_ga_merge(&[segments], Some(&root)).is_err());
    }

    #[test]
    fn fresh_days_append_in_one_work_root_without_changing_completed_days() {
        use std::os::unix::fs::MetadataExt;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("source");
        let work = temp.path().join("work");
        let days = ["2026-06-06", "2026-06-07"];
        catalog(&root, "staging", &days, &[]);
        let run = |work: &Path, day: &str, from, until, filter| {
            crate::cli_run_all::run_all(
                root.clone(),
                temp.path().join("prepared-year"),
                temp.path().join("prepared"),
                work.into(),
                vec![day.into()],
                None,
                from,
                until,
                Feed::Adsblol,
                filter,
                None,
                None,
                false,
            )
        };
        let append = |work: &Path, day: &str, until| {
            run(work, day, FromStage::Stage0, until, ClassFilterArg::Ga)
        };
        let first_identity = || {
            ["flights", "segments"].map(|stage| {
                let path = work.join(stage).join(format!("{}.arrow", days[0]));
                let stat = std::fs::metadata(&path).unwrap();
                (
                    path.clone(),
                    std::fs::read(path).unwrap(),
                    stat.dev(),
                    stat.ino(),
                    stat.size(),
                    stat.mtime(),
                    stat.mtime_nsec(),
                    stat.ctime(),
                    stat.ctime_nsec(),
                )
            })
        };
        let first_receipts = || {
            let result = Command::new("python3")
                .args(["-c", "import sqlite3,sys; db=sqlite3.connect('file:'+sys.argv[1]+'?mode=ro',uri=True); print([(t,db.execute('SELECT * FROM '+t+' WHERE day=? ORDER BY 1,2',(sys.argv[2],)).fetchall()) for t in ('sources','pending','artifacts')])"])
                .arg(work.join("source-receipts.sqlite")).arg(days[0]).output().unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            result.stdout
        };
        append(&work, days[0], FromStage::Stage1).unwrap();
        let original = first_identity();
        let receipts = first_receipts();
        append(&work, days[1], FromStage::Stage0).unwrap();
        assert_eq!(first_identity(), original);
        assert_eq!(first_receipts(), receipts);
        run(
            &work,
            days[1],
            FromStage::Stage1,
            FromStage::Stage1,
            ClassFilterArg::Ga,
        )
        .unwrap();
        validate_ga_merge(&[work.join("segments")], Some(&root)).unwrap();
        assert_eq!(first_identity(), original);
        assert_eq!(first_receipts(), receipts);
        assert!(
            append(&work, days[0], FromStage::Stage0)
                .unwrap_err()
                .to_string()
                .contains("already exists")
        );
        let next = "2026-06-08";
        assert!(
            append(&work, next, FromStage::Shuffle)
                .unwrap_err()
                .to_string()
                .contains("cannot append")
        );
        assert!(
            run(
                &work,
                next,
                FromStage::Stage0,
                FromStage::Stage0,
                ClassFilterArg::NonGa
            )
            .is_err()
        );
        let legacy = temp.path().join("legacy");
        std::fs::create_dir_all(legacy.join("flights")).unwrap();
        std::fs::copy(
            &original[0].0,
            legacy.join("flights").join(format!("{}.arrow", days[0])),
        )
        .unwrap();
        assert!(append(&legacy, days[1], FromStage::Stage0).is_err());
        let collision = temp.path().join("segment-collision");
        aircraft_extract::arrow_io::write_segments(
            &collision
                .join("segments")
                .join(format!("{}.arrow", days[1])),
            &[],
        )
        .unwrap();
        assert!(
            append(&collision, days[1], FromStage::Stage0)
                .unwrap_err()
                .to_string()
                .contains("already exists")
        );
        std::fs::create_dir_all(work.join("segments_by_square")).unwrap();
        std::fs::write(work.join("segments_by_square/days"), days.join("\n")).unwrap();
        assert!(
            append(&work, next, FromStage::Stage0)
                .unwrap_err()
                .to_string()
                .contains("cannot append")
        );
        assert!(!work.join("flights").join(format!("{next}.arrow")).exists());
        assert_eq!(first_identity(), original);
        assert_eq!(first_receipts(), receipts);
    }

    #[test]
    fn catalog_mlat_omission_flows_through_native_days_and_sampling_weights() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("source");
        let work = temp.path().join("work");
        let days = ["2026-06-06", "2026-06-07", "2026-06-08"];
        catalog(&root, "staging", &days, &[days[1]]);
        crate::cli_run_all::run_all(
            root.clone(),
            temp.path().join("prepared-year"),
            temp.path().join("prepared"),
            work.clone(),
            days.map(str::to_owned).to_vec(),
            None,
            FromStage::Stage0,
            FromStage::Stage1,
            Feed::Adsblol,
            ClassFilterArg::Ga,
            None,
            None,
            false,
        )
        .unwrap();
        assert!(
            !work
                .join("flights")
                .join(format!("{}.arrow", days[1]))
                .exists()
        );
        let segments = work.join("segments");
        crate::cli_validate::validate_segments(
            &segments,
            &days.map(str::to_owned),
            ClassFilterArg::Ga,
            Feed::Adsblol,
            &root,
        )
        .unwrap();
        validate_ga_merge(std::slice::from_ref(&segments), Some(&root)).unwrap();
        let ga = crate::cli_validate::list_segments_day_paths(&segments).unwrap();
        assert_eq!(ga.len(), 2);
        let airline = temp.path().join("airline");
        let mut primary = Vec::new();
        for month in 1..=12 {
            let path = airline.join(format!("2025-{month:02}-01.arrow"));
            aircraft_extract::arrow_io::write_segments(&path, &[]).unwrap();
            primary.push(path);
        }
        let shuffled = temp.path().join("shuffled");
        aircraft_extract::shuffle::shuffle_per_square(&primary, &ga, &shuffled, None).unwrap();
        let n_days = crate::cli_validate::read_window_n_days(&shuffled).unwrap();
        let ga_n_days = crate::cli_validate::read_ga_n_days(&shuffled).unwrap();
        assert_eq!((n_days, ga_n_days), (12, 2));
        let output = temp.path().join("airborne.arrow");
        aircraft_extract::arrow_io::write_airborne(&output, &[], n_days, ga_n_days).unwrap();
        let (schema, _) = aircraft_extract::arrow_io::read_record_batches(&output).unwrap();
        use noise_compute::emission::aircraft::{ClassWeights, SAMPLE_DAYS_BY_CLASS_KEY};
        let weights = ClassWeights::parse(
            schema
                .metadata()
                .get(SAMPLE_DAYS_BY_CLASS_KEY)
                .map(String::as_str),
            n_days,
        )
        .unwrap();
        assert_eq!(weights.ga_n_days(), 2);
        assert_eq!(schema.metadata()["n_days"], "12");
        assert_eq!(schema.metadata()["ga_n_days"], "2");
    }

    #[test]
    fn corrupt_preferred_day_retries_from_verified_original_without_changing_completed_work() {
        use std::os::unix::fs::MetadataExt;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("source");
        let work = temp.path().join("work");
        let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let output = Command::new("python3")
            .current_dir(&project)
            .args(["-B", "-c", "import runpy,sys; sys.path.insert(0,'scripts'); runpy.run_path('scripts/test_download_adsblol.py')['create_recovery_catalog'](sys.argv[1])"])
            .arg(&root).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let control = "2026-04-16";
        let failed = "2026-04-17";
        let run = |day: &str| {
            crate::cli_run_all::run_all(
                root.clone(),
                temp.path().join("prepared-year"),
                temp.path().join("prepared"),
                work.clone(),
                vec![day.to_owned()],
                None,
                FromStage::Stage0,
                FromStage::Stage0,
                Feed::Adsblol,
                ClassFilterArg::Ga,
                None,
                None,
                false,
            )
        };
        run(control).unwrap();
        let original = || {
            let path = work.join("flights").join(format!("{control}.arrow"));
            let metadata = std::fs::metadata(&path).unwrap();
            let receipts = Command::new("python3")
                .args(["-B", "-c", "import sqlite3,sys; db=sqlite3.connect('file:'+sys.argv[1]+'?mode=ro',uri=True); print([(t,db.execute('SELECT * FROM '+t+' WHERE day=? ORDER BY 1,2',(sys.argv[2],)).fetchall()) for t in ('sources','pending','artifacts')])"])
                .arg(work.join("source-receipts.sqlite")).arg(control).output().unwrap();
            assert!(receipts.status.success());
            (
                path.clone(),
                std::fs::read(path).unwrap(),
                metadata.dev(),
                metadata.ino(),
                metadata.mtime(),
                metadata.mtime_nsec(),
                metadata.ctime(),
                metadata.ctime_nsec(),
                receipts.stdout,
            )
        };
        let before = original();
        assert!(
            run(failed)
                .unwrap_err()
                .to_string()
                .contains("incomplete extraction")
        );
        let failed_path = work.join("flights").join(format!("{failed}.arrow"));
        assert!(!failed_path.exists());
        let cache = SourceCache::new(&root, &work, ClassFilterArg::Ga);
        assert!(
            cache
                .validate(Some(&[failed.into()]), Some("flights"))
                .is_err()
        );
        let output = Command::new("python3")
            .arg(project.join("scripts/download-adsblol.py"))
            .args(["recover", "--source-root"])
            .arg(&root)
            .args(["--days", failed, "--reserve-bytes", "0"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("GA source recovery:"));
        run(failed).unwrap();
        cache
            .validate(Some(&[control.into(), failed.into()]), Some("flights"))
            .unwrap();
        let selected = cache.validate(Some(&[failed.into()]), None).unwrap();
        assert!(
            selected[failed]
                .iter()
                .all(|path| path.to_string_lossy().contains("-prod-"))
        );
        assert_eq!(
            aircraft_extract::stage_1::read_flights(&failed_path)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(original(), before);
    }

    #[test]
    fn selected_catalog_paths_cannot_silently_include_another_native_tar_export() {
        use aircraft_extract::source::FlightSource;
        let temp = tempfile::tempdir().unwrap();
        catalog(temp.path(), "staging", &["2026-06-06"], &[]);
        let cache = SourceCache::new(temp.path(), temp.path(), ClassFilterArg::Ga);
        let selected = cache.validate(Some(&["2026-06-06".into()]), None).unwrap();
        let parent = selected["2026-06-06"][0].parent().unwrap();
        std::fs::write(parent.join("unselected.tar"), [0; 1024]).unwrap();
        let source = aircraft_extract::source_adsb_tar::AdsbTarSource::new("")
            .with_selected_archives(selected);
        assert!(
            source
                .read_day("2026-06-06")
                .err()
                .expect("unselected archive must fail")
                .to_string()
                .contains("native archive set differs")
        );
    }
}

//! Catalog-bound primary days run through Stage 0/1, admission and shuffle; completed work survives failures.

use super::*;
use crate::cli_run_all::{run_all, RunAllRequest};
use crate::FromStage;

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

fn request(root: &Path, temp: &Path, work: &Path, days: &[&str], from: FromStage, until: FromStage) -> RunAllRequest {
    RunAllRequest {
        primary_cache: root.into(),
        secondary_cache: None,
        prepared_year_dir: temp.join("prepared-year"),
        prepared_dir: temp.join("prepared"),
        work_dir: work.into(),
        reused_segments_dirs: Vec::new(),
        days: days.iter().map(|d| d.to_string()).collect(),
        increment_days: Vec::new(),
        scope_bbox: None,
        from_stage: from,
        until_stage: until,
        cruise_phase: aircraft_extract::stage_2b::CruisePhase::All,
        cruise_spill_disk_budget_bytes: None,
    }
}

/// An MLAT-only catalog day has no complete export: it stays missing in the
/// admission and in the sampling window, never an observed-empty day.
#[test]
fn catalog_days_reach_the_shuffle_window_and_mlat_only_days_stay_missing() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    let work = temp.path().join("work");
    let days = ["2026-06-06", "2026-06-07", "2026-06-08"];
    catalog(&root, "staging", &days, &[days[1]]);
    run_all(request(&root, temp.path(), &work, &days, FromStage::Stage0, FromStage::Stage1)).unwrap();
    assert!(!work.join("flights").join(format!("{}.arrow", days[1])).exists());
    let cache = SourceCache::new(&root, &work);
    cache
        .validate(Some(&[days[0].into(), days[2].into()]), Some("segments"))
        .unwrap();
    run_all(request(&root, temp.path(), &work, &days, FromStage::Shuffle, FromStage::Shuffle)).unwrap();
    let shuffled = work.join("segments_by_square");
    let window = aircraft_extract::shuffle::completion::sampling_window(&shuffled).unwrap();
    assert_eq!((window.baseline_days, window.increment_days), (2, 0));
    let admission: serde_json::Value =
        serde_json::from_slice(&std::fs::read(shuffled.join("admission.json")).unwrap()).unwrap();
    assert_eq!(admission["primary"][days[1]]["status"], "missing");
    validate_shuffled_sources(&shuffled, &root).unwrap();
    let other = temp.path().join("other-export");
    catalog(&other, "prod", &days, &[days[1]]);
    assert!(validate_shuffled_sources(&shuffled, &other).is_err());
}

#[test]
fn fresh_days_append_in_one_work_root_without_changing_completed_days() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    let work = temp.path().join("work");
    let days = ["2026-06-06", "2026-06-07"];
    catalog(&root, "staging", &days, &[]);
    let append = |day: &str, until| {
        run_all(request(&root, temp.path(), &work, &[day], FromStage::Stage0, until))
    };
    append(days[0], FromStage::Stage1).unwrap();
    let first = work.join("segments").join(format!("{}.arrow", days[0]));
    let original = std::fs::read(&first).unwrap();
    append(days[1], FromStage::Stage1).unwrap();
    assert_eq!(std::fs::read(&first).unwrap(), original);
    assert!(append(days[0], FromStage::Stage0)
        .unwrap_err()
        .to_string()
        .contains("already exists"));
    assert!(append("2026-06-08", FromStage::Shuffle)
        .unwrap_err()
        .to_string()
        .contains("cannot append"));
}

#[test]
fn corrupt_preferred_day_retries_from_verified_original_without_changing_completed_work() {
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
        run_all(request(&root, temp.path(), &work, &[day], FromStage::Stage0, FromStage::Stage0))
    };
    run(control).unwrap();
    let control_path = work.join("flights").join(format!("{control}.arrow"));
    let before = std::fs::read(&control_path).unwrap();
    assert!(run(failed)
        .unwrap_err()
        .to_string()
        .contains("incomplete extraction"));
    let failed_path = work.join("flights").join(format!("{failed}.arrow"));
    assert!(!failed_path.exists());
    let cache = SourceCache::new(&root, &work);
    assert!(cache
        .validate(Some(&[failed.into()]), Some("flights"))
        .is_err());
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
    assert!(String::from_utf8_lossy(&output.stderr).contains("adsb.lol source recovery:"));
    run(failed).unwrap();
    cache
        .validate(Some(&[control.into(), failed.into()]), Some("flights"))
        .unwrap();
    assert_eq!(
        aircraft_extract::stage_1::read_flights(&failed_path)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(std::fs::read(&control_path).unwrap(), before);
}

#[test]
fn selected_catalog_paths_cannot_silently_include_another_native_tar_export() {
    use aircraft_extract::source::FlightSource;
    let temp = tempfile::tempdir().unwrap();
    catalog(temp.path(), "staging", &["2026-06-06"], &[]);
    let cache = SourceCache::new(temp.path(), temp.path());
    let selected = cache.validate(Some(&["2026-06-06".into()]), None).unwrap();
    let parent = selected["2026-06-06"][0].parent().unwrap();
    std::fs::write(parent.join("unselected.tar"), [0; 1024]).unwrap();
    let source =
        aircraft_extract::source_adsb_tar::AdsbTarSource::new("").with_selected_archives(selected);
    assert!(source
        .read_provider_day("2026-06-06")
        .err()
        .expect("unselected archive must fail")
        .to_string()
        .contains("native archive set differs"));
}

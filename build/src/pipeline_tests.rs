//! Resume safety against changed inputs, interrupted work and cross-boundary spatial context.
use crate::{dev4::Square, pipeline_plan::context, pipeline_storage::Run};
use serde_json::json;
use std::{fs, path::Path};

fn fixture(root: &Path) {
    fs::create_dir_all(root.join("sources/2000")).unwrap();
    fs::write(root.join("sources/2000/raw"), b"publisher data").unwrap();
    fs::write(
        root.join("year.json"),
        serde_json::to_vec(&json!({
            "year":"2000", "built":false, "as_of":"2001-01-01",
            "flows":{"start":"2000-01-01","end":"2001-01-01"},
            "inputs":[], "unavailable":[]
        }))
        .unwrap(),
    )
    .unwrap();
}

fn open(root: &Path, selection: &str, options: serde_json::Value) -> anyhow::Result<Run> {
    Run::open(
        &root.join("year.json"),
        &root.join("sources"),
        &root.join("year"),
        &root.join("scratch"),
        selection,
        &options,
    )
}

fn reopen(root: &Path, selection: &str, options: serde_json::Value) -> anyhow::Result<Run> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let result = open(root, selection, options.clone());
        let inherited = result.as_ref().err().is_some_and(|error| {
            matches!(
                error.downcast_ref::<std::fs::TryLockError>(),
                Some(std::fs::TryLockError::WouldBlock)
            )
        });
        if !inherited || std::time::Instant::now() >= deadline {
            return result;
        }
        // Parallel tests may have forked with this descriptor just before its close;
        // their unrelated children release it at exec, independently of our child's wait.
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn year_runner_rejects_stage_options_it_does_not_apply() {
    let options = crate::cli::Arguments::parse(&["--pieces".into(), "4".into()]).unwrap();
    assert!(
        crate::pipeline::build(&options)
            .unwrap_err()
            .to_string()
            .contains("--pieces is not supported")
    );
}

#[test]
fn staged_producer_outputs_fail_before_starting_expensive_preparation() {
    let root = tempfile::tempdir().unwrap();
    let manifest = crate::year::Manifest::read(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../years/2026.json"),
    )
    .unwrap();
    fs::create_dir(root.path().join("2026")).unwrap();
    fs::write(root.path().join("2026/bus"), b"previous derived output").unwrap();
    assert!(crate::pipeline_plan::check_generated_aliases(&manifest, root.path()).is_err());
    fs::remove_file(root.path().join("2026/bus")).unwrap();
    let alias = root.path().join("2026/osm-geography");
    fs::write(
        crate::osm::year_sibling_path(&alias, "train-routes").unwrap(),
        b"staged side file",
    )
    .unwrap();
    assert!(crate::pipeline_plan::check_generated_aliases(&manifest, root.path()).is_err());
}

#[test]
fn an_orphaned_preparer_retains_the_run_lock_until_it_finishes() {
    use std::{io::Write, process::Stdio};
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    fixture(root);
    let mut run = open(root, "0:0", json!({})).unwrap();
    run.set_executable_for_test(Path::new("/bin/sh"));
    let mut child = run
        .child_command()
        .args(["-c", "read finished"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    drop(run);
    assert!(open(root, "0:0", json!({})).is_err());
    child.stdin.take().unwrap().write_all(b"done\n").unwrap();
    assert!(child.wait().unwrap().success());
    reopen(root, "0:0", json!({})).unwrap();
}

#[test]
fn an_invalid_workspace_creates_nothing_inside_publisher_inputs() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    fixture(root);
    let scratch = root.join("sources/2000/mistaken-scratch/nested");
    assert!(
        Run::open(
            &root.join("year.json"),
            &root.join("sources"),
            &root.join("year"),
            &scratch,
            "0:0",
            &json!({}),
        )
        .is_err()
    );
    assert!(!root.join("sources/2000/mistaken-scratch").exists());
}

#[test]
fn resume_refuses_changed_inputs_options_outputs_and_another_writer() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    fixture(root);
    let mut run = open(root, "255:170", json!({"boxes":"51,-1,52,0"})).unwrap();
    assert!(open(root, "255:170", json!({"boxes":"51,-1,52,0"})).is_err());
    // A successful child must leave an inventory; a crashed child gets no completion marker.
    run.set_executable_for_test(Path::new("/bin/false"));
    let output = run.out.join("weather");
    fs::write(&output, b"weather").unwrap();
    assert!(run.execute("weather", &output, &[], None).is_err());
    assert!(!run.stage_root("weather").join("complete.json").exists());
    run.set_executable_for_test(Path::new("/bin/true"));
    run.execute("weather", &output, &[], None).unwrap();
    run.set_executable_for_test(Path::new("/bin/false"));
    run.execute("weather", &output, &[], None).unwrap();
    fs::remove_file(output).unwrap();
    assert!(
        run.execute("weather", &run.out.join("weather"), &[], None)
            .is_err()
    );
    drop(run);
    assert!(reopen(root, "255:170", json!({"boxes":"50,-1,52,0"})).is_err());
    assert!(reopen(root, "256:170", json!({"boxes":"51,-1,52,0"})).is_err());
    let run = reopen(root, "255:170", json!({"boxes":"51,-1,52,0"})).unwrap();
    drop(run);
    fs::write(root.join("sources/2000/raw"), b"different data").unwrap();
    assert!(reopen(root, "255:170", json!({"boxes":"51,-1,52,0"})).is_err());
}

#[test]
fn spatial_context_wraps_longitude_and_stops_at_the_pole() {
    let stages = crate::pipeline_plan::steps();
    assert_eq!(
        stages.iter().find(|s| s.name == "terrain").unwrap().radius,
        Some(2)
    );
    assert_eq!(
        stages
            .iter()
            .find(|s| s.name == "prepare-rasters")
            .unwrap()
            .radius,
        Some(3)
    );
    let at_pole = context(&[Square { x: 0, y: 0 }], 3);
    assert_eq!(at_pole.len(), 28);
    assert!(at_pole.contains(&Square { x: 509, y: 3 }));
    assert!(!at_pole.contains(&Square { x: 0, y: 511 }));
    let middle = context(&[Square { x: 276, y: 173 }], 3);
    assert_eq!(middle.len(), 49);
    for building in context(&[Square { x: 276, y: 173 }], 1) {
        for facility in building.with_neighbours() {
            for raw in facility.with_neighbours() {
                assert!(middle.contains(&raw));
            }
        }
    }
}

#[test]
fn completed_native_steps_ignore_other_stages_partial_files_and_pin_the_manifest() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    fixture(root);
    let original_manifest = fs::read(root.join("year.json")).unwrap();
    let mut run = open(root, "0:0", json!({})).unwrap();
    let terrain = run.out.join("0/0/terrain");
    fs::create_dir_all(terrain.parent().unwrap()).unwrap();
    fs::write(&terrain, b"unfinished terrain").unwrap();
    let obstacles = run.out.join("0/0/obstacles");
    fs::write(&obstacles, b"screening").unwrap();
    run.set_executable_for_test(Path::new("/bin/true"));
    run.execute("obstacles", &run.out, &[], None).unwrap();
    fs::write(terrain, b"completed terrain from a retry").unwrap();
    run.set_executable_for_test(Path::new("/bin/false"));
    run.execute("obstacles", &run.out, &[], None).unwrap();
    fs::write(root.join("year.json"), b"changed while building").unwrap();
    assert_eq!(fs::read(&run.manifest).unwrap(), original_manifest);
    fs::remove_file(obstacles).unwrap();
    assert!(run.execute("obstacles", &run.out, &[], None).is_err());
}

#[test]
fn diagnostic_scope_cannot_be_opened_as_a_complete_world() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    fixture(root);
    let run = open(root, "255:170", json!({})).unwrap();
    let square = Square { x: 255, y: 170 };
    crate::pipeline::record_partial(&run, &[square], Some("51.3,-0.6,51.6,-0.2")).unwrap();
    assert!(tiles::check_year_complete(&run.out).is_err());
    let scope: serde_json::Value =
        serde_json::from_slice(&fs::read(run.out.join("partial.json")).unwrap()).unwrap();
    assert_eq!(scope["squares"], json!([[255, 170]]));
    assert_eq!(scope["aircraft_boxes"], "51.3,-0.6,51.6,-0.2");
}

#[test]
fn aircraft_progress_survives_a_failed_stage_retry() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    fixture(root);
    let mut run = open(root, "255:170", json!({})).unwrap();
    let progress = run.stage_root("aircraft-events").join("progress");
    fs::create_dir_all(&progress).unwrap();
    fs::write(progress.join("finished-square"), b"").unwrap();
    run.set_executable_for_test(Path::new("/bin/false"));
    assert!(run.execute("aircraft-events", &run.out, &[], None).is_err());
    assert!(progress.join("finished-square").exists());
    let raster = run
        .stage_root("prepare-rasters")
        .join("output/z9/255/170/dem.u16le");
    fs::create_dir_all(raster.parent().unwrap()).unwrap();
    fs::write(&raster, b"atomic raster").unwrap();
    let log = run.stage_root("prepare-rasters").join("build.log");
    fs::write(&log, b"original failure\n").unwrap();
    assert!(
        run.execute("prepare-rasters", raster.parent().unwrap(), &[], None)
            .is_err()
    );
    assert_eq!(fs::read(raster).unwrap(), b"atomic raster");
    assert_eq!(fs::read(log).unwrap(), b"original failure\n");
}

#[test]
fn inputs_changed_during_a_stage_cannot_get_a_completion_marker() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    fixture(root);
    let mut run = open(root, "0:0", json!({})).unwrap();
    run.set_executable_for_test(Path::new("/bin/true"));
    fs::write(root.join("sources/2000/raw"), b"changed publisher data").unwrap();
    assert!(run.execute("weather", &run.out, &[], None).is_err());
    assert!(!run.stage_root("weather").join("complete.json").exists());
}

#[test]
fn runtime_library_changes_cannot_reuse_completion_markers() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    fixture(root);
    let libraries = root.join("libraries");
    fs::create_dir_all(&libraries).unwrap();
    fs::write(libraries.join("libgdal.so"), b"runtime a").unwrap();
    let alias = root.join("library-alias");
    std::os::unix::fs::symlink(&libraries, &alias).unwrap();
    let options = json!({"road-library-path":alias});
    let run = open(root, "0:0", options.clone()).unwrap();
    run.verify_inputs().unwrap();
    let metadata = fs::metadata(libraries.join("libgdal.so")).unwrap();
    fs::write(libraries.join("libgdal.so"), b"runtime b").unwrap();
    fs::File::options()
        .write(true)
        .open(libraries.join("libgdal.so"))
        .unwrap()
        .set_modified(metadata.modified().unwrap())
        .unwrap();
    assert!(run.verify_inputs().is_err());
    drop(run);
    assert!(reopen(root, "0:0", options).is_err());
}

#[test]
fn finished_terrain_can_feed_an_unfinished_year_but_missing_context_cannot() {
    let temporary = tempfile::tempdir().unwrap();
    fixture(temporary.path());
    let inputs = crate::year::YearInputs::read(
        &temporary.path().join("year.json"),
        &temporary.path().join("sources"),
    )
    .unwrap();
    let square = Square { x: 255, y: 170 };
    let terrain = temporary.path().join("terrain");
    let surrounding = context(&[square], 1);
    crate::terrain_completion::write(&inputs, &terrain, &surrounding).unwrap();
    assert!(crate::terrain_completion::check(&inputs, &terrain, &surrounding).is_ok());
    assert!(!terrain.join("complete").exists());
    assert!(crate::terrain_completion::check(&inputs, &terrain, &context(&[square], 2)).is_err());
    let data = tiles::square::square_path(&terrain, tiles::Kind::Terrain, square);
    fs::create_dir_all(data.parent().unwrap()).unwrap();
    fs::write(&data, b"height nodes").unwrap();
    crate::terrain_completion::write(&inputs, &terrain, &surrounding).unwrap();
    fs::remove_file(data).unwrap();
    assert!(crate::terrain_completion::check(&inputs, &terrain, &surrounding).is_err());
}

#[test]
fn a_missing_declared_wall_file_fails_but_explicit_absence_keeps_mapped_walls() {
    let temporary = tempfile::tempdir().unwrap();
    fixture(temporary.path());
    let path = temporary.path().join("year.json");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let inputs =
        || crate::year::YearInputs::read(&path, &temporary.path().join("sources")).unwrap();
    assert!(crate::barriers::Barriers::from_year(&inputs()).is_err());
    value["unavailable"] = json!([{"id":"national-walls","kind":"barriers","region":"world",
        "description":"No earlier national inventory exists.","depends_on":[]}]);
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(crate::barriers::Barriers::from_year(&inputs()).is_ok());
    value["unavailable"] = json!([]);
    value["inputs"] = json!([{"id":"national-walls","kind":"barriers","region":"world",
        "publisher":"Authority", "url":"https://example.org/walls", "observed":{"start":"2000","end":"2000"},
        "published":null, "licence":"CC0", "public_ok":true, "depends_on":[], "description":"National screens."}]);
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(crate::barriers::Barriers::from_year(&inputs()).is_err());
}

#[test]
fn world_preflight_needs_an_admitted_vintage_or_declared_absence_for_each_kind() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    let future = json!({"id":"future-feed", "kind":"prepare-timetable", "region":"world",
        "publisher":"Operator", "url":"https://example.org/feed", "observed":{"start":"2099","end":"2099"},
        "published":null, "licence":"CC0", "public_ok":true, "depends_on":[], "description":"Future edition."});
    let path = root.path().join("year.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["inputs"] = json!([future]);
    fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let read = || crate::year::YearInputs::read(&path, &root.path().join("sources")).unwrap();
    assert!(crate::pipeline_plan::check_publisher_inputs(&read()).is_err());
    let mut earlier = manifest["inputs"][0].clone();
    earlier["id"] = json!("earlier-feed");
    earlier["observed"] = json!({"start":"1999", "end":"1999"});
    manifest["inputs"].as_array_mut().unwrap().push(earlier);
    fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let admitted = root.path().join("sources/2000/earlier-feed");
    fs::write(&admitted, "retained timetable").unwrap();
    crate::pipeline_plan::check_publisher_inputs(&read()).unwrap();
    fs::remove_file(&admitted).unwrap();
    assert!(crate::pipeline_plan::check_publisher_inputs(&read()).is_err());
}

#[test]
fn a_table_the_run_fits_is_neither_pinned_from_an_earlier_fit_nor_required_before_the_run() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    fixture(root);
    let tables = root.join("tables/2000");
    fs::create_dir_all(&tables).unwrap();
    fs::write(
        tables.join("tagged-speeds.json"),
        br#"{"rows":[{"fixture":1}]}"#,
    )
    .unwrap();
    fs::write(tables.join("hotel-trip-rate.json"), br#"{"rows":[2.87]}"#).unwrap();
    fs::write(
        tables.join("dwelling-trips.json"),
        br#"{"rows":[{"country":"AT","value":3.4}]}"#,
    )
    .unwrap();
    let produced = crate::pipeline_plan::produced_kinds();
    assert!(produced.contains(&crate::year::Kind::TaggedSpeeds));
    assert!(produced.contains(&crate::year::Kind::HotelTripRate));
    let run = open(root, "0:0", json!({})).unwrap();
    let pinned = run.root.join("tables/2000");
    assert!(pinned.join("dwelling-trips.json").is_file());
    assert!(!pinned.join("tagged-speeds.json").exists());
    assert!(!pinned.join("hotel-trip-rate.json").exists());
    let inputs =
        crate::year::YearInputs::read(&root.join("year.json"), &root.join("sources")).unwrap();
    let every: std::collections::BTreeSet<_> =
        crate::source_tables::MODEL_KINDS.iter().copied().collect();
    crate::source_tables::check_tables(&inputs, &every).unwrap();
    let mut all_but_speeds = every.clone();
    all_but_speeds.remove(&crate::year::Kind::TaggedSpeeds);
    let missing = crate::source_tables::check_tables(&inputs, &all_but_speeds).unwrap_err();
    assert!(missing.contains("tagged-speeds"), "{missing}");
}

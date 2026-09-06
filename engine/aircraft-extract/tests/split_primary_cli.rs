//! Multiple validated primary roots reach shuffle and cruise without copying completed input.

use aircraft_extract::arrow_io::{read_record_batches, write_segments};
use aircraft_extract::flight::{FlightSegment, Phase, source_id};
use aircraft_extract::spatial::square_directories;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn segments(day: &str, id: u64) -> Vec<FlightSegment> {
    [Phase::Airborne, Phase::Cruise]
        .into_iter()
        .map(|phase| FlightSegment {
            flight_id: id,
            callsign: format!("FL{id}"),
            aircraft_type: *b"B738",
            profile_idx: aircraft_extract::profile::profile_idx("B738"),
            source_id: source_id::ADSB_EXCHANGE,
            origin: 0,
            veh_kind: 0,
            gse_class: 0,
            period: 0,
            date_id: aircraft_extract::period::parse_date_id(day).unwrap(),
            phase,
            flags: 0,
            start_lat: 50.10,
            start_lon: 14.20,
            start_alt_m: 11_000.0,
            end_lat: 50.10,
            end_lon: 14.205,
            end_alt_m: 11_000.0,
            speed_kt: 460.0,
            length_m: aircraft_extract::geo::flat_dist(50.10, 14.20, 50.10, 14.205),
            agl_avg_m: 11_000.0,
            start_elev_m: 0.0,
            end_elev_m: 0.0,
        })
        .collect()
}

fn identity(path: &Path) -> (Vec<u8>, u64, u64, i64, i64, i64, i64) {
    let stat = path.metadata().unwrap();
    (
        std::fs::read(path).unwrap(),
        stat.dev(),
        stat.ino(),
        stat.mtime(),
        stat.mtime_nsec(),
        stat.ctime(),
        stat.ctime_nsec(),
    )
}

#[test]
fn split_primary_roots_match_single_root_through_shuffle_and_cruise_and_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let days = ["2025-01-01", "2025-02-01"];
    let combined = root.join("combined/segments");
    let split = [root.join("first/segments"), root.join("second/segments")];
    let mut originals = Vec::new();
    for (index, day) in days.iter().enumerate() {
        for dir in [&split[index], &combined] {
            let path = dir.join(format!("{day}.arrow"));
            write_segments(&path, &segments(day, index as u64 + 1)).unwrap();
            originals.push((path.clone(), identity(&path)));
        }
    }
    let run = |work: &Path,
               prepared: &Path,
               dirs: &[PathBuf],
               selected: &str,
               from: &str,
               until: &str| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_aircraft-extract"));
        command
            .args([
                "--max-threads",
                "1",
                "run-all",
                "--feed",
                "adsbexchange",
                "--class-filter",
                "non-ga",
                "--days",
                selected,
                "--from-stage",
                from,
                "--until-stage",
                until,
                "--scope-bbox",
                "50,14,50.2,14.5",
                "--fail-on-ga-cruise",
            ])
            .arg("--adsb-cache")
            .arg(root.join("unused-cache"))
            .arg("--prepared-dir")
            .arg(root.join("unused-rasters"))
            .arg("--prepared-year-dir")
            .arg(prepared)
            .arg("--work-dir")
            .arg(work);
        for dir in dirs {
            command.arg("--segments-dir").arg(dir);
        }
        command.output().unwrap()
    };
    let selected = days.join(",");
    let combined_work = root.join("combined");
    let split_work = root.join("split-output");
    let reference = root.join("reference/2026");
    let actual = root.join("actual/2026");
    for (work, prepared, dirs) in [
        (&combined_work, &reference, Vec::new()),
        (
            &split_work,
            &actual,
            vec![split[1].clone(), split[0].clone()],
        ),
    ] {
        for stage in ["shuffle", "stage2b"] {
            let output = run(work, prepared, &dirs, &selected, stage, stage);
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert_eq!(
            std::fs::read_to_string(work.join("segments_by_square/days")).unwrap(),
            days.join("\n")
        );
        assert!(
            std::fs::read(work.join("segments_by_square/ga_days"))
                .unwrap()
                .is_empty()
        );
    }
    let expected_squares = square_directories(&reference).unwrap();
    assert!(!expected_squares.is_empty());
    assert_eq!(
        expected_squares
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>(),
        square_directories(&actual)
            .unwrap()
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>()
    );
    for (id, path) in expected_squares {
        let expected = read_record_batches(&path.join("cruise.arrow")).unwrap();
        assert_eq!(expected.0.metadata()["n_days"], "2");
        assert!(
            expected
                .1
                .iter()
                .map(|batch| batch.num_rows())
                .sum::<usize>()
                > 0
        );
        assert_eq!(
            read_record_batches(
                &actual
                    .join(aircraft_extract::geo::square_path(id))
                    .join("cruise.arrow")
            )
            .unwrap(),
            expected
        );
    }
    assert!(!split_work.join("segments").exists());
    assert!(!split_work.join("flights").exists());
    let rejected_work = root.join("rejected");
    let rejected_output = root.join("rejected-output/2026");
    for (dirs, from, expected_error) in [
        (
            vec![split[0].clone()],
            "shuffle",
            "segment day set mismatch",
        ),
        (
            vec![split[0].clone(), split[0].clone(), split[1].clone()],
            "shuffle",
            "duplicate segment day",
        ),
        (
            split.to_vec(),
            "stage0",
            "--segments-dir reuses completed inputs",
        ),
    ] {
        let output = run(
            &rejected_work,
            &rejected_output,
            &dirs,
            &selected,
            from,
            from,
        );
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected_error),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!rejected_work.exists());
        assert!(!rejected_output.exists());
    }
    let wrong = root.join("wrong-feed/segments");
    for (index, day) in days.iter().enumerate() {
        let mut rows = segments(day, index as u64 + 1);
        rows[0].source_id = source_id::ADSB_LOL_TAR;
        write_segments(&wrong.join(format!("{day}.arrow")), &rows).unwrap();
    }
    let output = run(
        &rejected_work,
        &rejected_output,
        &[wrong],
        &selected,
        "shuffle",
        "shuffle",
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("wrong date, feed, or hybrid class"));
    assert!(!rejected_work.exists());
    assert!(!rejected_output.exists());
    std::fs::write(
        split_work.join("segments_by_square/days"),
        "2025-01-01\n2025-03-01\n",
    )
    .unwrap();
    let output = run(
        &split_work,
        &rejected_output,
        &split,
        &selected,
        "stage2b",
        "stage2b",
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cruise days differ"));
    assert!(!rejected_output.exists());
    for (path, before) in originals {
        assert_eq!(identity(&path), before);
    }
}

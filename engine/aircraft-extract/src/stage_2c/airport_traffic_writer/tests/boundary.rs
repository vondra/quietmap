//! Ground ownership, cross-border normalization, and input integrity regressions.
use super::*;
use crate::arrow_io::{
    read_airport_summary, read_airport_traffic, write_segments, AirportTrafficRow,
};
use crate::stage_2c::{run_stage_2c, AIRPORT_SUMMARY_FILENAME};

fn leg(lat: f32, lon: f32) -> FlightSegment {
    FlightSegment {
        flight_id: 42,
        callsign: "TEST".into(),
        aircraft_type: *b"B738",
        profile_idx: 23,
        source_id: 0,
        origin: 0,
        veh_kind: 0,
        gse_class: 0,
        period: 0,
        date_id: 0,
        phase: Phase::Ground,
        flags: 0,
        start_lat: lat,
        start_lon: lon - 0.0001,
        start_alt_m: 0.0,
        end_lat: lat + 0.002,
        end_lon: grid::geo::normalize_longitude(f64::from(lon) + 0.0002) as f32,
        end_alt_m: 0.0,
        speed_kt: 30.0,
        length_m: crate::geo::flat_dist(lat, lon - 0.0001, lat + 0.002, lon + 0.0002),
        agl_avg_m: 0.0,
        start_elev_m: 0.0,
        end_elev_m: 0.0,
    }
}

fn partitioned_run(lat: f64, lon: f64, split: bool) -> Vec<AirportTrafficRow> {
    let temp = tempfile::tempdir().unwrap();
    let prepared = temp.path().join("2025");
    let inputs = temp.path().join("segments");
    let left = crate::spatial::square_id(lat, lon - 0.0003).unwrap();
    let right = crate::spatial::square_id(lat, lon + 0.0003).unwrap();
    assert_ne!(left, right);
    let make_line = |id, offset| FakeRealLine {
        osm_id: id,
        segment_idx: 0,
        start_lat: lat,
        end_lat: lat + 0.002,
        start_lon: grid::geo::normalize_longitude(lon + offset),
        end_lon: grid::geo::normalize_longitude(lon + offset),
        length_m: crate::geo::flat_dist(lat as f32, 0.0, (lat + 0.002) as f32, 0.0),
        aeroway_type: 1,
    };
    if split {
        for (owner, line) in [(left, make_line(1, -0.0003)), (right, make_line(2, 0.0003))] {
            write_real_airport_lines_arrow(
                &prepared
                    .join(square_path(owner))
                    .join("airport_lines.arrow"),
                &[line],
            );
        }
    } else {
        write_real_airport_lines_arrow(
            &prepared
                .join(square_path(right))
                .join("airport_lines.arrow"),
            &[make_line(1, -0.0003), make_line(2, 0.0003)],
        );
    }
    write_segments(
        &inputs.join(square_path(right)).join("ground.arrow"),
        &[leg(lat as f32, lon as f32)],
    )
    .unwrap();
    let area = AirportArea::new(
        99,
        AERODROME_AEROWAY_TYPE,
        "Test".into(),
        "TEST".into(),
        lat,
        lon,
        Vec::new(),
        1e6,
    );
    let n = run_stage_2c(&inputs, &[area], &prepared, 12, 365, None).unwrap();
    assert_eq!(n, if split { 2 } else { 1 });
    let mut rows = Vec::new();
    for (owner, dir) in crate::spatial::square_directories(&prepared).unwrap() {
        let path = dir.join("airport_traffic.arrow");
        if !path.exists() {
            continue;
        }
        for row in read_airport_traffic(&path).unwrap() {
            if split {
                assert_eq!(owner, if row.osm_id == 1 { left } else { right });
            }
            rows.push(row);
        }
    }
    rows.sort_by_key(|row| row.osm_id);
    assert_eq!(rows.len(), 2, "each line is emitted exactly once");
    for row in &rows {
        assert_eq!(row.unique_movement_count, 1);
        assert_eq!(row.microseg_unique_count, 1);
        assert!(row.band_energy_lin.iter().any(|energy| *energy > 0.0));
    }
    let summary =
        read_airport_summary(&prepared.join("aircraft").join(AIRPORT_SUMMARY_FILENAME)).unwrap();
    assert_eq!(summary.len(), 1);
    assert_eq!(summary[0].airport_unique_ops_count_per_kind[1], 1);
    rows
}

#[test]
fn neighboring_line_owners_preserve_unsplit_energy_and_rotation_unions() {
    for (lat, lon) in [(50.0, 0.0), (50.0, 180.0), (85.0, 0.0)] {
        let unsplit = partitioned_run(lat, lon, false);
        let split = partitioned_run(lat, lon, true);
        for (expected, actual) in unsplit.iter().zip(split.iter()) {
            assert_eq!(expected.osm_id, actual.osm_id);
            assert_eq!(expected.microseg_unique_count, actual.microseg_unique_count);
            for (a, b) in expected
                .band_energy_lin
                .iter()
                .zip(actual.band_energy_lin.iter())
            {
                assert!(
                    (a - b).abs() <= a.abs() * 1e-6,
                    "partition changed energy: {a} vs {b}"
                );
            }
        }
    }
}

#[test]
fn all_corrupt_ground_or_line_inputs_fail_before_prior_output_is_removed() {
    for filename in ["ground.arrow", "airport_lines.arrow", SYNTH_LINES_FILE] {
        let temp = tempfile::tempdir().unwrap();
        let inputs = temp.path().join("segments");
        let prepared = temp.path().join("2025");
        let first = crate::spatial::square_id(50.0, 0.0003).unwrap();
        let second = crate::spatial::square_id(50.0, 1.0).unwrap();
        write_segments(
            &inputs.join(square_path(first)).join("ground.arrow"),
            &[leg(50.0, 0.0)],
        )
        .unwrap();
        let prior = prepared
            .join(square_path(first))
            .join("airport_traffic.arrow");
        std::fs::create_dir_all(prior.parent().unwrap()).unwrap();
        std::fs::write(&prior, b"prior-good-output").unwrap();
        let corrupt = if filename == "ground.arrow" {
            inputs.join(square_path(second)).join(filename)
        } else {
            prepared.join(square_path(second)).join(filename)
        };
        std::fs::create_dir_all(corrupt.parent().unwrap()).unwrap();
        std::fs::write(corrupt, b"corrupt").unwrap();
        assert!(run_stage_2c(&inputs, &[], &prepared, 12, 365, None).is_err());
        assert_eq!(std::fs::read(&prior).unwrap(), b"prior-good-output");
    }
}

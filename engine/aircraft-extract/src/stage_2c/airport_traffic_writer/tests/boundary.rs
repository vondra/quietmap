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
    let index = crate::airport_index::AerodromeIndex::build(std::slice::from_ref(&area));
    let plan = plan_ground_traffic(&inputs, &prepared, None, &index).unwrap();
    assert_eq!(plan.len(), if split { 2 } else { 1 });
    let input = inputs.join(square_path(right)).join("ground.arrow");
    for work in &plan {
        assert_eq!(work.inputs, vec![input.clone()]);
        assert_eq!(work.input_rows, 1);
        assert_eq!(work.input_bytes, input.metadata().unwrap().len());
        assert_eq!(work.candidates.len(), if split { 2 } else { 1 });
        assert_eq!(work.cached_lines, 2);
        assert_eq!(work.owned_lines, if split { 1 } else { 2 });
        assert_eq!(work.maximum_counter_rows, 54 * work.owned_lines);
        assert!(work.maximum_airport_key_bytes >= "TEST".len());
    }
    assert!(!prepared.join(".airport_traffic_pending").exists());
    let n = run_stage_2c(&inputs, &[area], &prepared, 12, 365, None).unwrap();
    assert_eq!(n, if split { 2 } else { 1 });
    let mut rows = Vec::new();
    for (owner, dir) in crate::spatial::square_directories(&prepared).unwrap() {
        let path = dir.join("airport_traffic.arrow");
        if !path.exists() {
            continue;
        }
        let summary = read_airport_summary(&dir.join(AIRPORT_SUMMARY_FILENAME)).unwrap();
        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0].airport_unique_ops_count_per_kind[1], 1);
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
    assert!(!prepared.join("aircraft").exists());
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

#[test]
fn inconsistent_classes_for_one_flight_keep_all_counter_and_union_dimensions() {
    use noise_compute::emission::{
        aircraft::is_ga_sampled_class, profiles_generated::CLASS_REP_PROFILE_IDX,
    };
    let temp = tempfile::tempdir().unwrap();
    let inputs = temp.path().join("input");
    let prepared = temp.path().join("prepared");
    let owner = crate::spatial::square_id(50.0, 14.0).unwrap();
    let dir = prepared.join(square_path(owner));
    let segment = leg(50.0, 14.0);
    write_real_airport_lines_arrow(
        &dir.join("airport_lines.arrow"),
        &[FakeRealLine {
            osm_id: 1,
            segment_idx: 0,
            start_lat: segment.start_lat as f64,
            start_lon: segment.start_lon as f64,
            end_lat: segment.end_lat as f64,
            end_lon: segment.end_lon as f64,
            length_m: segment.length_m,
            aeroway_type: 0,
        }],
    );
    let ga_class = (0..CLASS_REP_PROFILE_IDX.len())
        .find(|&i| is_ga_sampled_class(i as u8))
        .unwrap();
    let mut segments = Vec::new();
    for (profile, period) in [
        (segment.profile_idx, 0),
        (CLASS_REP_PROFILE_IDX[ga_class], 1),
    ] {
        for departure in [false, true] {
            let mut row = segment.clone();
            row.profile_idx = profile;
            row.period = period;
            row.flags = if departure {
                crate::flight::segment_flags::IS_DEPARTURE
            } else {
                0
            };
            segments.push(row);
        }
    }
    for class in 0..NUM_GSE_CLASSES {
        let mut row = segment.clone();
        row.veh_kind = 1;
        row.gse_class = class as u8;
        row.period = 2;
        segments.push(row);
    }
    segments.extend(segments.clone());
    write_segments(
        &inputs.join(square_path(owner)).join("ground.arrow"),
        &segments,
    )
    .unwrap();
    let area = AirportArea::new(
        1,
        AERODROME_AEROWAY_TYPE,
        "A".into(),
        "A".into(),
        50.0,
        14.0,
        Vec::new(),
        0.0,
    );
    let index = crate::airport_index::AerodromeIndex::build(std::slice::from_ref(&area));
    let plan = plan_ground_traffic(&inputs, &prepared, None, &index).unwrap();
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].maximum_counter_rows, 99);
    run_stage_2c(&inputs, &[area], &prepared, 12, 365, None).unwrap();
    let rows = read_airport_traffic(&dir.join("airport_traffic.arrow")).unwrap();
    assert_eq!(rows.len(), 7);
    for row in rows {
        assert_eq!(row.unique_movement_count, 1);
        assert_eq!(
            (row.microseg_unique_count, row.microseg_unique_ga_count),
            (1, 1)
        );
        assert_eq!(
            (row.microseg_unique_arr_count, row.microseg_unique_dep_count),
            (1, 1)
        );
        assert_eq!(
            (
                row.microseg_unique_ga_arr_count,
                row.microseg_unique_ga_dep_count
            ),
            (1, 1)
        );
        assert_eq!(row.microseg_unique_gse_count_per_class, [1, 1, 1]);
        if row.veh_kind == 0 {
            assert_eq!(row.unique_arr_count, u32::from(row.is_departure == 0));
            assert_eq!(row.unique_dep_count, u32::from(row.is_departure == 1));
        } else {
            assert_eq!(row.unique_gse_count_per_class[row.class_idx as usize], 1);
        }
    }
    let summary = read_airport_summary(&dir.join(AIRPORT_SUMMARY_FILENAME))
        .unwrap()
        .remove(0);
    assert_eq!(
        (
            summary.airport_unique_arr_count,
            summary.airport_unique_dep_count
        ),
        (1, 1)
    );
    assert_eq!(
        (
            summary.airport_unique_ga_arr_count,
            summary.airport_unique_ga_dep_count
        ),
        (1, 1)
    );
    assert_eq!(summary.airport_unique_gse_count_per_class, [1, 1, 1]);
}

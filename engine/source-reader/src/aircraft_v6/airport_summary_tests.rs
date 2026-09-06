//! Real Stage 2C output must supply complete local airport counts across cell boundaries.

use super::*;
use aircraft_extract::{arrow_io::*, flight::*, stage_2c::run_stage_2c};
use arrow::{array::*, ipc::writer::FileWriter};
use noise_compute::{propagation::obstacle_index::ObstacleSet, types::*};
use std::{fs::File, path::Path, sync::Arc};

struct FlatGround;
impl RasterSampler for FlatGround {
    fn elevation(&self, _: f64, _: f64) -> f64 {
        0.0
    }
    fn ground_g(&self, _: f64, _: f64) -> f64 {
        1.0
    }
    fn building_enclosure(&self, _: f64, _: f64) -> f64 {
        0.0
    }
}

fn write_runway(root: &Path, lat: f64, lon: f64, id: i64) {
    let start = grid::lonlat_to_grid(lon, lat);
    let end = grid::lonlat_to_grid(lon, lat + 0.002);
    let batch = RecordBatch::try_from_iter(vec![
        ("osm_id", Arc::new(Int64Array::from(vec![id])) as ArrayRef),
        ("segment_idx", Arc::new(Int16Array::from(vec![0]))),
        ("start_gx", Arc::new(Int32Array::from(vec![start.0]))),
        ("start_gy", Arc::new(Int32Array::from(vec![start.1]))),
        ("end_gx", Arc::new(Int32Array::from(vec![end.0]))),
        ("end_gy", Arc::new(Int32Array::from(vec![end.1]))),
        (
            "length_m",
            Arc::new(Float32Array::from(vec![
                grid::geo::flat_dist(lat, lon, lat + 0.002, lon) as f32,
            ])),
        ),
        ("aeroway_type", Arc::new(UInt8Array::from(vec![0]))),
    ])
    .unwrap();
    std::fs::create_dir_all(root).unwrap();
    let mut writer = FileWriter::try_new(
        File::create(root.join("airport_lines.arrow")).unwrap(),
        &batch.schema(),
    )
    .unwrap();
    writer.write(&batch).unwrap();
    writer.finish().unwrap();
}

fn movement(lat: f64, lon: f64, flight_id: u64) -> FlightSegment {
    let start_lon = grid::geo::normalize_longitude(lon - 0.0001) as f32;
    let end_lon = grid::geo::normalize_longitude(lon + 0.0002) as f32;
    FlightSegment {
        flight_id,
        callsign: "TEST".into(),
        aircraft_type: *b"B738",
        profile_idx: noise_compute::emission::profiles_generated::profile_idx("B738"),
        source_id: 0,
        origin: 0,
        veh_kind: 0,
        gse_class: 0,
        period: 0,
        date_id: 0,
        phase: Phase::Ground,
        flags: 0,
        start_lat: lat as f32,
        start_lon,
        start_alt_m: 0.0,
        end_lat: (lat + 0.002) as f32,
        end_lon,
        end_alt_m: 0.0,
        speed_kt: 30.0,
        length_m: grid::geo::flat_dist(lat, f64::from(start_lon), lat + 0.002, f64::from(end_lon))
            as f32,
        agl_avg_m: 0.0,
        start_elev_m: 0.0,
        end_elev_m: 0.0,
    }
}

#[test]
fn stage2c_cell_summaries_preserve_popup_unions_and_refuse_incomplete_neighbors() {
    for (lat, lon) in [(50.0, 0.0), (50.0, 180.0), (85.0, 0.0)] {
        let temp = tempfile::tempdir().unwrap();
        let prepared = temp.path().join("prepared");
        let inputs = temp.path().join("inputs");
        let left = crate::query::square_dir(&prepared, grid::square_of(lat, lon - 0.0003));
        let right = crate::query::square_dir(&prepared, grid::square_of(lat, lon + 0.0003));
        assert_ne!(left, right);
        write_runway(&left, lat, grid::geo::normalize_longitude(lon - 0.0003), 1);
        write_runway(&right, lat, grid::geo::normalize_longitude(lon + 0.0003), 2);
        let mut departure = movement(lat, lon, 2);
        departure.flags = 1;
        let mut ga = movement(lat, lon, 3);
        ga.aircraft_type = *b"C172";
        ga.profile_idx = noise_compute::emission::profiles_generated::profile_idx("C172");
        let mut gse = movement(lat, lon, 4);
        gse.veh_kind = 1;
        write_segments(
            &crate::query::square_dir(&inputs, grid::square_of(lat, lon)).join("ground.arrow"),
            &[movement(lat, lon, 1), departure, ga, gse],
        )
        .unwrap();
        let airport = AirportArea::new(
            99,
            5,
            "Test".into(),
            "TEST".into(),
            lat,
            lon,
            Vec::new(),
            1e6,
        );
        assert_eq!(
            run_stage_2c(&inputs, &[airport], &prepared, 12, 365, None).unwrap(),
            2
        );
        assert!(!prepared.join("aircraft").exists());
        let summary_path = left.join("airport_summary.arrow");
        let rows = read_airport_summary(&summary_path).unwrap();
        assert_eq!(
            rows,
            read_airport_summary(&right.join("airport_summary.arrow")).unwrap()
        );
        let sources = crate::collect_sources_at_point(&prepared, lat + 0.001, lon).unwrap();
        assert_eq!(sources.airport_summary.lookup().len(), 1);
        let summary = sources.airport_summary.lookup()["TEST"];
        assert_eq!(
            (
                summary.arr_count,
                summary.dep_count,
                summary.ga_arr_count,
                summary.ga_dep_count
            ),
            (1, 1, 1, 0)
        );
        assert_eq!(summary.gse_count_per_class, [1, 0, 0]);
        assert_eq!(summary.ops_count_per_kind, [2, 0, 0]);
        assert_eq!(summary.ga_ops_count_per_kind, [1, 0, 0]);
        let receiver = Receiver::new(lat + 0.001, lon, 0.0);
        let obstacles = ObstacleSet::empty();
        let mut result = noise_compute::compute_at_point(
            &receiver,
            &[],
            &[],
            &[],
            &[],
            &obstacles,
            &FlatGround,
            &ComputeConfig::default(),
        );
        let mut traces = TraceCollector::default();
        add_v6_aircraft_to_result(
            &mut result,
            &mut traces,
            &receiver,
            &[],
            &[],
            &sources.aircraft_airport_traffic_batches,
            &sources.airport_lines_batches,
            &sources.airport_summary,
            &FlatGround,
            &obstacles,
            12,
            150,
        )
        .unwrap();
        assert_eq!(result.contributors.len(), 1);
        let Some(SourceMetadata::Aircraft(metadata)) = &result.contributors[0].metadata else {
            panic!("aircraft metadata");
        };
        let ground = metadata.ground_ops.as_ref().unwrap();
        assert!((ground.arrivals_per_day - (1.0 / 12.0 + 1.0 / 365.0)).abs() < 1e-12);
        assert!((ground.departures_per_day - 1.0 / 12.0).abs() < 1e-12);
        assert_eq!(ground.gse_per_day, [1.0 / 12.0, 0.0, 0.0]);
        assert!(result.total.lden_db.is_finite());
        assert!(!traces.segments.is_empty());
        serde_json::to_string(&result).unwrap();

        let original = std::fs::read(&summary_path).unwrap();
        for defect in ["missing", "empty", "conflict", "duplicate", "corrupt"] {
            match defect {
                "missing" => std::fs::remove_file(&summary_path).unwrap(),
                "empty" => write_airport_summary(&summary_path, &[]).unwrap(),
                "conflict" => {
                    let mut changed = rows.clone();
                    changed[0].airport_unique_arr_count += 1;
                    write_airport_summary(&summary_path, &changed).unwrap();
                }
                "duplicate" => {
                    write_airport_summary(&summary_path, &[rows[0].clone(), rows[0].clone()])
                        .unwrap()
                }
                "corrupt" => std::fs::write(&summary_path, b"broken").unwrap(),
                _ => unreachable!(),
            }
            let error = crate::collect_sources_at_point(&prepared, lat + 0.001, lon).unwrap_err();
            assert!(error.contains("airport_summary"), "{defect}: {error}");
            std::fs::write(&summary_path, &original).unwrap();
        }
        assert!(crate::collect_sources_at_point(&prepared, lat + 0.001, lon).is_ok());
        eprintln!("airport z9 owner→union→reader→popup ({lat},{lon}): global counts, hybrid normalization, local missing/empty/conflict/duplicate/corrupt PASS");
    }
}

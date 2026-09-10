//! Actual producer IPC files must decode into complete current popup views.

use super::*;
use aircraft_extract::{arrow_io::*, flight::*};
use arrow::{
    array::*,
    datatypes::{DataType, Field, Int32Type, Schema},
};
use noise_compute::compute::aircraft_v6::airport_traffic::AirportSummaryEntry;
use std::sync::Arc;

fn flight() -> FlightSegment {
    FlightSegment {
        flight_id: 42,
        callsign: "TEST42".into(),
        aircraft_type: *b"A320",
        profile_idx: 2,
        source_id: 2,
        origin: 0,
        veh_kind: 0,
        gse_class: 0,
        period: 2,
        date_id: 365,
        phase: Phase::Airborne,
        flags: 1,
        start_lat: 50.1,
        start_lon: 14.26,
        start_alt_m: 1000.4,
        end_lat: 50.11,
        end_lon: 14.27,
        end_alt_m: 1100.6,
        speed_kt: 250.0,
        length_m: 1200.0,
        agl_avg_m: 800.0,
        start_elev_m: 234.6,
        end_elev_m: 250.4,
    }
}

#[test]
fn producer_files_decode_geometry_identity_counts_and_windows() {
    let dir = tempfile::tempdir().unwrap();
    let airborne = dir.path().join("airborne.arrow");
    write_airborne(&airborne, &[flight()], 12, 365).unwrap();
    let (_, batches) = read_record_batches(&airborne).unwrap();
    assert_airborne_contract("airborne.arrow", &batches).unwrap();
    build_class_weights(&batches, &[], 12).unwrap();
    let accum = AirborneRowAccum::new(&batches).unwrap();
    let row = &accum.views()[0];
    let key = row.flight_key[0] as usize;
    assert_eq!(
        (
            row.flight_id[0],
            row.flights.callsign(key),
            row.flights.aircraft_type(key)
        ),
        (42, "TEST42", *b"A320")
    );
    assert_eq!(
        (
            row.flights.source_id[key],
            row.flights.profile_idx[key],
            row.flights.origin[key]
        ),
        (2, 2, 0)
    );
    assert_eq!(row.start_alt_m, &[1000]);
    assert_eq!(row.end_alt_m, &[1101]);
    assert_eq!(row.terrain_start_elev_m, &[235]);
    assert_eq!(row.terrain_end_elev_m, &[250]);
    assert_eq!(row.date_id, &[365]);
    assert_eq!(row.period, &[2]);
    assert_eq!(row.flags, &[1]);
    assert_eq!(row.speed_kt, &[250.0]);
    assert_eq!(row.length_m, &[1200.0]);
    let (gx, gy) = grid::lonlat_to_grid(f64::from(14.26_f32), f64::from(50.1_f32));
    let (lon, lat) = square_store::grid_cols::grid_cell_lonlat(gx, gy);
    assert_eq!(row.start_lat_lon(0), [lat as f32, lon as f32]);
    let xs = batches[0]
        .column_by_name("start_gx")
        .unwrap()
        .as_any()
        .downcast_ref::<Int32Array>()
        .unwrap();
    assert_eq!(row.start_gx.as_ptr(), xs.values().as_ptr());
    let callsigns = batches[0]
        .column_by_name("flight")
        .unwrap()
        .as_any()
        .downcast_ref::<DictionaryArray<Int32Type>>()
        .unwrap()
        .values()
        .as_any()
        .downcast_ref::<StructArray>()
        .unwrap()
        .column_by_name("callsign")
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap()
        .value_data()
        .as_ptr();
    assert_eq!(row.flights.callsign_bytes.as_ptr(), callsigns);

    let cruise = dir.path().join("cruise.arrow");
    let id = grid::cruise::cruise_cell_id(50.1, 14.26);
    write_cruise(
        &cruise,
        &[CruiseBucket {
            cruise_cell_id: id,
            class: 3,
            rep_profile_idx: 2,
            fl_bin: 4,
            period: 1,
            sum_length_m: 10000.0,
            rep_len_m: 2000.0,
            rep_alt_m: 11000.0,
            rep_speed_kt: 450.0,
            unique_count: 20,
            source_id: 2,
            origin: 0,
            top_candidates: vec![CruiseTopCandidate {
                flight_id: 42,
                callsign: "TEST42".into(),
                aircraft_type: *b"A320",
                peak_lmax_25m_db: 95.0,
                altitude_m: 11000.0,
            }],
        }],
        12,
    )
    .unwrap();
    let (_, batches) = read_record_batches(&cruise).unwrap();
    assert_cruise_contract("cruise.arrow", &batches).unwrap();
    let accum = CruiseRowAccum::new(&batches).unwrap();
    let slices = accum.views();
    let views = slices.as_row_views();
    assert_eq!(
        (views[0].lon, views[0].lat),
        grid::cruise::cruise_centroid(id)
    );
    assert_eq!(views[0].unique_count, 20);
    assert_eq!(views[0].top_candidates[0].callsign, "TEST42");
    assert_eq!(views[0].sum_length_m, 10000.0);

    let traffic = dir.path().join("airport_traffic.arrow");
    let end = grid::lonlat_to_grid(14.261, 50.1);
    write_airport_traffic(
        &traffic,
        &[AirportTrafficRow {
            airport_key: "LKTEST".into(),
            osm_id: 123,
            segment_idx: 7,
            geometry_kind: 0,
            start_gx: gx,
            start_gy: gy,
            end_gx: end.0,
            end_gy: end.1,
            length_m: 72.0,
            ops_kind: 1,
            is_departure: 1,
            veh_kind: 0,
            class_idx: 3,
            period: 2,
            band_energy_lin: [123.0; 8],
            unique_movement_count: 9,
            unique_arr_count: 0,
            unique_dep_count: 9,
            unique_gse_count_per_class: [0; 3],
            microseg_unique_count: 7,
            microseg_unique_arr_count: 0,
            microseg_unique_dep_count: 7,
            microseg_unique_gse_count_per_class: [0; 3],
            microseg_unique_ga_count: 2,
            microseg_unique_ga_arr_count: 0,
            microseg_unique_ga_dep_count: 2,
        }],
        12,
        365,
    )
    .unwrap();
    let (_, batches) = read_record_batches(&traffic).unwrap();
    assert_airport_traffic_contract("airport_traffic.arrow", &batches).unwrap();
    build_class_weights(&[], &batches, 12).unwrap();
    let accum = AirportTrafficRowAccum::new(&batches).unwrap();
    let views = accum.views();
    assert_eq!(
        (views[0].start_lon, views[0].start_lat),
        (lon as f32, lat as f32)
    );
    assert_eq!((views[0].osm_id, views[0].segment_idx), (123, 7));
    assert_eq!(views[0].airport_key, "LKTEST");
    assert_eq!(views[0].microseg_unique_count, 7);
    assert_eq!(views[0].microseg_unique_ga_count, 2);
    assert_eq!(views[0].band_energy_lin, &[123.0; 8]);

    let mut summaries = std::collections::BTreeMap::new();
    summaries.insert(
        "LKTEST".to_string(),
        AirportSummaryEntry {
            arr_count: 3,
            dep_count: 4,
            gse_count_per_class: [1, 2, 3],
            ops_count_per_kind: [5, 6, 7],
            ga_arr_count: 8,
            ga_dep_count: 9,
            ga_ops_count_per_kind: [10, 11, 12],
        },
    );
    stamp_airport_summaries(&traffic, &summaries).unwrap();
    let (schema, batches) = read_record_batches(&traffic).unwrap();
    let mut accum = AirportSummaryAccum::default();
    accum.merge_square(&schema, &batches).unwrap();
    assert_eq!(accum.lookup()["LKTEST"], summaries["LKTEST"]);
}

#[test]
fn current_stamps_never_turn_wrong_geometry_into_zero_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("airborne.arrow");
    write_airborne(&path, &[flight()], 12, 365).unwrap();
    let (_, batches) = read_record_batches(&path).unwrap();
    let batch = &batches[0];
    let schema = batch.schema();
    let columns: Vec<_> = schema
        .fields()
        .iter()
        .enumerate()
        .filter(|(_, field)| field.name() != "flight")
        .map(|(index, field)| (field.as_ref().clone(), batch.column(index).clone()))
        .collect();
    let bad = RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(
            columns
                .iter()
                .map(|(field, _)| field.clone())
                .collect::<Vec<_>>(),
            schema.metadata().clone(),
        )),
        columns.into_iter().map(|(_, array)| array).collect(),
    )
    .unwrap();
    assert!(AirborneRowAccum::new(&[bad])
        .err()
        .unwrap()
        .contains("flight"));
    let old_geometry = RecordBatch::new_empty(Arc::new(Schema::new(vec![Field::new(
        "start_gx",
        DataType::Float32,
        false,
    )])));
    assert!(AirportTrafficRowAccum::new(&[old_geometry]).is_err());
    let null = Arc::new(Int32Array::from(vec![None])) as ArrayRef;
    assert!(columns::required_array::<Int32Array>(Some(&null), "start_gx").is_err());
}

#[test]
fn cruise_popup_names_and_highlights_the_actual_producer_cell() {
    struct SeaLevel;
    impl RasterSampler for SeaLevel {
        fn elevation(&self, _: f64, _: f64) -> f64 {
            0.0
        }
        fn ground_g(&self, _: f64, _: f64) -> f64 {
            0.0
        }
        fn building_enclosure(&self, _: f64, _: f64) -> f64 {
            0.0
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cruise.arrow");
    // Adjacent cells share a z9 parent; edge columns must not wrap the polygon.
    for (x, y) in [
        (17680u64, 11120u64),
        (17681, 11120),
        (16384, 16384),
        (0, 16384),
        (32767, 16384),
        (16384, 0),
        (16384, 32767),
    ] {
        write_cruise(
            &path,
            &[CruiseBucket {
                cruise_cell_id: (x << 15) | y,
                class: 3,
                rep_profile_idx: 2,
                fl_bin: 4,
                period: 1,
                sum_length_m: 10000.0,
                rep_len_m: 2000.0,
                rep_alt_m: 11000.0,
                rep_speed_kt: 450.0,
                unique_count: 1,
                source_id: 2,
                origin: 0,
                top_candidates: vec![CruiseTopCandidate {
                    flight_id: 42,
                    callsign: "TEST42".into(),
                    aircraft_type: *b"A320",
                    peak_lmax_25m_db: 95.0,
                    altitude_m: 11000.0,
                }],
            }],
            12,
        )
        .unwrap();
        let (_, batches) = read_record_batches(&path).unwrap();
        let accum = CruiseRowAccum::new(&batches).unwrap();
        let slices = accum.views();
        let rows = slices.as_row_views();
        let mut traces = TraceCollector::default();
        noise_compute::compute::aircraft_v6::cruise::scatter(
            &Receiver::new(rows[0].lat, rows[0].lon, 0.0),
            &rows,
            &SeaLevel,
            12.0,
            &mut HashMap::new(),
            &mut HashMap::new(),
            &mut HashMap::new(),
            Some(&mut traces),
        );
        assert_eq!(traces.segments.len(), 1);
        let trace = &traces.segments[0];
        eprintln!(
            "cruise-cell {x}/{y} {}",
            serde_json::to_string(trace).unwrap()
        );
        assert_eq!(trace.name, format!("Cruise over z15/{x}/{y}"));
        let noise_compute::types::EmissionTrace::AircraftCruise { square, .. } = &trace.emission
        else {
            panic!("expected cruise emission");
        };
        assert_eq!(square, &format!("z15/{x}/{y}"));
        // Independent slippy-map inverse, not the production grid helper.
        let latitude = |row: u64| {
            (std::f64::consts::PI * (1.0 - 2.0 * row as f64 / 32768.0))
                .sinh()
                .atan()
                .to_degrees()
        };
        let west = x as f64 * 360.0 / 32768.0 - 180.0;
        let east = (x + 1) as f64 * 360.0 / 32768.0 - 180.0;
        let (south, north) = (latitude(y + 1), latitude(y));
        let expected = [
            (south, west),
            (south, east),
            (north, east),
            (north, west),
            (south, west),
        ];
        let polygon = trace.cell_polygon.as_ref().unwrap();
        assert_eq!(polygon.len(), expected.len());
        for ((lat, lon), (expected_lat, expected_lon)) in polygon.iter().zip(expected) {
            assert!(
                (lat - expected_lat).abs() < 1e-12,
                "latitude {lat} vs {expected_lat}"
            );
            assert!(
                (lon - expected_lon).abs() < 1e-12,
                "longitude {lon} vs {expected_lon}"
            );
        }
        assert_eq!(polygon.first(), polygon.last());
        assert!(trace.received_lden.full.is_finite());
    }
}

/// A sliced batch keeps its dictionary and every decoded coordinate, so a
/// block decoded from the middle of a file still joins the right flight.
#[test]
fn borrowed_geometry_preserves_sliced_rows_and_flight_keys() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("airborne.arrow");
    let mut rows = Vec::new();
    for (index, (lat, lon)) in [(-85.0, -179.99), (85.0, 179.99), (0.0, 0.0)]
        .into_iter()
        .enumerate()
    {
        let mut row = flight();
        row.flight_id += index as u64;
        row.callsign = format!("SLICE{index}");
        row.start_lat = lat;
        row.start_lon = lon;
        row.end_lat = lat;
        row.end_lon = lon + 0.001;
        rows.push(row);
    }
    write_airborne(&path, &rows, 12, 365).unwrap();
    let (_, batches) = read_record_batches(&path).unwrap();
    // Three rows in three z14 cells: one batch each, one shared dictionary.
    assert_eq!(batches.len(), 3);
    let sliced = [batches[1].slice(0, 1)];
    let accum = AirborneRowAccum::new(&sliced).unwrap();
    let batch = &accum.views()[0];
    assert_eq!(batch.len(), 1);
    let key = batch.flight_key[0] as usize;
    assert_eq!(batch.flights.len(), 3);
    assert_eq!(
        batch.flights.callsign(key),
        format!("SLICE{}", batch.flight_id[0] - 42)
    );
    for (gx, gy, actual) in [
        (batch.start_gx[0], batch.start_gy[0], batch.start_lat_lon(0)),
        (batch.end_gx[0], batch.end_gy[0], batch.end_lat_lon(0)),
    ] {
        let (lon, lat) = square_store::grid_cols::grid_cell_lonlat(gx, gy);
        assert_eq!(actual, [lat as f32, lon as f32]);
    }
}

/// A key beyond the dictionary is a corrupt file, never an out-of-bounds read.
#[test]
fn flight_keys_outside_the_dictionary_are_refused() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("airborne.arrow");
    write_airborne(&path, &[flight()], 12, 365).unwrap();
    let (_, batches) = read_record_batches(&path).unwrap();
    let batch = &batches[0];
    let index = batch.schema().index_of("flight").unwrap();
    let flight = batch
        .column(index)
        .as_any()
        .downcast_ref::<DictionaryArray<Int32Type>>()
        .unwrap();
    let mut columns = batch.columns().to_vec();
    // `try_new` refuses the key; a corrupt file would not have asked.
    columns[index] = Arc::new(unsafe {
        DictionaryArray::<Int32Type>::new_unchecked(
            Int32Array::from(vec![5]),
            flight.values().clone(),
        )
    });
    let bad = RecordBatch::try_new(batch.schema(), columns).unwrap();
    assert!(AirborneRowAccum::new(&[bad])
        .err()
        .unwrap()
        .contains("flight key"));
}

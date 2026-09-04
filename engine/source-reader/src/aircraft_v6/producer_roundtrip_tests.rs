//! Actual producer IPC files must decode into complete current popup views.

use super::*;
use aircraft_extract::{arrow_io::*, flight::*, spatial};
use arrow::{
    array::*,
    datatypes::{DataType, Field, Schema},
};
use std::sync::Arc;

fn flight() -> AirborneEvent {
    AirborneEvent {
        flight_id: 42,
        callsign: "TEST42".into(),
        aircraft_type: *b"A320",
        profile_idx: 2,
        source_id: 2,
        origin: 0,
        sub_segments: vec![AirborneSubSegment {
            start_lat: 50.1,
            start_lon: 14.26,
            start_alt_m: 1000.4,
            end_lat: 50.11,
            end_lon: 14.27,
            end_alt_m: 1100.6,
            speed_kt: 250.0,
            length_m: 1200.0,
            period: 2,
            date_id: 365,
            flags: 1,
            terrain_start_elev_m: 234.6,
            terrain_end_elev_m: 250.4,
        }],
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
    let views = accum.views();
    let row = &views[0];
    assert_eq!(
        (row.flight_id, row.callsign, row.aircraft_type),
        (42, "TEST42", *b"A320")
    );
    assert_eq!((row.source_id, row.profile_idx, row.origin), (2, 2, 0));
    assert_eq!(row.sub_segments.start_alt_m, &[1000.0]);
    assert_eq!(row.sub_segments.end_alt_m, &[1101.0]);
    assert_eq!(row.sub_segments.terrain_start_elev_m, &[235.0]);
    assert_eq!(row.sub_segments.terrain_end_elev_m, &[250.0]);
    assert_eq!(row.sub_segments.date_id, &[365]);
    assert_eq!(row.sub_segments.period, &[2]);
    assert_eq!(row.sub_segments.flags, &[1]);
    assert_eq!(row.sub_segments.speed_kt, &[250.0]);
    assert_eq!(row.sub_segments.length_m, &[1200.0]);
    let (gx, gy) = grid::lonlat_to_grid(f64::from(14.26_f32), f64::from(50.1_f32));
    let (lon, lat) = square_store::grid_cols::grid_cell_lonlat(gx, gy);
    assert_eq!(row.sub_segments.start_lat, &[lat as f32]);
    assert_eq!(row.sub_segments.start_lon, &[lon as f32]);

    let cruise = dir.path().join("cruise.arrow");
    let id = spatial::cruise_cell_id(50.1, 14.26);
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
    assert_eq!((views[0].lon, views[0].lat), spatial::cruise_centroid(id));
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

    let summary = dir.path().join("airport_summary.arrow");
    write_airport_summary(
        &summary,
        &[AirportSummaryRow {
            airport_key: "LKTEST".into(),
            airport_unique_arr_count: 3,
            airport_unique_dep_count: 4,
            airport_unique_gse_count_per_class: [1, 2, 3],
            airport_unique_ops_count_per_kind: [5, 6, 7],
            airport_unique_ga_arr_count: 8,
            airport_unique_ga_dep_count: 9,
            airport_unique_ga_ops_count_per_kind: [10, 11, 12],
        }],
    )
    .unwrap();
    let accum = airport_summary_view::load_airport_summary(&summary)
        .unwrap()
        .unwrap();
    let row = &accum.lookup()["LKTEST"];
    assert_eq!(
        (
            row.arr_count,
            row.dep_count,
            row.ga_arr_count,
            row.ga_dep_count
        ),
        (3, 4, 8, 9)
    );
    assert_eq!(row.gse_count_per_class, [1, 2, 3]);
    assert_eq!(row.ga_ops_count_per_kind, [10, 11, 12]);
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
        .filter(|(_, field)| field.name() != "sub_segments")
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
        .contains("sub_segments"));
    let old_geometry = RecordBatch::new_empty(Arc::new(Schema::new(vec![Field::new(
        "start_gx",
        DataType::Float32,
        false,
    )])));
    assert!(AirportTrafficRowAccum::new(&[old_geometry]).is_err());
    let null = Arc::new(Int32Array::from(vec![None])) as ArrayRef;
    assert!(columns::required_array::<Int32Array>(Some(&null), "start_gx").is_err());
}

//! Road tests.

use super::*;

#[test]
fn observed_timing_attribution_survives_reader_to_result_json() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (lat, lon) = (50.0, 14.0);
    let dir = fx::square_dir(tmp.path(), grid::square_of(lat, lon));
    std::fs::create_dir_all(&dir).unwrap();
    let dictionary = r#"{"source":"https://www.fhwa.dot.gov/policyinformation/tables/tmasdata/","entries":[{"station":"6-021560","window":"2025-01..2025-12","days":360,"status":"total hourly volumes; no vehicle classes","profile":{"total":[0.80,0.10,0.10],"heavy":[0.55,0.18,0.27]}}]}"#;
    fx::write_roads_file_opts(
        &dir.join("roads.arrow"),
        &[
            fx::FixtureRoad {
                osm_id: 11,
                start: (lon, lat),
                end: (lon + 0.001, lat),
                traffic_profile_id: 1,
                ..Default::default()
            },
            fx::FixtureRoad {
                osm_id: 12,
                start: (lon, lat),
                end: (lon + 0.001, lat),
                traffic_profile_id: 0,
                ..Default::default()
            },
        ],
        Some(dictionary),
    );
    let square = load_square(&dir).unwrap();
    let results = query_roads_from_batches(
        &square.roads.batches_all().unwrap(),
        lat,
        lon,
        noise_compute::constants::ROAD_MAX_RADIUS[0],
    )
    .unwrap();
    assert_eq!(results.len(), 2);
    for r in &results {
        let wire = serde_json::to_value(r).unwrap();
        if r.osm_id == 11 {
            let attr = r
                .time_profile_attribution
                .as_ref()
                .expect("profiled row carries attribution");
            assert_eq!(
                attr.source,
                "https://www.fhwa.dot.gov/policyinformation/tables/tmasdata/"
            );
            assert_eq!(attr.window, "2025-01..2025-12");
            assert!(
                attr.total_transfer,
                "light/medium/moto inherit the total share"
            );
            assert_eq!(wire["time_profile_attribution"]["total_transfer"], true);
            assert!(
                r.time_profile.is_some(),
                "emission input rides along untouched"
            );
            assert!(
                wire.get("time_profile").is_none(),
                "shares stay off the wire"
            );
        } else {
            assert!(
                r.time_profile_attribution.is_none(),
                "id 0 stays unattributed"
            );
            assert!(
                wire.get("time_profile_attribution").is_none(),
                "absence omitted on the wire"
            );
            assert!(r.time_profile.is_none());
        }
    }
}

fn year_with_road() -> tempfile::TempDir {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = fx::square_dir(tmp.path(), prague());
    std::fs::create_dir_all(&dir).unwrap();
    fx::write_roads_file(
        &dir.join("roads.arrow"),
        &[fx::FixtureRoad {
            osm_id: 123,
            start: (LON, LAT),
            end: (LON + 0.002, LAT + 0.001),
            road_class: 2,
            speed_limit: 50,
            lanes: 2,
            name: "Test Street".to_string(),
            ..Default::default()
        }],
    );
    tmp
}

#[test]
fn grid_road_row_collects_with_lonlat_geometry() {
    let tmp = year_with_road();
    let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
    assert_eq!(data.roads.len(), 1);
    let r = &data.roads[0];
    assert_eq!(r.osm_id, 123);
    assert_eq!(r.name, "Test Street");
    assert!((r.start_lon - LON).abs() < 0.001, "slon={}", r.start_lon);
    assert!((r.start_lat - LAT).abs() < 0.001, "slat={}", r.start_lat);
    assert_eq!(
        r.square_country_city, None,
        "no baked columns → receiver fallback"
    );
}

#[test]
fn far_road_row_is_rejected() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = fx::square_dir(tmp.path(), prague());
    std::fs::create_dir_all(&dir).unwrap();
    fx::write_roads_file(
        &dir.join("roads.arrow"),
        &[fx::FixtureRoad {
            osm_id: 9,
            start: (LON + 5.0, LAT + 5.0),
            end: (LON + 5.002, LAT + 5.001),
            road_class: 2,
            speed_limit: 50,
            lanes: 2,
            name: String::new(),
            ..Default::default()
        }],
    );
    let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
    assert!(data.roads.is_empty());
}

#[test]
fn far_row_is_rejected_by_its_own_class_reach() {
    use noise_compute::constants::ROAD_MAX_RADIUS;
    let between = (ROAD_MAX_RADIUS[5] + ROAD_MAX_RADIUS[0]) / 2.0;
    let lat = LAT + between / grid::geo::M_PER_DEG_LAT;
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = fx::square_dir(tmp.path(), prague());
    std::fs::create_dir_all(&dir).unwrap();
    let road = |osm_id, road_class| fx::FixtureRoad {
        osm_id,
        start: (LON, lat),
        end: (LON + 0.002, lat),
        road_class,
        speed_limit: 50,
        lanes: 2,
        name: String::new(),
        ..Default::default()
    };
    fx::write_roads_file(&dir.join("roads.arrow"), &[road(1, 0), road(2, 5)]);
    let square = load_square(&dir).unwrap();
    let kept = query_roads_from_batches(
        &square.roads.batches_all().unwrap(),
        LAT,
        LON,
        ROAD_MAX_RADIUS[0],
    )
    .unwrap();
    assert_eq!(kept.iter().map(|r| r.osm_id).collect::<Vec<_>>(), vec![1]);
}

#[test]
fn prepared_traffic_is_consumed_verbatim() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = fx::square_dir(tmp.path(), prague());
    std::fs::create_dir_all(&dir).unwrap();
    fx::write_roads_file(
        &dir.join("roads.arrow"),
        &[
            fx::FixtureRoad {
                osm_id: 11,
                start: (LON, LAT),
                end: (LON + 0.002, LAT),
                aadt_light: 0.0,
                aadt_medium: 0.0,
                aadt_heavy: 500.0,
                aadt_moto: 0.0,
                traffic_estimated: 0,
                access: 2,
                ..Default::default()
            },
            fx::FixtureRoad {
                osm_id: 12,
                start: (LON, LAT),
                end: (LON + 0.002, LAT),
                aadt_light: 0.0,
                aadt_medium: 0.0,
                aadt_heavy: 0.0,
                aadt_moto: 0.0,
                ..Default::default()
            },
            fx::FixtureRoad {
                osm_id: 13,
                start: (LON, LAT),
                end: (LON + 0.002, LAT),
                ..Default::default()
            },
        ],
    );
    let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
    assert_eq!(
        data.roads.iter().map(|r| r.osm_id).collect::<Vec<_>>(),
        vec![11, 13],
        "heavy-only on access=no emits; true zero stays silent"
    );
    let heavy = &data.roads[0];
    assert_eq!(heavy.traffic.heavy, 500.0);
    assert_eq!(heavy.traffic.light, 0.0);
    assert_eq!(heavy.traffic.estimated, 0);
    assert_eq!(heavy.source_id, 0, "source_id=0 prior is valid traffic");
    let prior = &data.roads[1];
    assert_eq!(prior.traffic.light, 3_000.0);
    assert_eq!(prior.traffic.estimated, 15);
}

#[test]
fn unfinalized_road_arrow_is_rejected() {
    use arrow::array::{ArrayRef, Int32Array, UInt8Array};
    use std::sync::Arc;
    let batch = |metadata: Option<&str>| {
        let mut columns: Vec<(&str, ArrayRef)> = vec![
            ("osm_id", Arc::new(arrow::array::Int64Array::from(vec![1]))),
            ("road_class", Arc::new(UInt8Array::from(vec![2]))),
            ("oneway", Arc::new(UInt8Array::from(vec![0]))),
            ("aadt_light", Arc::new(Int32Array::from(vec![10_000]))),
        ];
        let fields = columns
            .iter()
            .map(|(name, array)| {
                arrow::datatypes::Field::new(*name, array.data_type().clone(), false)
            })
            .collect::<Vec<_>>();
        let schema = arrow::datatypes::Schema::new(fields);
        let schema = match metadata {
            Some(value) => schema.with_metadata(std::collections::HashMap::from([(
                "road_traffic_contract".to_owned(),
                value.to_owned(),
            )])),
            None => schema,
        };
        arrow::record_batch::RecordBatch::try_new(
            Arc::new(schema),
            columns.iter_mut().map(|(_, array)| array.clone()).collect(),
        )
        .unwrap()
    };
    for metadata in [None, Some("1")] {
        let err = query_roads_from_batches(&[batch(metadata)], LAT, LON, 1000.0).unwrap_err();
        assert!(
            err.contains("road_traffic_contract") || err.contains("road traffic column"),
            "got: {err}"
        );
    }
}

//! Actual native popup keeps clicked metadata and accumulates airborne rows from every owner square.

use aircraft_extract::{arrow_io, flight::FlightSegment};
use raster_reader::channel::Channel;
use serde_json::Value;
use std::path::Path;

use crate::structure_test_fixture as fx;

pub(super) fn facade_popup_preserves_aircraft_and_observation_multiplicity(root: &Path) {
    let lat = -2.0 / grid::geo::M_PER_DEG_LAT;
    let lon = 0.35;
    let north_wall = 0.5 / grid::geo::M_PER_DEG_LAT;
    let click_square = grid::square_of(lat, lon);
    let house = fx::StructureRow {
        kind: square_store::store::STRUCTURE_KIND_BUILDING,
        ring_lonlat: Some(vec![
            (lon - 0.001, -0.001),
            (lon + 0.001, -0.001),
            (lon + 0.001, north_wall),
            (lon - 0.001, north_wall),
            (lon - 0.001, -0.001),
        ]),
        centroid_lonlat: Some((lon, -0.0005)),
        height_m: 12,
        height_tier: 0,
        envelope_class: 1,
        osm_id: Some(901),
        building_type: Some(1),
        area_m2: Some(1000.0),
        ..Default::default()
    };
    fx::write_square_structures(root, click_square, &[house]);
    let set = crate::structure_store::load_obstacle_set(root, lat, lon).unwrap();
    let (facade_lat, facade_lon, winner) =
        crate::structure_store::locate_facade_receiver(&set, lat, lon);
    assert!(winner.is_some());
    let facade_square = grid::square_of(facade_lat, facade_lon);
    assert_ne!(facade_square, click_square);
    for channel in Channel::ALL {
        for square in crate::query::squares_within_reach(facade_lat, facade_lon).unwrap() {
            let ocean = channel.path(root, square);
            std::fs::create_dir_all(ocean.parent().unwrap()).unwrap();
            std::fs::write(ocean, []).unwrap();
        }
    }
    assert!(crate::RASTERS
        .set(raster_reader::RealRasters::new(root))
        .is_ok());
    let row = FlightSegment {
        flight_id: 42,
        callsign: "FACADE42".into(),
        aircraft_type: *b"B738",
        profile_idx: noise_compute::emission::profiles_generated::profile_idx("B738"),
        source_id: 2,
        origin: 0,
        veh_kind: 0,
        gse_class: 0,
        period: 0,
        date_id: 0,
        phase: aircraft_extract::flight::Phase::Airborne,
        flags: 1,
        start_lat: 0.001,
        start_lon: 0.349,
        start_alt_m: 1000.0,
        end_lat: 0.001,
        end_lon: 0.351,
        end_alt_m: 1000.0,
        speed_kt: 450.0,
        length_m: grid::geo::flat_dist(0.001, 0.349, 0.001, 0.351) as f32,
        agl_avg_m: 1000.0,
        start_elev_m: 0.0,
        end_elev_m: 0.0,
    };
    let facade_dir = fx::square_dir(root, facade_square);
    std::fs::create_dir_all(&facade_dir).unwrap();
    fx::write_square_structures(root, facade_square, &[]);
    let path = facade_dir.join("airborne.arrow");
    arrow_io::write_airborne(&path, std::slice::from_ref(&row), 12, 0).unwrap();
    let popup = |lat, lon| -> Value {
        super::reset_store(root);
        let mut value: Value =
            serde_json::from_str(&crate::query_noise_at_point(lat, lon).unwrap()).unwrap();
        value.as_object_mut().unwrap().remove("timings");
        value
    };
    let inside = popup(lat, lon);
    let outside = popup(facade_lat, facade_lon);
    assert_eq!(inside["center"], serde_json::json!([lat, lon]));
    assert_eq!(inside["envelope_class"], "residential");
    let outdoor_total = outside["total_lden"].as_f64().unwrap();
    assert_eq!(inside["facade_lden"], (outdoor_total * 10.0).round() / 10.0);
    let building_distance = |value: &Value| {
        value["top_contributors"]
            .as_array()
            .unwrap()
            .iter()
            .find(|source| source["source_type"] == "building" && source["osm_id"] == 901)
            .unwrap()["distance_m"]
            .as_i64()
            .unwrap()
    };
    assert!(building_distance(&outside) > 0);
    assert_eq!(
        building_distance(&inside),
        building_distance(&outside),
        "moving an indoor receiver must update source distances before screening"
    );
    let airborne = |value: &Value| {
        value["sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|source| source["source_type"] == "aircraft")
            .unwrap()
            .clone()
    };
    let first = airborne(&outside);
    assert!(first["lden"].as_f64().unwrap() > 0.0);
    assert_eq!(first["segment_count"].as_u64().unwrap(), 1);
    assert_eq!(airborne(&inside)["segment_count"], first["segment_count"]);
    // Rows are owned by their square: a row stored in the neighbouring square
    // is a second observation for this receiver, not a copy to ignore.
    arrow_io::write_airborne(
        &fx::square_dir(root, click_square).join("airborne.arrow"),
        std::slice::from_ref(&row),
        12,
        0,
    )
    .unwrap();
    let neighbours = airborne(&popup(facade_lat, facade_lon));
    assert_eq!(neighbours["segment_count"].as_u64().unwrap(), 2);
    let increase = neighbours["lden"].as_f64().unwrap() - first["lden"].as_f64().unwrap();
    assert!(
        (increase - 2.0_f64.log10() * 10.0).abs() < 1e-9,
        "second owner square increase: {increase}"
    );
    // Two original observations remain two energy contributions, even when identical.
    arrow_io::write_airborne(&path, &[row.clone(), row], 12, 0).unwrap();
    let tripled = airborne(&popup(facade_lat, facade_lon));
    assert_eq!(tripled["segment_count"].as_u64().unwrap(), 3);
    let increase = tripled["lden"].as_f64().unwrap() - first["lden"].as_f64().unwrap();
    assert!(
        (increase - 3.0_f64.log10() * 10.0).abs() < 1e-9,
        "duplicate observation increase: {increase}"
    );
    // Neighbouring receivers share the owner squares and the selected rows,
    // and a warm cache returns the previous click's exact answer.
    let south = (-0.005, lon);
    let north = (0.005, lon);
    let mut south_squares = crate::query::squares_within_reach(south.0, south.1).unwrap();
    let mut north_squares = crate::query::squares_within_reach(north.0, north.1).unwrap();
    south_squares.sort_by_key(|square| (square.x, square.y));
    north_squares.sort_by_key(|square| (square.x, square.y));
    assert_eq!(south_squares, north_squares);
    let expected_north = popup(north.0, north.1);
    let south_popup = popup(south.0, south.1);
    assert_eq!(
        airborne(&south_popup)["segment_count"],
        airborne(&expected_north)["segment_count"]
    );
    assert_ne!(
        airborne(&south_popup)["lden"],
        airborne(&expected_north)["lden"]
    );
    let mut warm_north: Value =
        serde_json::from_str(&crate::query_noise_at_point(north.0, north.1).unwrap()).unwrap();
    warm_north.as_object_mut().unwrap().remove("timings");
    assert_eq!(
        warm_north, expected_north,
        "previous click must not change aircraft sources"
    );
}

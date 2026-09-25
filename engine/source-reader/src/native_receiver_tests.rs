//! Actual native popup answers a building click at its stored façade receiver and accumulates airborne rows from every owner square.

use aircraft_extract::{arrow_io, flight::FlightSegment};
use raster_reader::channel::Channel;
use serde_json::Value;
use std::path::Path;

use crate::structure_test_fixture as fx;

pub(super) fn building_popup_uses_its_stored_facade_receiver_and_keeps_aircraft_multiplicity(root: &Path) {
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
        height_source: 0,
        envelope_class: 1,
        osm_id: Some(901),
        building_type: Some(1),
        area_m2: Some(1000.0),
        ..Default::default()
    };
    let structures = fx::write_square_structures(root, click_square, &[house]);
    let set = crate::structure_store::load_obstacle_set(root, lat, lon).unwrap();
    let building = set.enclosed_footprint_at(lat, lon).expect("the click is inside the house");
    let (_, rings) = crate::structure_store::enclosed_building_footprints(
        &std::fs::read(&structures).unwrap(),
        &structures,
    )
    .unwrap()
    .remove(0);
    let receivers = noise_compute::facade_receivers::exposed_facade_receivers(&rings, &set);
    // The stage's choice, here the north-wall receiver nearest the click: it
    // stands in the next square, so the popup must load sources around it.
    let chosen = (0..receivers.len())
        .filter(|&i| receivers[i].latitude_longitude().0 > 0.0)
        .min_by(|&a, &b| {
            let off = |i: usize| (receivers[i].latitude_longitude().1 - lon).abs();
            off(a).total_cmp(&off(b))
        })
        .unwrap();
    let click_dir = fx::square_dir(root, click_square);
    fx::write_facade_exposure(
        &click_dir,
        &[fx::facade_exposure_row(building.key.id, &receivers, Some(chosen))],
    );
    let (facade_lat, facade_lon) = receivers[chosen].latitude_longitude();
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
    arrow_io::write_airborne(&path, std::slice::from_ref(&row), &fx::sampling_window(12, 0)).unwrap();
    let popup = |lat, lon| -> Value {
        super::reset_store(root);
        let mut value: Value =
            serde_json::from_str(&crate::query_noise_at_point(lat, lon, None).unwrap()).unwrap();
        value.as_object_mut().unwrap().remove("timings");
        value
    };
    let inside = popup(lat, lon);
    let outside = popup(facade_lat, facade_lon);
    assert_eq!(inside["center"], serde_json::json!([lat, lon]));
    assert_eq!(
        inside["building_exposure"],
        serde_json::json!({
            "receiver": [facade_lat, facade_lon],
            "facade_bearing_deg": 0.0,
            "facade_points": receivers.len(),
        })
    );
    assert!(outside.get("building_exposure").is_none());
    for removed in ["envelope_class", "envelope_delta_db", "facade_lden", "indoor_lden_tilted"] {
        assert!(inside.get(removed).is_none(), "{removed} left the popup");
    }
    let building_lden = |value: &Value| {
        value["top_contributors"]
            .as_array()
            .unwrap()
            .iter()
            .find(|source| source["source_type"] == "building" && source["osm_id"] == 901)
            .unwrap()["received_lden"]
            .as_f64()
            .unwrap()
    };
    // Three of the nine density probes at the north wall fall inside the house:
    // an outdoor point there gets 1.5 dB, its own façade's receiver none (§2.8).
    assert!(
        (building_lden(&outside) - building_lden(&inside) - 1.5).abs() < 0.051,
        "own-façade bonus: outside {} inside {}",
        building_lden(&outside),
        building_lden(&inside)
    );
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
        "a building click must measure source distances from its façade receiver"
    );
    // The answer names the point it computed: the facade point for an indoor click, 4 m up.
    let receiver = |value: &Value| {
        let receiver = &value["receiver"];
        [&receiver["lat"], &receiver["lng"], &receiver["height_m"]].map(|v| v.as_f64().unwrap())
    };
    let default_height = noise_compute::constants::DEFAULT_RECEIVER_HEIGHT;
    assert_eq!(receiver(&inside), [facade_lat, facade_lon, default_height]);
    assert_eq!(receiver(&outside), [facade_lat, facade_lon, default_height]);
    // A microphone height is a runtime receiver choice: an explicit 4 m is the default answer,
    // another height moves every layer's geometry, and a height under the floor is refused.
    let popup_at_height = |height: f64| -> Value {
        super::reset_store(root);
        let mut value: Value = serde_json::from_str(
            &crate::query_noise_at_point(facade_lat, facade_lon, Some(height)).unwrap(),
        )
        .unwrap();
        value.as_object_mut().unwrap().remove("timings");
        value
    };
    assert_eq!(popup_at_height(default_height), outside);
    let low = popup_at_height(1.2);
    assert_eq!(receiver(&low), [facade_lat, facade_lon, 1.2]);
    assert_ne!(low["total_lden"], outside["total_lden"]);
    assert!(crate::query_noise_at_point(facade_lat, facade_lon, Some(0.4)).is_err());
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
        std::slice::from_ref(&row), &fx::sampling_window(12, 0))
    .unwrap();
    let neighbours = airborne(&popup(facade_lat, facade_lon));
    assert_eq!(neighbours["segment_count"].as_u64().unwrap(), 2);
    let increase = neighbours["lden"].as_f64().unwrap() - first["lden"].as_f64().unwrap();
    assert!(
        (increase - 2.0_f64.log10() * 10.0).abs() < 1e-9,
        "second owner square increase: {increase}"
    );
    // Two original observations remain two energy contributions, even when identical.
    arrow_io::write_airborne(&path, &[row.clone(), row.clone()], &fx::sampling_window(12, 0)).unwrap();
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
        serde_json::from_str(&crate::query_noise_at_point(north.0, north.1, None).unwrap()).unwrap();
    warm_north.as_object_mut().unwrap().remove("timings");
    assert_eq!(
        warm_north, expected_north,
        "previous click must not change aircraft sources"
    );
    // Owner squares disagreeing on the sampling windows cost the visitor the aircraft layer,
    // named in the answer; the building keeps its level and the popup is never refused.
    arrow_io::write_airborne(&path, std::slice::from_ref(&row), &fx::sampling_window(12, 5)).unwrap();
    let without_aircraft = popup(facade_lat, facade_lon);
    assert_eq!(
        without_aircraft["unavailable_layers"],
        serde_json::json!(["aircraft"])
    );
    let served_layers = without_aircraft["sources"].as_array().unwrap();
    assert!(served_layers.iter().all(|source| source["source_type"] != "aircraft"));
    assert!(building_distance(&without_aircraft) > 0);
    assert!(outside.get("unavailable_layers").is_none());
    assert!(
        crate::STORE.read().unwrap().squares.is_empty(),
        "a square served with a fault is reloaded on the next click"
    );
    // A release without the stage's file refuses a building click, never answers
    // it from another point; outdoor clicks are unaffected.
    std::fs::remove_file(click_dir.join("facade_exposure.arrow")).unwrap();
    super::reset_store(root);
    let missing = crate::query_noise_at_point(lat, lon, None).unwrap_err();
    assert!(missing.to_string().contains("building exposure missing"), "{missing}");
    assert!(crate::query_noise_at_point(facade_lat, facade_lon, None).is_ok());
    // The screening table is never dropped: a stale stamp refuses the popup end to end,
    // even though its paired index still maps.
    arrow_io::write_airborne(&path, std::slice::from_ref(&row), &fx::sampling_window(12, 0)).unwrap();
    fx::write_structure_file(&facade_dir.join("structures.arrow"), &[], false);
    let refused = crate::query_noise_at_point(facade_lat, facade_lon, None).unwrap_err();
    assert!(refused.to_string().contains("structures_contract mismatch"), "{refused}");
}

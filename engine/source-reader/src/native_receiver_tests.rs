//! Actual native popup keeps clicked metadata and reads aircraft exactly once at its facade.

use aircraft_extract::{
    arrow_io,
    flight::{AirborneEvent, AirborneSubSegment},
};
use raster_reader::{
    catalog::{begin_channel, record_square},
    channel::Channel,
};
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
        ..Default::default()
    };
    fx::write_square_structures(root, click_square, &[house]);
    let set = crate::structure_store::load_obstacle_set(root, root, lat, lon).unwrap();
    let (facade_lat, facade_lon, winner) =
        crate::structure_store::locate_facade_receiver(&set, lat, lon);
    assert!(winner.is_some());
    let facade_square = grid::square_of(facade_lat, facade_lon);
    assert_ne!(facade_square, click_square);
    for channel in Channel::ALL {
        let database = begin_channel(root, channel, &"a".repeat(64)).unwrap();
        for square in crate::query::squares_within_reach(facade_lat, facade_lon).unwrap() {
            record_square(&database, channel, square, None).unwrap();
        }
    }
    assert!(crate::RASTERS
        .set(raster_reader::RealRasters::new(root))
        .is_ok());
    let event = AirborneEvent {
        flight_id: 42,
        callsign: "FACADE42".into(),
        aircraft_type: *b"B738",
        profile_idx: noise_compute::emission::profiles_generated::profile_idx("B738"),
        source_id: 2,
        origin: 0,
        sub_segments: vec![AirborneSubSegment {
            start_lat: 0.001,
            end_lat: 0.001,
            start_lon: 0.349,
            end_lon: 0.351,
            start_alt_m: 1000.0,
            end_alt_m: 1000.0,
            speed_kt: 450.0,
            length_m: grid::geo::flat_dist(0.001, 0.349, 0.001, 0.351) as f32,
            period: 0,
            date_id: 0,
            flags: 1,
            terrain_start_elev_m: 0.0,
            terrain_end_elev_m: 0.0,
        }],
    };
    let facade_dir = fx::square_dir(root, facade_square);
    std::fs::create_dir_all(&facade_dir).unwrap();
    fx::write_structure_file(&facade_dir.join("structures.arrow"), &[], true);
    let path = facade_dir.join("airborne.arrow");
    arrow_io::write_airborne(&path, std::slice::from_ref(&event), 12, 0).unwrap();
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
    assert!(first["segment_count"].as_u64().unwrap() > 0);
    assert_eq!(airborne(&inside)["segment_count"], first["segment_count"]);
    // A second spatial copy is invisible to this receiver, including all wire fields.
    arrow_io::write_airborne(
        &fx::square_dir(root, click_square).join("airborne.arrow"),
        std::slice::from_ref(&event),
        12,
        0,
    )
    .unwrap();
    assert_eq!(popup(lat, lon), inside);
    // Two original observations remain two energy contributions, even when identical.
    arrow_io::write_airborne(&path, &[event.clone(), event], 12, 0).unwrap();
    let doubled = airborne(&popup(facade_lat, facade_lon));
    let increase = doubled["lden"].as_f64().unwrap() - first["lden"].as_f64().unwrap();
    assert!(
        (increase - 2.0_f64.log10() * 10.0).abs() < 1e-9,
        "duplicate observation increase: {increase}"
    );
}

pub(super) fn native_listings_honor_requested_radius(root: &Path) {
    let owner = grid::square_of(0.0, 2.5);
    let directory = fx::square_dir(root, owner);
    std::fs::create_dir_all(&directory).unwrap();
    fx::write_structure_file(
        &directory.join("structures.arrow"),
        &[fx::StructureRow {
            kind: square_store::store::STRUCTURE_KIND_BUILDING,
            centroid_lonlat: Some((2.5, 0.0)),
            osm_id: Some(901),
            ring_lonlat: Some(fx::square_ring_lonlat(0.0, 2.5)),
            height_m: 12,
            building_type: Some(1),
            ..Default::default()
        }],
        true,
    );
    super::reset_store(root);
    let large: Value =
        serde_json::from_str(&crate::query_buildings(0.0, 0.35, 250_000.0).unwrap()).unwrap();
    assert_eq!(large.as_array().unwrap().len(), 1);
    assert_eq!(large[0]["osm_id"], 901);
    let small: Value =
        serde_json::from_str(&crate::query_buildings(0.0, 0.35, 1000.0).unwrap()).unwrap();
    assert!(small.as_array().unwrap().is_empty());
}

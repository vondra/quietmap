//! Settlement tests.

use super::*;

fn building_row(osm_id: i64) -> fx::StructureRow {
    fx::StructureRow {
        kind: square_store::store::STRUCTURE_KIND_BUILDING,
        ring_lonlat: Some(fx::square_ring_lonlat(LAT, LON)),
        height_m: 12,
        height_tier: 0,
        envelope_class: 1,
        centroid_lonlat: Some((LON + 0.0001, LAT + 0.0001)),
        osm_id: Some(osm_id),
        building_type: Some(1),
        area_m2: Some(450.0),
        ..Default::default()
    }
}

#[test]
fn building_rows_feed_emission_and_walls_do_not() {
    let tmp = tempfile::TempDir::new().unwrap();
    fx::write_square_structures(
        tmp.path(),
        prague(),
        &[
            building_row(55),
            fx::StructureRow {
                kind: square_store::store::STRUCTURE_KIND_BARRIER,
                ring_lonlat: Some(vec![(LON, LAT), (LON + 0.001, LAT + 0.001)]),
                height_m: 3,
                height_tier: 0,
                envelope_class: 0,
                centroid_lonlat: Some((LON + 0.0005, LAT + 0.0005)),
                osm_id: Some(66),
                segment_idx: Some(0),
                ..Default::default()
            },
            fx::StructureRow {
                kind: square_store::store::STRUCTURE_KIND_BUILDING,
                ring_lonlat: Some(fx::square_ring_lonlat(LAT + 0.001, LON + 0.001)),
                height_m: 8,
                height_tier: 2,
                envelope_class: 5,
                centroid_lonlat: Some((LON + 0.001, LAT + 0.001)),
                ..Default::default()
            },
        ],
    );
    let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
    assert_eq!(data.buildings.len(), 1);
    assert_eq!(data.buildings[0].osm_id, 55);
}

#[test]
fn far_building_is_outside_the_building_reach() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut far = building_row(57);
    far.centroid_lonlat = Some((LON, LAT + 0.03));
    far.ring_lonlat = Some(fx::square_ring_lonlat(LAT + 0.03, LON));
    fx::write_square_structures(tmp.path(), prague(), &[building_row(55), far]);
    let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
    assert_eq!(
        data.buildings.iter().map(|b| b.osm_id).collect::<Vec<_>>(),
        vec![55]
    );
}

#[test]
fn emission_overrides_win_over_screening_geometry() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut row = building_row(56);
    row.emission_centroid_lonlat = Some((LON + 0.0002, LAT + 0.0002));
    row.emission_ring_lonlat = Some(fx::square_ring_lonlat(LAT + 0.0002, LON + 0.0002));
    fx::write_square_structures(tmp.path(), prague(), &[row]);
    let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
    assert_eq!(data.buildings.len(), 1);
    let pt = &data.buildings[0];
    assert!((pt.lon - (LON + 0.0002)).abs() < 0.0002, "lon={}", pt.lon);
}

#[test]
fn leisure_folds_into_buildings_with_sport_tag() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = fx::square_dir(tmp.path(), prague());
    std::fs::create_dir_all(&dir).unwrap();
    fx::write_leisure_file(
        &dir.join("leisure.arrow"),
        &[fx::FixtureLeisure {
            osm_id: 88,
            centroid: (LON, LAT),
            sport: 3,
            name: "Court".to_string(),
        }],
    );
    let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
    assert_eq!(data.buildings.len(), 1);
    assert_eq!(
        data.buildings[0].source_type,
        noise_compute::types::LEISURE_TYPE_BASE + 3
    );
}

#[test]
fn industrial_row_collects() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = fx::square_dir(tmp.path(), prague());
    std::fs::create_dir_all(&dir).unwrap();
    fx::write_industrial_file(
        &dir.join("industrial.arrow"),
        &[fx::FixtureIndustrial {
            osm_id: 99,
            centroid: (LON, LAT),
            source_type: 0,
            name: "Plant".to_string(),
        }],
    );
    let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
    assert_eq!(data.industrial.len(), 1);
    assert_eq!(data.industrial[0].osm_id, 99);
}

#[test]
fn unstamped_structures_fail_loud() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = fx::square_dir(tmp.path(), prague());
    std::fs::create_dir_all(&dir).unwrap();
    fx::write_structure_file(&dir.join("structures.arrow"), &[building_row(1)], false);
    let err = collect_sources_at_point(tmp.path(), LAT, LON).unwrap_err();
    assert!(err.contains("structures_contract mismatch"), "got: {err}");
}

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
            ring_lonlat: None,
            suppressed: false,
        }],
    );
    let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
    assert_eq!(data.industrial.len(), 1);
    assert_eq!(data.industrial[0].osm_id, 99);
}

#[test]
fn industrial_gate_is_the_polygon_edge_not_its_centroid() {
    // Garzweiler east end: the mine's centroid is 5.6 km away (past the old
    // 5 km centroid gate) but its boundary is 250 m off — the popup must see
    // it, as the painter always has. A near-centroid point row past 4 km is
    // correctly gone (the painter's per-point cap drops it too).
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = fx::square_dir(tmp.path(), prague());
    std::fs::create_dir_all(&dir).unwrap();
    let m_per_deg_lon = grid::geo::m_per_deg_lon(LAT.to_radians());
    let mine_centroid = (LON + 5600.0 / m_per_deg_lon, LAT);
    let mine_west_edge = LON + 250.0 / m_per_deg_lon;
    let mine_east_edge = mine_centroid.0 + (mine_centroid.0 - mine_west_edge);
    let half_height = 0.004;
    fx::write_industrial_file(
        &dir.join("industrial.arrow"),
        &[
            fx::FixtureIndustrial {
                osm_id: 100,
                centroid: mine_centroid,
                source_type: 0,
                name: "Mine".to_string(),
                suppressed: false,
                ring_lonlat: Some(vec![
                    (mine_west_edge, LAT - half_height),
                    (mine_east_edge, LAT - half_height),
                    (mine_east_edge, LAT + half_height),
                    (mine_west_edge, LAT + half_height),
                    (mine_west_edge, LAT - half_height),
                ]),
            },
            fx::FixtureIndustrial {
                osm_id: 101,
                centroid: (LON + 4500.0 / m_per_deg_lon, LAT),
                source_type: 0,
                name: "Far shed".to_string(),
                ring_lonlat: None,
                suppressed: false,
            },
            // A lifecycle-retired quarry at the receiver: silent either way.
            fx::FixtureIndustrial {
                osm_id: 102,
                centroid: (LON, LAT),
                source_type: 1,
                name: "Disused quarry".to_string(),
                ring_lonlat: None,
                suppressed: true,
            },
        ],
    );
    let centroid_dist =
        grid::geo::flat_dist(LAT, LON, mine_centroid.1, mine_centroid.0);
    assert!(centroid_dist > 5000.0, "centroid {centroid_dist:.0} m");
    let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
    let ids: Vec<i64> = data.industrial.iter().map(|p| p.osm_id).collect();
    assert!(ids.contains(&100), "the mine edge reaches: {ids:?}");
    assert!(!ids.contains(&101), "a 4.5 km point is past reach: {ids:?}");
    assert!(!ids.contains(&102), "a suppressed quarry stays silent: {ids:?}");
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

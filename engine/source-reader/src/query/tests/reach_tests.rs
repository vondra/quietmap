//! Reach tests.

use super::*;

#[test]
fn cruise_rows_are_read_from_neighbouring_owner_squares() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (owner_lat, owner_lon) = (50.0, 14.10);
    let owner = grid::square_of(owner_lat, owner_lon);
    let dir = fx::square_dir(tmp.path(), owner);
    std::fs::create_dir_all(&dir).unwrap();
    aircraft_extract::arrow_io::write_cruise(
        &dir.join("cruise.arrow"),
        &[aircraft_extract::flight::CruiseBucket {
            cruise_cell_id: grid::cruise::cruise_cell_id(owner_lat, owner_lon),
            class: 3,
            rep_profile_idx: 2,
            fl_bin: 4,
            period: 0,
            sum_length_m: 4000.0,
            heading_bin: 2,
            rep_alt_m: 11_000.0,
            rep_speed_kt: 460.0,
            unique_count: 1,
            top_candidates: Vec::new(),
            source_id: 2,
            origin: 0,
            secondary_only: false,
        }], &crate::structure_test_fixture::sampling_window(12, 0))
    .unwrap();
    let near = grid::square_of(50.0, 14.0);
    assert_ne!(near, owner);
    let collected = collect_sources_at_point(tmp.path(), 50.0, 14.0).unwrap();
    assert_eq!(
        collected
            .aircraft_cruise_batches
            .iter()
            .map(|b| b.num_rows())
            .sum::<usize>(),
        1
    );
    let far = collect_sources_at_point(tmp.path(), 50.0, 13.0).unwrap();
    assert!(far.aircraft_cruise_batches.is_empty());
    assert!(!squares_within_reach(50.0, 13.0).unwrap().contains(&owner));
}

#[test]
fn source_envelope_contains_own_square_and_high_latitude_rail() {
    let squares = squares_within_reach(LAT, LON).unwrap();
    assert!(squares.contains(&prague()));
    assert_eq!(prague(), grid::Square { x: 276, y: 173 });
    let receiver_lat = 81.82379430564337;
    let source_lat = 81.92318633602197;
    let tmp = tempfile::TempDir::new().unwrap();
    let source_square = grid::square_of(source_lat, 0.0);
    let dir = fx::square_dir(tmp.path(), source_square);
    std::fs::create_dir_all(&dir).unwrap();
    fx::write_railways_file(
        &dir.join("railways.arrow"),
        &[fx::FixtureRail {
            osm_id: 901,
            start: (0.0, source_lat),
            end: (0.001, source_lat),
            rail_type: 0,
            maxspeed: 160,
        }],
    );
    let collected = collect_sources_at_point(tmp.path(), receiver_lat, 0.0).unwrap();
    assert_eq!(collected.railways.len(), 1);
    assert_eq!(collected.railways[0].osm_id, 901);
    assert!(collected.railways[0].dist_m < noise_compute::constants::RAILWAY_REACH_CEILING);
}

#[test]
fn road_and_rail_radius_preserves_all_bearings_at_high_latitudes() {
    for lat in [0.0_f64, 50.0, 70.0, 82.0, -82.0] {
        for bearing in 0..8 {
            let angle = (f64::from(bearing) * 45.0).to_radians();
            let source_lat = lat + 990.0 * angle.sin() / grid::geo::M_PER_DEG_LAT;
            let source_lon =
                990.0 * angle.cos() / grid::geo::m_per_deg_lon(source_lat.to_radians());
            let tmp = tempfile::TempDir::new().unwrap();
            let dir = fx::square_dir(tmp.path(), grid::square_of(source_lat, source_lon));
            std::fs::create_dir_all(&dir).unwrap();
            fx::write_roads_file(
                &dir.join("roads.arrow"),
                &[fx::FixtureRoad {
                    osm_id: 1,
                    start: (source_lon, source_lat),
                    end: (source_lon + 0.00001, source_lat),
                    road_class: 0,
                    speed_limit: 100,
                    lanes: 2,
                    name: String::new(),
                    ..Default::default()
                }],
            );
            fx::write_railways_file(
                &dir.join("railways.arrow"),
                &[fx::FixtureRail {
                    osm_id: 2,
                    start: (source_lon, source_lat),
                    end: (source_lon + 0.00001, source_lat),
                    rail_type: 0,
                    maxspeed: 160,
                }],
            );
            let square = load_square(&dir).unwrap();
            let roads =
                query_roads_from_batches(&square.roads.batches_all().unwrap(), lat, 0.0, 1000.0)
                    .unwrap();
            let rails = query_railways_from_batches(
                &square.railways.batches_all().unwrap(),
                lat,
                0.0,
                1000.0,
            )
            .unwrap();
            assert_eq!(
                (roads.len(), rails.len()),
                (1, 1),
                "lat={lat} bearing={bearing}"
            );
        }
    }
}

#[test]
fn square_dir_layout_is_z9_x_y() {
    let year = std::path::Path::new("/prepared/2026");
    assert_eq!(
        square_dir(year, grid::Square { x: 276, y: 173 }),
        std::path::PathBuf::from("/prepared/2026/z9/276/173")
    );
}

#[test]
fn antimeridian_road_and_rail_survive_the_midpoint_prefilter() {
    let (lat, lon) = (0.0, -180.0);
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = fx::square_dir(tmp.path(), grid::square_of(lat, lon));
    std::fs::create_dir_all(&dir).unwrap();
    fx::write_roads_file(
        &dir.join("roads.arrow"),
        &[fx::FixtureRoad {
            osm_id: 1,
            start: (179.999, lat),
            end: (-179.999, lat),
            road_class: 2,
            speed_limit: 50,
            lanes: 2,
            name: "Dateline Road".to_string(),
            ..Default::default()
        }],
    );
    fx::write_railways_file(
        &dir.join("railways.arrow"),
        &[fx::FixtureRail {
            osm_id: 2,
            start: (179.999, lat),
            end: (-179.999, lat),
            rail_type: 0,
            maxspeed: 80,
        }],
    );

    let data = collect_sources_at_point(tmp.path(), lat, lon).unwrap();
    assert_eq!(data.roads.len(), 1);
    assert_eq!(data.railways.len(), 1);
}

#[test]
fn empty_tree_collects_nothing_and_no_sampling_window() {
    let tmp = tempfile::TempDir::new().unwrap();
    let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
    assert!(data.roads.is_empty());
    assert!(data.buildings.is_empty());
    assert_eq!(data.aircraft_sampling_window, None);
}

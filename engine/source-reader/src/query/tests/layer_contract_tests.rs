//! Layer contract tests.

use super::*;

#[test]
fn rail_row_collects_with_grid_geometry() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = fx::square_dir(tmp.path(), prague());
    std::fs::create_dir_all(&dir).unwrap();
    fx::write_railways_file(
        &dir.join("railways.arrow"),
        &[fx::FixtureRail {
            osm_id: 77,
            start: (LON, LAT),
            end: (LON + 0.002, LAT),
            rail_type: 0,
            maxspeed: 160,
        }],
    );
    let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
    assert_eq!(data.railways.len(), 1);
    assert_eq!(data.railways[0].osm_id, 77);
    assert_eq!(data.railways[0].maxspeed, 160);
}

#[test]
fn ship_cells_collect_with_cell_identity_and_reach() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = fx::square_dir(tmp.path(), prague());
    std::fs::create_dir_all(&dir).unwrap();
    let cells = [
        fx::FixtureShipCell {
            centroid: (LON + 0.01, LAT),
            hours: [60.0, 5.0, 0.0],
        },
        fx::FixtureShipCell {
            centroid: (LON, LAT + 0.005),
            hours: [0.0, 0.0, 0.0],
        },
        fx::FixtureShipCell {
            centroid: (LON + 0.2, LAT),
            hours: [500.0, 0.0, 0.0],
        },
    ];
    fx::write_ships_file(&dir.join("ships.arrow"), &cells, "ships_v1");
    let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
    let n = data.ships.len();
    assert!(
        (16..=25).contains(&n),
        "a 1 km cell grids into 4–5 sub-cells per axis, got {n}"
    );
    let (gx, gy) = fx::grid_of(LON + 0.01, LAT);
    let cell_id = (i64::from(gx) << 32) | i64::from(gy as u32);
    let energy: f64 = data
        .ships
        .iter()
        .map(|p| 10f64.powf(f64::from(p.lw_day[1]) / 10.0))
        .sum();
    let whole = noise_compute::emission::ships::ship_emission_bands(
        noise_compute::emission::ships::ship_cell_lw([60.0, 5.0, 0.0])
            .unwrap()
            .0,
    )[1];
    assert!(
        (10.0 * energy.log10() - whole).abs() < 0.05,
        "sub-cells conserve the cell energy"
    );
    for cell in &data.ships {
        assert_eq!(cell.osm_id, cell_id);
        assert_eq!(cell.source_type, 0, "large ships carry the energy");
        assert_eq!(cell.source_id, 9901);
        assert_eq!(cell.ship_hours, Some([60.0, 5.0, 0.0]));
        assert!(
            (80.0..=150.0).contains(&cell.exclusion_radius_m),
            "{}",
            cell.exclusion_radius_m
        );
        assert!(cell.max_radius_m <= noise_compute::emission::ships::SHIP_MAX_RADIUS_M);
        assert!(
            cell.dist_m > 200.0 && cell.dist_m < 1300.0,
            "{}",
            cell.dist_m
        );
    }

    let neighbour = squares_within_reach(LAT, LON)
        .unwrap()
        .into_iter()
        .find(|square| *square != prague())
        .expect("the reach spans a second owner square");
    let neighbour_dir = fx::square_dir(tmp.path(), neighbour);
    std::fs::create_dir_all(&neighbour_dir).unwrap();
    fx::write_ships_file(&neighbour_dir.join("ships.arrow"), &cells, "ships_v0");
    let served = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
    assert_eq!(served.unavailable_layers, ["ships"]);
    assert!(served.ships.is_empty());
}

#[test]
fn reference_square_gate_reads_rows_strictly() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = fx::square_dir(tmp.path(), prague());
    std::fs::create_dir_all(&dir).unwrap();
    fx::write_roads_file(
        &dir.join("roads.arrow"),
        &[fx::FixtureRoad {
            osm_id: 1,
            start: (LON, LAT),
            end: (LON + 0.001, LAT),
            road_class: 2,
            speed_limit: 50,
            lanes: 2,
            name: String::new(),
            ..Default::default()
        }],
    );
    let name = grid::square_name(prague());
    assert_eq!(
        square_store::store::validate_reference_square(tmp.path(), &name).unwrap(),
        1
    );
    assert!(square_store::store::validate_reference_square(tmp.path(), "z9/../escape").is_err());
}

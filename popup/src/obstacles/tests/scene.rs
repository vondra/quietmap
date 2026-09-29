//! Tiles and frames: outlines stored in two tiles are one footprint, the antimeridian is
//! continuous, an unread tile fails the query, and scene steps map to the click's metres.

use super::*;

/// Metres north of the click of the tile boundary north of it.
fn north_tile_edge_m(stock: &Stock) -> f64 {
    let origin = stock.frame.origin;
    let edge = Mercator {
        x: origin.x,
        y: origin.y.floor(),
    };
    stock.frame.to_metres(edge)[1]
}

#[test]
fn an_outline_across_a_tile_boundary_is_one_footprint_from_either_tile() {
    let mut stock = Stock::new();
    let edge = north_tile_edge_m(&stock);
    stock.building(42, 15.0, &[square(0.0, edge, 20.0)]);
    let south_of_edge = stock.steps([0.0, edge - 10.0]);
    let north_of_edge = stock.steps([0.0, edge + 10.0]);
    let tiles = tiles_crossed(&stock.outlines[0].vertices);
    assert_eq!(tiles.len(), 2, "stored whole in both tiles");
    assert_ne!(
        TileId::containing(south_of_edge.to_mercator()),
        TileId::containing(north_of_edge.to_mercator())
    );
    stock.with_scene(|scene| {
        // Rays wholly inside either tile, and across the boundary: entry and exit once each.
        for (from, to) in [
            ([-100.0, edge - 10.0], [100.0, edge - 10.0]),
            ([-100.0, edge + 10.0], [100.0, edge + 10.0]),
            ([0.0, edge - 100.0], [0.0, edge + 100.0]),
            ([-50.0, edge - 60.0], [50.0, edge + 60.0]),
        ] {
            let found = crossings_of(scene, from, to);
            assert_eq!(found.len(), 2, "{found:?}");
            assert!(
                found
                    .iter()
                    .all(|c| c.footprint_id == 42 && c.building && c.height_m == 15.0)
            );
            assert_eq!(found, every_crossing(scene, from, to));
        }
        for probe in [[0.0, edge - 10.0], [0.0, edge + 10.0], [15.0, edge]] {
            let building = scene.enclosing_building(probe).unwrap().expect("inside");
            assert_eq!((building.id, building.rings.len()), (42, 1));
        }
        assert_eq!(scene.enclosing_building([0.0, edge + 30.0]).unwrap(), None);
    });
}

#[test]
fn a_roof_across_two_tiles_pairs_its_entry_and_exit_under_one_id() {
    let mut stock = Stock::new();
    let edge = north_tile_edge_m(&stock);
    stock.building(7, 12.0, &[rectangle(-10.0, edge - 40.0, 10.0, edge + 40.0)]);
    stock.building(8, 9.0, &[rectangle(-10.0, edge + 60.0, 10.0, edge + 70.0)]);
    let found =
        stock.with_scene(|scene| crossings_of(scene, [0.0, edge - 100.0], [0.0, edge + 100.0]));
    let ids: Vec<u64> = found.iter().map(|c| c.footprint_id).collect();
    assert_eq!(ids, vec![7, 7, 8, 8], "{found:?}");
    assert!(found.windows(2).all(|pair| pair[0].t < pair[1].t));
}

#[test]
fn a_footprint_whose_parts_lie_in_two_tiles_keeps_every_part() {
    let mut stock = Stock::new();
    let edge = north_tile_edge_m(&stock);
    stock.building(9, 10.0, &[rectangle(0.0, edge - 30.0, 10.0, edge - 20.0)]);
    stock.building(9, 10.0, &[rectangle(0.0, edge + 20.0, 10.0, edge + 30.0)]);
    for part in &stock.outlines {
        assert_eq!(
            tiles_crossed(&part.vertices).len(),
            1,
            "each part in one tile only"
        );
    }
    stock.with_scene(|scene| {
        let south = scene
            .enclosing_building([5.0, edge - 25.0])
            .unwrap()
            .unwrap();
        let north = scene
            .enclosing_building([5.0, edge + 25.0])
            .unwrap()
            .unwrap();
        assert_eq!(south, north, "one footprint, whichever part is clicked");
        assert_eq!(south.rings.len(), 2);
        assert!(south.rings.iter().all(|ring| !ring.hole));
        assert_eq!(scene.facade_receivers(&south).unwrap().len(), 2 * 8);
    });
}

#[test]
fn the_antimeridian_is_continuous() {
    let mut stock = Stock::at(10.0, 179.998);
    let dateline = stock.frame.to_metres(Mercator::from_degrees(10.0, 180.0))[0];
    stock.building(3, 10.0, &[square(dateline, 0.0, 30.0)]);
    stock.wall(
        4,
        4.0,
        &[[dateline + 100.0, -50.0], [dateline + 100.0, 50.0]],
    );
    let tiles = tiles_crossed(&stock.outlines[0].vertices);
    assert_eq!(
        tiles.iter().map(|tile| tile.x).collect::<Vec<_>>(),
        vec![0, TILES_PER_AXIS - 1]
    );
    stock.with_scene(|scene| {
        let found = crossings_of(scene, [0.0, 0.0], [dateline + 200.0, 0.0]);
        let ids: Vec<u64> = found.iter().map(|c| c.footprint_id).collect();
        assert_eq!(ids, vec![3, 3, 4]);
        let length = dateline + 200.0;
        assert!((found[0].t * length - (dateline - 30.0)).abs() < 0.2);
        assert!((found[1].t * length - (dateline + 30.0)).abs() < 0.2);
        for probe in [[dateline - 20.0, 0.0], [dateline + 20.0, 0.0]] {
            assert_eq!(
                scene.enclosing_building(probe).unwrap().map(|b| b.id),
                Some(3)
            );
        }
        let footprint = scene.enclosing_building([dateline, 0.0]).unwrap().unwrap();
        let xs = footprint.rings[0].points.iter().map(|p| p[0]);
        let (west, east) = xs.fold((f64::MAX, f64::MIN), |(w, e), x| (w.min(x), e.max(x)));
        assert!(
            (east - west - 60.0).abs() < 0.5,
            "one piece across E180: {west} .. {east}"
        );
        assert_eq!(scene.facade_receivers(&footprint).unwrap().len(), 4 * 12);
    });
}

#[test]
fn a_query_needing_an_unread_tile_fails_and_the_skyline_skips_it() {
    let mut stock = Stock::new();
    stock.building(1, 10.0, &[square(100.0, 0.0, 10.0)]);
    let files = stock.files();
    let centre = TileId::containing(stock.frame.origin);
    let mut scene = Scene::new(stock.frame);
    for (tile, bytes) in &files {
        match bytes {
            Some(bytes) if *tile == centre => scene.insert(*tile, Obstacles::parse(bytes).unwrap()),
            _ if *tile == centre => scene.insert_empty(*tile),
            _ => {}
        }
    }
    let mut out = Vec::new();
    assert!(scene.crossings([0.0, 0.0], [200.0, 0.0], &mut out).is_ok());
    assert_eq!(out.len(), 2);
    let error = scene
        .crossings([0.0, 0.0], [0.0, 20_000.0], &mut out)
        .unwrap_err();
    assert!(error.contains("not read"), "{error}");
    assert!(scene.reflection_db([0.0, 19_000.0], None).is_err());
    let mut arcs = Vec::new();
    scene.skyline_arcs([0.0, 0.0], -0.5, 0.5, 20_000.0, 0.0, &mut |arc| {
        arcs.push([arc.lo_rad, arc.hi_rad, arc.nearest_m])
    });
    arcs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    arcs.dedup();
    assert_eq!(arcs.len(), 4, "the read tile's four edges");
}

#[test]
fn scene_steps_are_the_frames_metres() {
    for (lat, lon) in [
        (50.0, 14.0),
        (-33.9, 151.2),
        (10.0, 179.999),
        (10.0, -179.999),
    ] {
        let stock = Stock::at(lat, lon);
        stock.with_scene(|scene| {
            for metres in [
                [0.0, 0.0],
                [1234.5, -987.25],
                [-9_000.0, 6_000.0],
                [300.0, 300.0],
            ] {
                let global = stock.steps(metres);
                let exact = stock
                    .frame
                    .metres_of_steps([global.x as f64, global.y as f64]);
                let scene_metres = scene.lattice.metres(scene_steps(scene, global));
                assert!(
                    (scene_metres[0] - exact[0]).abs() < 1e-6
                        && (scene_metres[1] - exact[1]).abs() < 1e-6
                );
                let back = scene.lattice.steps(scene_metres);
                let steps = scene_steps(scene, global);
                assert!((back[0] - steps[0]).abs() < 1e-6 && (back[1] - steps[1]).abs() < 1e-6);
            }
        });
    }
}

//! Set regression tests.

use super::*;

#[test]
fn max_height_crossed_reads_exact_crossings() {
    // East ray: 12 m block at 300 m, 25 m block at 900 m, a 30 m wall
    // across the ray at 1200 m, 6 m block at 1500 m. West ray: 8 m block
    // at 600 m; the 20 m wall to the north stands aside. North ray: only
    // that wall. South ray: clear.
    let rings = [
        (square(300.0, 0.0, 60.0), 12.0, ObstacleKind::Building),
        (square(900.0, 0.0, 60.0), 25.0, ObstacleKind::Building),
        (square(1200.0, 0.0, 10.0), 30.0, ObstacleKind::Barrier),
        (square(1500.0, 0.0, 60.0), 6.0, ObstacleKind::Building),
        (square(-600.0, 0.0, 60.0), 8.0, ObstacleKind::Building),
        (square(0.0, 400.0, 60.0), 20.0, ObstacleKind::Barrier),
    ];
    let index_of = |members: &[usize]| {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        for &i in members {
            let (ring, height_m, kind) = &rings[i];
            b.add_ring(ring, *height_m, *kind, i as u32);
        }
        std::sync::Arc::new(b.build())
    };
    // One index holding everything, and the same stock split so the east
    // ray's tallest block and the shorter one behind it sit in different
    // indexes (a footprint lives in exactly one).
    let sets = [
        ObstacleSet {
            indexes: vec![index_of(&[0, 1, 2, 3, 4, 5])],
        },
        ObstacleSet {
            indexes: vec![index_of(&[0, 1, 5]), index_of(&[2, 3, 4])],
        },
    ];
    let mut scratch = Vec::new();
    for set in &sets {
        // The gated walk must agree with the tallest building among the
        // unpruned crossings: a skipped cell never holds the answer.
        let mut probe = |rcv_lat: f64, rcv_lon: f64| {
            let h = set.max_height_crossed(OLAT, OLON, rcv_lat, rcv_lon);
            set.crossings(OLAT, OLON, rcv_lat, rcv_lon, &mut scratch);
            let unpruned = scratch
                .iter()
                .filter(|c| c.kind == ObstacleKind::Building)
                .map(|c| c.height_m as f64)
                .fold(0.0, f64::max);
            assert_eq!(h, unpruned, "gated walk vs every crossing");
            h
        };
        assert_eq!(probe(OLAT, OLON + 0.03), 25.0);
        assert_eq!(probe(OLAT, OLON - 0.02), 8.0);
        assert_eq!(probe(OLAT + 0.02, OLON), 0.0);
        assert_eq!(probe(OLAT - 0.02, OLON), 0.0);
    }
}

#[test]
fn dateline_sector_scan_uses_the_short_receiver_frame() {
    const SECTORS: usize = 8;
    const RECEIVER_LON: f64 = 179.999;
    let m_lon = m_per_deg_lon(0.0);
    let at_receiver_offset = |east_m: f64, north_m: f64| {
        (
            north_m / M_PER_DEG_LAT,
            grid::geo::normalize_longitude(RECEIVER_LON + east_m / m_lon),
        )
    };

    // The box sits 180-230 m east of a receiver on the other canonical
    // side of E180. Sector 0 is centred at 22.5 degrees and crosses both
    // its near and far faces.
    let mut builder = ObstacleIndex::builder(0.0, -179.7);
    builder.add_ring(
        &[
            at_receiver_offset(180.0, 65.0),
            at_receiver_offset(230.0, 65.0),
            at_receiver_offset(230.0, 105.0),
            at_receiver_offset(180.0, 105.0),
        ],
        10.0,
        ObstacleKind::Building,
        0,
    );
    let set = ObstacleSet {
        indexes: vec![std::sync::Arc::new(builder.build())],
    };
    let directions: [(f64, f64); SECTORS] = std::array::from_fn(|sector| {
        let angle = (sector as f64 + 0.5) * std::f64::consts::TAU / SECTORS as f64;
        (angle.sin(), angle.cos())
    });
    let mut scratch = CrossingScratch::default();
    let mut hits = Vec::new();
    set.visit_building_sector_crossings(
        0.0,
        RECEIVER_LON,
        M_PER_DEG_LAT,
        m_lon,
        500.0,
        &directions,
        &mut scratch,
        &mut |sector, range_m, height_m| hits.push((sector, range_m, height_m)),
    );

    let sector_zero: Vec<_> = hits.iter().filter(|(sector, _, _)| *sector == 0).collect();
    assert_eq!(sector_zero.len(), 2, "sector hits: {hits:?}");
    assert!(
        sector_zero
            .iter()
            .all(|(_, range_m, height_m)| (190.0..255.0).contains(range_m) && *height_m == 10.0),
        "sector hits: {hits:?}"
    );
}

/// Two indexes on different origins whose footprints reuse id 0: the merged index crosses the
/// same walls at the same chainages, and the two footprints keep distinct ids.
#[test]
fn a_merged_set_crosses_the_same_walls_with_set_unique_ids() {
    let mut west = ObstacleIndex::builder(OLAT, OLON);
    west.add_ring(&square(0.0, 0.0, 10.0), 12.0, ObstacleKind::Building, 0);
    west.add_polyline(&[ll(40.0, -20.0), ll(40.0, 20.0)], 3.0, ObstacleKind::Barrier, 1);
    let east_origin = ll(500.0, 0.0);
    let mut east = ObstacleIndex::builder(east_origin.0, east_origin.1);
    east.add_ring(&square(300.0, 0.0, 15.0), 20.0, ObstacleKind::Building, 0);
    let set = ObstacleSet { indexes: vec![std::sync::Arc::new(west.build()), std::sync::Arc::new(east.build())] };
    let merged = set.merged(OLAT, OLON + 0.001);
    let (from, to) = (ll(-100.0, 1.0), ll(400.0, 2.0));
    let mut expected = Vec::new();
    set.crossings(from.0, from.1, to.0, to.1, &mut expected);
    let got = run(&merged, from, to);
    assert_eq!(got.len(), expected.len());
    assert_eq!(got.len(), 5);
    for (g, e) in got.iter().zip(&expected) {
        assert!((g.t - e.t).abs() * 500.0 < 1e-3, "{} vs {}", g.t, e.t);
        assert_eq!((g.kind, g.height_m), (e.kind, e.height_m));
    }
    let ids: Vec<u32> = got.iter().map(|c| c.id).collect();
    assert_eq!(ids[0], ids[1]);
    assert_eq!(ids[3], ids[4]);
    assert_ne!(ids[0], ids[3]);
}

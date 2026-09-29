//! Containment regression tests.

use super::*;

#[test]
fn contains_and_enclosure_thresholds() {
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    b.add_ring(&square(0.0, 0.0, 60.0), 12.0, ObstacleKind::Building, 1);
    let idx = b.build();
    let mut sc = Vec::new();
    assert!(
        idx.contains_built(OLAT, OLON, 5.0, &mut sc),
        "centre is inside"
    );
    let (out_lat, out_lon) = ll(300.0, 0.0);
    assert!(
        !idx.contains_built(out_lat, out_lon, 5.0, &mut sc),
        "outside"
    );
    assert!(
        !idx.contains_built(OLAT, OLON, 20.0, &mut sc),
        "min-height gate must exclude the 12 m footprint"
    );

    let set = ObstacleSet {
        indexes: vec![std::sync::Arc::new(idx)],
    };
    // 60 m half-size square vs 75 m probes: only the centre probe is
    // inside → density 1/9 → 0 dB.
    assert_eq!(enclosure_db(&set, OLAT, OLON, 75.0, None), 0.0);

    // A 200 m half-size block swallows all 9 probes → 3 dB.
    let mut b2 = ObstacleIndex::builder(OLAT, OLON);
    b2.add_ring(&square(0.0, 0.0, 200.0), 12.0, ObstacleKind::Building, 1);
    let set2 = ObstacleSet {
        indexes: vec![std::sync::Arc::new(b2.build())],
    };
    assert_eq!(enclosure_db(&set2, OLAT, OLON, 75.0, None), 3.0);
}

#[test]
fn overlapping_footprints_contain_correctly() {
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    b.add_ring(&square(0.0, 0.0, 50.0), 12.0, ObstacleKind::Building, 1);
    b.add_ring(&square(20.0, 0.0, 50.0), 15.0, ObstacleKind::Building, 2);
    let idx = b.build();
    let mut sc = Vec::new();
    assert!(idx.contains_built(OLAT, OLON, 5.0, &mut sc), "inside both");
    let (lat_e, lon_e) = ll(60.0, 0.0);
    assert!(
        idx.contains_built(lat_e, lon_e, 5.0, &mut sc),
        "inside #2 only"
    );
    let (lat_o, lon_o) = ll(200.0, 0.0);
    assert!(
        !idx.contains_built(lat_o, lon_o, 5.0, &mut sc),
        "outside both"
    );
}

#[test]
fn parity_ray_through_vertices_is_consistent() {
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    b.add_ring(&square(100.0, 0.0, 40.0), 10.0, ObstacleKind::Building, 1);
    let idx = b.build();
    let mut sc = Vec::new();
    // Probe WEST of the square at exactly the corner row (y = -40 m is a
    // vertex latitude): the horizontal parity ray grazes both corners.
    // OUTSIDE must stay outside despite the graze; a mid-edge-row probe
    // west of the square is outside too; INSIDE stays inside.
    let (corner_lat, west_lon) = (ll(0.0, -40.0).0, ll(-200.0, 0.0).1);
    assert!(!idx.contains_built(corner_lat, west_lon, 5.0, &mut sc));
    let (mid_lat, _unused) = ll(0.0, 0.0);
    assert!(!idx.contains_built(mid_lat, west_lon, 5.0, &mut sc));
    let (in_lat, in_lon) = (ll(100.0, -39.9).0, ll(100.0, 0.0).1);
    assert!(idx.contains_built(in_lat, in_lon, 5.0, &mut sc));
}

#[test]
fn far_footprint_does_not_phantom_capture() {
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    b.add_ring(&square(0.0, 0.0, 20.0), 10.0, ObstacleKind::Building, 0);
    // 800 m wide block whose interior would swallow a 2 km ray end.
    b.add_ring(&square(1800.0, 0.0, 400.0), 10.0, ObstacleKind::Building, 1);
    let idx = b.build();
    let mut sc = Vec::new();
    let (plat, plon) = ll(100.0, 0.0); // between the two, inside neither
    assert!(!idx.contains_built(plat, plon, 5.0, &mut sc));
    let (ilat, ilon) = ll(1800.0, 0.0);
    assert!(idx.contains_built(ilat, ilon, 5.0, &mut sc));
}

#[test]
fn oversized_footprint_still_contained() {
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    b.add_ring(&square(0.0, 0.0, 1500.0), 10.0, ObstacleKind::Building, 0);
    let idx = b.build();
    let mut sc = Vec::new();
    let (plat, plon) = ll(-1400.0, 0.0); // 2.9 km from the east wall
    assert!(idx.contains_built(plat, plon, 5.0, &mut sc));
    let (olat, olon) = ll(-1600.0, 0.0);
    assert!(!idx.contains_built(olat, olon, 5.0, &mut sc));
}

#[test]
fn courtyard_reads_outside_annulus_inside() {
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    b.add_ring(&square(0.0, 0.0, 50.0), 12.0, ObstacleKind::Building, 0);
    b.add_ring(&square(0.0, 0.0, 20.0), 12.0, ObstacleKind::Building, 0);
    let idx = b.build();
    let mut sc = Vec::new();
    assert!(
        !idx.contains_built(OLAT, OLON, 5.0, &mut sc),
        "courtyard centre"
    );
    let (alat, alon) = ll(35.0, 0.0);
    assert!(
        idx.contains_built(alat, alon, 5.0, &mut sc),
        "annulus between hole and outer wall"
    );
    assert!(
        idx.containing_footprint(OLAT, OLON, 5.0, &mut sc).is_none(),
        "hover must also leave the courtyard outdoors"
    );
    assert!(
        idx.containing_footprint(alat, alon, 5.0, &mut sc).is_some(),
        "hover must retain the annulus building"
    );
}

#[test]
fn tangent_vertex_keeps_parity() {
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    let tri = vec![ll(0.0, 0.0), ll(30.0, 40.0), ll(-30.0, 40.0)];
    b.add_ring(&tri, 10.0, ObstacleKind::Building, 0);
    let idx = b.build();
    let mut sc = Vec::new();
    // Probe west of the apex, ON the apex row: both slanted edges cross
    // the row AT the apex — two counts (even), outside. One count would
    // report phantom containment all the way west.
    let (alat, wlon) = (ll(0.0, 0.0).0, ll(-200.0, 0.0).1);
    assert!(!idx.contains_built(alat, wlon, 5.0, &mut sc));
    let (ilat, ilon) = ll(0.0, 20.0);
    assert!(idx.contains_built(ilat, ilon, 5.0, &mut sc), "interior");
}

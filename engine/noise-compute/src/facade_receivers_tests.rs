//! §2.8 façade receiver placement: spacing, offsets, party walls and courtyards.

use super::*;
use crate::propagation::obstacle_index::{ObstacleIndex, ObstacleKind};

const LAT: f64 = 50.0;
const LON: f64 = 14.4;
/// Ground metres per degree on the Web Mercator sphere, the grid's own metric.
const M_PER_DEGREE: f64 = grid::WEB_MERCATOR_RADIUS_M * std::f64::consts::PI / 180.0;

fn m_per_deg_lon() -> f64 {
    M_PER_DEGREE * LAT.to_radians().cos()
}

fn lat_lon(x_m: f64, y_m: f64) -> (f64, f64) {
    (
        LAT + y_m / M_PER_DEGREE,
        LON + x_m / m_per_deg_lon(),
    )
}

fn grid_ring(corners_m: &[(f64, f64)]) -> Vec<(i32, i32)> {
    corners_m
        .iter()
        .map(|&(x, y)| {
            let (lat, lon) = lat_lon(x, y);
            grid::lonlat_to_grid(lon, lat)
        })
        .collect()
}

fn rectangle(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<(f64, f64)> {
    vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
}

fn metres(receiver: &FacadeReceiverPosition) -> (f64, f64) {
    let (lat, lon) = receiver.latitude_longitude();
    (
        (lon - LON) * m_per_deg_lon(),
        (lat - LAT) * M_PER_DEGREE,
    )
}

fn close(a: (f64, f64), b: (f64, f64)) -> bool {
    (a.0 - b.0).hypot(a.1 - b.1) < 0.06
}

fn obstacles(footprints: &[Vec<Vec<(f64, f64)>>]) -> ObstacleSet {
    let mut builder = ObstacleIndex::builder(LAT, LON);
    for (id, rings) in footprints.iter().enumerate() {
        for ring in rings {
            let ring: Vec<_> = ring.iter().map(|&(x, y)| lat_lon(x, y)).collect();
            builder.add_ring(&ring, 10.0, ObstacleKind::Building, id as u32);
        }
    }
    ObstacleSet {
        indexes: vec![std::sync::Arc::new(builder.build())],
    }
}

#[test]
fn a_ten_metre_house_gets_two_receivers_per_facade_two_and_a_half_metres_from_the_corners() {
    let receivers = facade_receiver_positions(&vec![vec![grid_ring(&rectangle(0.0, 0.0, 10.0, 10.0))]]);
    assert_eq!(receivers.len(), 8);
    for expected in [
        (2.5, -0.1),
        (7.5, -0.1),
        (10.1, 2.5),
        (10.1, 7.5),
        (7.5, 10.1),
        (2.5, 10.1),
        (-0.1, 7.5),
        (-0.1, 2.5),
    ] {
        assert!(
            receivers.iter().any(|r| close(metres(r), expected)),
            "no receiver at {expected:?}: {:?}",
            receivers.iter().map(metres).collect::<Vec<_>>()
        );
    }
    let south = receivers.iter().find(|r| close(metres(r), (2.5, -0.1))).unwrap();
    assert!((south.outward_bearing_deg - 180.0).abs() < 0.1);
    let east = receivers.iter().find(|r| close(metres(r), (10.1, 2.5))).unwrap();
    assert!((east.outward_bearing_deg - 90.0).abs() < 0.1);
}

#[test]
fn a_twelve_metre_facade_gets_three_receivers_four_metres_apart_and_a_three_metre_one_gets_one() {
    let receivers = facade_receiver_positions(&vec![vec![grid_ring(&rectangle(0.0, 0.0, 12.0, 3.0))]]);
    assert_eq!(receivers.len(), 3 + 1 + 3 + 1);
    for x in [2.0, 6.0, 10.0] {
        assert!(receivers.iter().any(|r| close(metres(r), (x, -0.1))));
    }
    assert!(receivers.iter().any(|r| close(metres(r), (12.1, 1.5))));
}

#[test]
fn a_lone_two_metre_facade_gets_none_and_short_runs_join_into_a_polyline() {
    let receivers = facade_receiver_positions(&vec![vec![grid_ring(&rectangle(0.0, 0.0, 12.0, 2.0))]]);
    assert_eq!(receivers.len(), 6, "the two 2 m ends carry no receiver");
    // A 6 m façade drawn as three 2 m pieces is one 6 m polyline: two receivers.
    let stepped = vec![
        (0.0, 0.0),
        (2.0, 0.0),
        (4.0, 0.0),
        (6.0, 0.0),
        (6.0, 20.0),
        (0.0, 20.0),
    ];
    let receivers = facade_receiver_positions(&vec![vec![grid_ring(&stepped)]]);
    let on_south: Vec<_> = receivers.iter().map(metres).filter(|p| p.1 < 0.0).collect();
    assert_eq!(on_south.len(), 2, "{on_south:?}");
    assert!(on_south.iter().any(|&p| close(p, (1.5, -0.1))));
    assert!(on_south.iter().any(|&p| close(p, (4.5, -0.1))));
}

#[test]
fn a_footprint_without_a_qualifying_segment_keeps_one_receiver_at_its_longest_edge() {
    // A 7 m perimeter of short edges is one polyline over 5 m: two receivers.
    let shed = facade_receiver_positions(&vec![vec![grid_ring(&rectangle(0.0, 0.0, 2.0, 1.5))]]);
    assert_eq!(shed.len(), 2);
    // A 5 m perimeter has no qualifying segment: one receiver mid longest edge.
    let kiosk = facade_receiver_positions(&vec![vec![grid_ring(&rectangle(0.0, 0.0, 1.5, 1.0))]]);
    assert_eq!(kiosk.len(), 1);
    let (x, y) = metres(&kiosk[0]);
    assert!((x - 0.75).abs() < 0.06 && ((y + 0.1).abs() < 0.06 || (y - 1.1).abs() < 0.06));
}

#[test]
fn terraced_houses_have_no_receiver_on_their_shared_wall() {
    let west = vec![rectangle(0.0, 0.0, 10.0, 10.0)];
    let east = vec![rectangle(10.0, 0.0, 20.0, 10.0)];
    let set = obstacles(&[west.clone(), east]);
    let receivers = exposed_facade_receivers(&vec![vec![grid_ring(&west[0])]], &set);
    assert_eq!(receivers.len(), 6);
    assert!(receivers.iter().all(|r| metres(r).0 < 10.0));
}

#[test]
fn courtyard_receivers_stand_in_the_courtyard_facing_into_it() {
    let outer = rectangle(0.0, 0.0, 30.0, 30.0);
    let hole = rectangle(10.0, 10.0, 20.0, 20.0);
    let footprint = vec![vec![grid_ring(&outer), grid_ring(&hole)]];
    let set = obstacles(&[vec![outer, hole]]);
    let receivers = exposed_facade_receivers(&footprint, &set);
    assert_eq!(receivers.len(), 4 * 6 + 4 * 2);
    let inner: Vec<_> = receivers
        .iter()
        .filter(|r| {
            let (x, y) = metres(r);
            (9.0..21.0).contains(&x) && (9.0..21.0).contains(&y)
        })
        .collect();
    assert_eq!(inner.len(), 8);
    for receiver in inner {
        let (x, y) = metres(receiver);
        assert!((10.0..=20.0).contains(&x) && (10.0..=20.0).contains(&y), "{x} {y}");
        let bearing = f64::from(receiver.outward_bearing_deg).to_radians();
        let towards_centre = (15.0 - x) * bearing.sin() + (15.0 - y) * bearing.cos();
        assert!(towards_centre > 0.0, "courtyard façade must face the courtyard");
    }
}

#[test]
fn a_large_hall_gets_receivers_along_its_whole_perimeter_and_none_inside() {
    let hall = rectangle(0.0, 0.0, 310.0, 309.0);
    let set = obstacles(&[vec![hall.clone()]]);
    let receivers = exposed_facade_receivers(&vec![vec![grid_ring(&hall)]], &set);
    assert_eq!(receivers.len(), 62 * 4);
    assert!(receivers.iter().all(|r| {
        let (x, y) = metres(r);
        !(0.0..=310.0).contains(&x) || !(0.0..=309.0).contains(&y)
    }));
}

#[test]
fn the_canonical_order_does_not_depend_on_the_stored_start_vertex_or_winding() {
    let ring = rectangle(0.0, 0.0, 10.0, 7.0);
    let reference = facade_receiver_positions(&vec![vec![grid_ring(&ring)]]);
    let mut rotated = ring.clone();
    rotated.rotate_left(2);
    rotated.reverse();
    assert_eq!(facade_receiver_positions(&vec![vec![grid_ring(&rotated)]]), reference);
}

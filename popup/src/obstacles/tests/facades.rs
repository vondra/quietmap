//! Façade receivers of CNOSSOS 2.8 case 1 (dev4 `facade_receivers_tests.rs`): spacing, offsets,
//! party walls and courtyards. Positions are compared within one int16 step, the storage quantum.

use super::super::facades::facade_receiver_positions;
use super::*;

/// The stored footprint around `inside`, and the receivers before and after the party-wall test.
fn receivers(stock: &Stock, inside: [f64; 2]) -> (Vec<FacadeReceiver>, Vec<FacadeReceiver>) {
    stock.with_scene(|scene| {
        let footprint = scene
            .enclosing_building(inside)
            .unwrap()
            .expect("inside a building");
        let resolution_m = scene.lattice.metres_per_step[0].max(scene.lattice.metres_per_step[1]);
        let all = facade_receiver_positions(&footprint, resolution_m);
        (all, scene.facade_receivers(&footprint).unwrap())
    })
}

fn close(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).hypot(a[1] - b[1]) < 0.2
}

fn find(receivers: &[FacadeReceiver], at: [f64; 2]) -> Option<&FacadeReceiver> {
    receivers
        .iter()
        .find(|receiver| close(receiver.position, at))
}

#[test]
fn a_ten_metre_house_gets_two_receivers_per_facade_two_and_a_half_metres_from_the_corners() {
    let mut stock = Stock::new();
    stock.building(1, 10.0, &[rectangle(0.0, 0.0, 10.0, 10.0)]);
    let (all, _) = receivers(&stock, [5.0, 5.0]);
    assert_eq!(all.len(), 8);
    for expected in [
        [2.5, -0.1],
        [7.5, -0.1],
        [10.1, 2.5],
        [10.1, 7.5],
        [7.5, 10.1],
        [2.5, 10.1],
        [-0.1, 7.5],
        [-0.1, 2.5],
    ] {
        assert!(
            find(&all, expected).is_some(),
            "no receiver at {expected:?}: {all:?}"
        );
    }
    assert!((find(&all, [2.5, -0.1]).unwrap().outward_bearing_deg - 180.0).abs() < 0.1);
    assert!((find(&all, [10.1, 2.5]).unwrap().outward_bearing_deg - 90.0).abs() < 0.1);
}

#[test]
fn a_twelve_metre_facade_gets_three_receivers_four_metres_apart_and_a_three_metre_one_gets_one() {
    let mut stock = Stock::new();
    stock.building(1, 10.0, &[rectangle(0.0, 0.0, 12.0, 3.0)]);
    let (all, _) = receivers(&stock, [6.0, 1.5]);
    assert_eq!(all.len(), 3 + 1 + 3 + 1);
    for x in [2.0, 6.0, 10.0] {
        assert!(find(&all, [x, -0.1]).is_some());
    }
    assert!(find(&all, [12.1, 1.5]).is_some());
}

#[test]
fn a_lone_two_metre_facade_gets_none_and_short_runs_join_into_a_polyline() {
    let mut stock = Stock::new();
    stock.building(1, 10.0, &[rectangle(0.0, 0.0, 12.0, 2.0)]);
    assert_eq!(
        receivers(&stock, [6.0, 1.0]).0.len(),
        6,
        "the two 2 m ends carry none"
    );
    // A 6 m façade drawn as three 2 m pieces is one 6 m polyline: two receivers.
    let mut stepped = Stock::new();
    let outline = vec![
        [0.0, 0.0],
        [2.0, 0.0],
        [4.0, 0.0],
        [6.0, 0.0],
        [6.0, 20.0],
        [0.0, 20.0],
    ];
    stepped.building(1, 10.0, &[outline]);
    let (all, _) = receivers(&stepped, [3.0, 10.0]);
    let south: Vec<_> = all.iter().filter(|r| r.position[1] < 0.0).collect();
    assert_eq!(south.len(), 2, "{south:?}");
    assert!(find(&all, [1.5, -0.1]).is_some() && find(&all, [4.5, -0.1]).is_some());
}

#[test]
fn a_footprint_without_a_qualifying_segment_keeps_one_receiver_at_its_longest_edge() {
    // A 7 m perimeter of short edges is one polyline over 5 m: two receivers.
    let mut shed = Stock::new();
    shed.building(1, 3.0, &[rectangle(0.0, 0.0, 2.0, 1.5)]);
    assert_eq!(receivers(&shed, [1.0, 0.75]).0.len(), 2);
    // A 5 m perimeter has no qualifying segment: one receiver mid its longest edge.
    let mut kiosk = Stock::new();
    kiosk.building(1, 3.0, &[rectangle(0.0, 0.0, 1.5, 1.0)]);
    let (all, _) = receivers(&kiosk, [0.75, 0.5]);
    assert_eq!(all.len(), 1);
    let [x, y] = all[0].position;
    assert!((x - 0.75).abs() < 0.2 && ((y + 0.1).abs() < 0.2 || (y - 1.1).abs() < 0.2));
}

#[test]
fn terraced_houses_have_no_receiver_on_their_shared_wall() {
    let mut stock = Stock::new();
    stock.building(1, 10.0, &[rectangle(0.0, 0.0, 10.0, 10.0)]);
    stock.building(2, 10.0, &[rectangle(10.0, 0.0, 20.0, 10.0)]);
    let (_, exposed) = receivers(&stock, [5.0, 5.0]);
    assert_eq!(exposed.len(), 6);
    assert!(exposed.iter().all(|r| r.position[0] < 10.0));
}

#[test]
fn courtyard_receivers_stand_in_the_courtyard_facing_into_it() {
    let mut stock = Stock::new();
    let (outer, hole) = (
        rectangle(0.0, 0.0, 30.0, 30.0),
        rectangle(10.0, 10.0, 20.0, 20.0),
    );
    stock.building(1, 10.0, &[outer, hole]);
    let (_, exposed) = receivers(&stock, [5.0, 5.0]);
    assert_eq!(exposed.len(), 4 * 6 + 4 * 2);
    let inner: Vec<_> = exposed
        .iter()
        .filter(|r| (9.0..21.0).contains(&r.position[0]) && (9.0..21.0).contains(&r.position[1]))
        .collect();
    assert_eq!(inner.len(), 8);
    for receiver in inner {
        let [x, y] = receiver.position;
        assert!(
            (10.0..=20.0).contains(&x) && (10.0..=20.0).contains(&y),
            "{x} {y}"
        );
        let bearing = receiver.outward_bearing_deg.to_radians();
        let towards_centre = (15.0 - x) * bearing.sin() + (15.0 - y) * bearing.cos();
        assert!(
            towards_centre > 0.0,
            "a courtyard façade faces the courtyard"
        );
    }
}

#[test]
fn a_large_hall_gets_receivers_along_its_whole_perimeter_and_none_inside() {
    let mut stock = Stock::new();
    stock.building(1, 10.0, &[rectangle(0.0, 0.0, 310.0, 309.0)]);
    let (_, exposed) = receivers(&stock, [100.0, 100.0]);
    assert_eq!(exposed.len(), 62 * 4);
    assert!(exposed.iter().all(|r| {
        let [x, y] = r.position;
        !(0.0..=310.0).contains(&x) || !(0.0..=309.0).contains(&y)
    }));
}

#[test]
fn the_canonical_order_does_not_depend_on_the_stored_start_vertex_or_winding() {
    let ring = rectangle(0.0, 0.0, 10.0, 7.0);
    let mut rotated = ring.clone();
    rotated.rotate_left(2);
    rotated.reverse();
    let placed = |ring: Vec<[f64; 2]>| {
        let mut stock = Stock::new();
        stock.building(1, 10.0, &[ring]);
        receivers(&stock, [5.0, 3.5]).0
    };
    assert_eq!(placed(rotated), placed(ring));
}

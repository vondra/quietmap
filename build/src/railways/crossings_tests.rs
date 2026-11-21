//! Regression tests for global-graph crossing matching, track sharing, and period subtraction.
use super::*;
use crate::osm::geometry::{Square, lonlat_to_grid};
use crate::railways::{
    encode::Expanded,
    merge::{class_prior, row_evidence},
    model::RailModel,
    network::{Edge, Network},
    parallel_tracks::{allocate_over_parallel_tracks, subtract_passenger_periods},
    split::{ChildGeom, ChildRow},
    square_intervals::Interval,
    types::Crossing,
};
use std::collections::HashSet;

fn test_model() -> RailModel {
    serde_json::from_value(serde_json::json!({
        "network_bounds": {},
        "residuals": {},
        "spatial_priors": {},
        "sources": [
            {"id": 100, "rank": 5, "edition": 2024, "owner": "", "countries": [], "residual": false, "whole_line": false},
            {"id": 111, "rank": 4, "edition": 2024, "owner": "US", "countries": ["US"], "residual": false, "whole_line": true},
            {"id": 2079, "rank": 6, "edition": 2024, "owner": "US", "countries": ["US"], "residual": false, "whole_line": false}
        ],
        "type_priors": [[80.0, 20.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0]],
        "usage_priors": [[80.0, 20.0], [30.0, 5.0], [0.0, 15.0], [40.0, 10.0]],
        "mainline_freight": {},
        "passenger_periods": [0.70, 0.20, 0.10],
        "freight_periods": [0.40, 0.20, 0.40],
        "urban_periods": [0.60, 0.25, 0.15],
        "regional_freight_periods": []
    }))
    .unwrap()
}

fn meters_to_latlon(base_lat: f64, base_lon: f64, east_m: f64, north_m: f64) -> [f64; 2] {
    let cos = base_lat.to_radians().cos();
    [
        base_lat + north_m / 110_540.0,
        base_lon + east_m / (111_320.0 * cos),
    ]
}

fn add_edge_named(
    net: &mut Network,
    nodes: [usize; 2],
    way: i64,
    segment: i16,
    square: [u16; 2],
    extent: [f64; 2],
    corridor: &str,
) {
    let pa = net.nodes[nodes[0]];
    let pb = net.nodes[nodes[1]];
    net.edges.push(Edge {
        nodes,
        points: [pa, pb],
        length: (extent[1] - extent[0]).abs(),
        way,
        segment,
        square,
        extent,
        rail_type: 0,
        usage: 0,
        corridor: corridor.to_owned(),
        traversal_only: false,
    });
}

fn add_edge(
    net: &mut Network,
    a: usize,
    b: usize,
    way: i64,
    segment: i16,
    square: [u16; 2],
    extent: [f64; 2],
) {
    add_edge_named(net, [a, b], way, segment, square, extent, "Main");
}

fn crossing_at(
    id: &str,
    point: [f64; 2],
    total_periods: [f64; 3],
    passenger_daily: Option<f64>,
    main_tracks: u8,
) -> Crossing {
    Crossing {
        id: id.into(),
        lat: point[0],
        lon: point[1],
        soundings: [0.0; 3],
        speed_kmh: Some(80.0),
        source_id: 111,
        synth_osm: -1,
        total_periods: Some(total_periods),
        passenger_daily,
        main_tracks: Some(main_tracks),
        freight_only: passenger_daily == Some(0.0),
        measured_periods: true,
    }
}

#[allow(clippy::too_many_arguments)]
fn expanded_row_named(
    model: &RailModel,
    osm_id: i64,
    p0: [f64; 2],
    p1: [f64; 2],
    extent: [f64; 2],
    intervals: &[Interval],
    country: [u8; 2],
    corridor: &str,
) -> Expanded {
    let (start_gx, start_gy) = lonlat_to_grid(p0[1], p0[0]);
    let (end_gx, end_gy) = lonlat_to_grid(p1[1], p1[0]);
    let length_m = crate::railways::network::distance(p0, p1) as f32;
    let (traffic, foreign) = row_evidence(model, intervals, extent[0], extent[1], 0, country);
    Expanded {
        parent: 0,
        child: ChildRow {
            geom: ChildGeom {
                start_gx,
                start_gy,
                end_gx,
                end_gy,
                length_m,
            },
            traffic,
            foreign,
        },
        prior: class_prior(model, 0, 0, 0, country, 0),
        osm_id,
        corridor: corridor.to_owned(),
        rail_type: 0,
        usage: 0,
        service: 0,
        traffic_mode: 0,
        country_iso: country,
    }
}

fn expanded_row(
    model: &RailModel,
    osm_id: i64,
    p0: [f64; 2],
    p1: [f64; 2],
    extent: [f64; 2],
    intervals: &[Interval],
    country: [u8; 2],
) -> Expanded {
    expanded_row_named(model, osm_id, p0, p1, extent, intervals, country, "Main")
}

#[test]
fn switch_does_not_steal_second_track_slot_and_unrelated_railroad_is_rejected() {
    let (lat, lon) = (41.5, -93.6);
    let mut net = Network::default();
    // Track 1 splits at a switch 4 m north of the crossing (degree 3: north, south, turnout).
    let n_s = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, -50.0));
    let n_sw = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 4.0));
    let n_n = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 60.0));
    let n_turn = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, -15.0, 50.0));
    // Track 2 runs parallel 4.2 m east of Track 1 (split at y = 4 like Track 1).
    let n_t2_s = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 4.2, -50.0));
    let n_t2_m = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 4.2, 4.0));
    let n_t2_n = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 4.2, 60.0));
    // Unrelated railroad runs parallel 45 m east with its own corridor name.
    let n_far_s = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 45.0, -50.0));
    let n_far_n = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 45.0, 60.0));

    let sq = [100, 100];
    add_edge(&mut net, n_s, n_sw, 10, 0, sq, [0.0, 54.0]);
    add_edge(&mut net, n_sw, n_n, 11, 0, sq, [0.0, 56.0]);
    add_edge(&mut net, n_sw, n_turn, 12, 0, sq, [0.0, 48.0]);
    add_edge(&mut net, n_t2_s, n_t2_m, 20, 0, sq, [0.0, 54.0]);
    add_edge(&mut net, n_t2_m, n_t2_n, 20, 1, sq, [54.0, 110.0]);
    add_edge_named(
        &mut net,
        [n_far_s, n_far_n],
        30,
        0,
        sq,
        [0.0, 110.0],
        "Other",
    );
    net.index();

    let runs = build_runs(&net);
    let grid = EdgeGrid::build(&net);
    let crossing = crossing_at("SW01", [lat, lon], [37.0, 10.0, 27.0], Some(0.0), 2);
    let slots = match_crossing_slots(&crossing, &net, &grid, &runs);
    // Slot 0 = way 10 (Track 1 south), Slot 1 = way 20 (Track 2); way 11 must not steal Slot 1.
    assert_eq!(slots.len(), 2);
    assert!(slots.contains(&runs[0]));
    assert!(slots.contains(&runs[3]));
    assert!(!slots.contains(&runs[1]));

    let squares = HashSet::from([sq]);
    let claims = match_crossings(&net, &[crossing], 111, *b"US", &squares);
    let by_key: HashMap<(i64, i16), &Interval> = claims
        .iter()
        .map(|(_, iv)| ((iv.osm_id, iv.segment_idx), iv))
        .collect();
    assert!((by_key[&(10, 0)].freight - 37.0).abs() < 1e-9);
    assert!((by_key[&(20, 0)].freight - 37.0).abs() < 1e-9);
    assert!(!by_key.contains_key(&(30, 0)));

    // 24-degree diverging switch at (40, 7) with parallel twin at x = 45:
    // shared switch node + cross-section lateral projection must keep Slot 1 on the twin.
    let mut sw24 = Network::default();
    let s0 = sw24.nodes.len();
    sw24.nodes.push(meters_to_latlon(lat, lon, 40.0, -50.0));
    let s_sw = sw24.nodes.len();
    sw24.nodes.push(meters_to_latlon(lat, lon, 40.0, 7.0));
    let s_nw = sw24.nodes.len();
    // 24 degrees west of north from (40, 7): dx = -sin(24)*50 = -20.34, dy = cos(24)*50 = 45.68
    sw24.nodes.push(meters_to_latlon(lat, lon, 19.66, 52.68));
    let s_n = sw24.nodes.len();
    sw24.nodes.push(meters_to_latlon(lat, lon, 40.0, 60.0));
    let t2_0 = sw24.nodes.len();
    sw24.nodes.push(meters_to_latlon(lat, lon, 45.0, -50.0));
    let t2_1 = sw24.nodes.len();
    sw24.nodes.push(meters_to_latlon(lat, lon, 45.0, 60.0));
    add_edge(&mut sw24, s0, s_sw, 100, 0, sq, [0.0, 57.0]);
    add_edge(&mut sw24, s_sw, s_nw, 101, 0, sq, [0.0, 50.0]);
    add_edge(&mut sw24, s_sw, s_n, 102, 0, sq, [0.0, 53.0]);
    add_edge(&mut sw24, t2_0, t2_1, 200, 0, sq, [0.0, 110.0]);
    sw24.index();
    let sw24_runs = build_runs(&sw24);
    let sw24_grid = EdgeGrid::build(&sw24);
    let sw24_slots = match_crossing_slots(
        &crossing_at("SW24", [lat, lon], [37.0, 10.0, 27.0], Some(0.0), 2),
        &sw24,
        &sw24_grid,
        &sw24_runs,
    );
    assert_eq!(sw24_slots.len(), 2);
    assert!(
        sw24_slots.contains(&sw24_runs[3]),
        "parallel twin at x=45 must receive Slot 1"
    );

    // With Track 2 absent, a main_tracks=2 crossing must NOT bleed onto an unrelated way 45 m away.
    let mut single_net = Network::default();
    let a0 = single_net.nodes.len();
    single_net
        .nodes
        .push(meters_to_latlon(lat, lon, 0.0, -50.0));
    let a1 = single_net.nodes.len();
    single_net.nodes.push(meters_to_latlon(lat, lon, 0.0, 60.0));
    let b0 = single_net.nodes.len();
    single_net
        .nodes
        .push(meters_to_latlon(lat, lon, 45.0, -50.0));
    let b1 = single_net.nodes.len();
    single_net
        .nodes
        .push(meters_to_latlon(lat, lon, 45.0, 60.0));
    add_edge(&mut single_net, a0, a1, 10, 0, sq, [0.0, 110.0]);
    add_edge_named(&mut single_net, [b0, b1], 30, 0, sq, [0.0, 110.0], "Other");
    single_net.index();
    let single_claims = match_crossings(
        &single_net,
        &[crossing_at(
            "FAR01",
            [lat, lon],
            [37.0, 10.0, 27.0],
            Some(0.0),
            2,
        )],
        111,
        *b"US",
        &squares,
    );
    assert_eq!(single_claims.len(), 1);
    assert_eq!(single_claims[0].1.osm_id, 10);
    assert!((single_claims[0].1.freight - 74.0).abs() < 1e-9);
}

#[test]
fn balloon_loop_parallel_legs_share_crossing_instead_of_doubling() {
    let (lat, lon) = (41.5, -93.6);
    let mut net = Network::default();
    // Single degree-2 loop passing the crossing twice on two parallel legs 5 m apart.
    let n0 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, -50.0));
    let n1 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 50.0));
    let n2 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 5.0, 50.0));
    let n3 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 5.0, -50.0));
    let sq = [100, 100];
    add_edge(&mut net, n0, n1, 1, 0, sq, [0.0, 100.0]);
    add_edge(&mut net, n1, n2, 2, 0, sq, [0.0, 5.0]);
    add_edge(&mut net, n2, n3, 3, 0, sq, [0.0, 100.0]);
    add_edge(&mut net, n3, n0, 4, 0, sq, [0.0, 5.0]);
    net.index();

    let runs = build_runs(&net);
    assert_eq!(
        runs[0], runs[2],
        "both legs belong to one degree-2 loop run"
    );
    let claims = match_crossings(
        &net,
        &[crossing_at(
            "LOOP1",
            [lat, lon],
            [10.0, 2.0, 8.0],
            Some(0.0),
            2,
        )],
        111,
        *b"US",
        &HashSet::from([sq]),
    );
    let by_way: HashMap<i64, f64> = claims
        .iter()
        .map(|(_, iv)| (iv.osm_id, iv.freight))
        .collect();
    assert!((by_way[&1] - 10.0).abs() < 1e-9);
    assert!((by_way[&3] - 10.0).abs() < 1e-9);
    assert!((by_way[&1] + by_way[&3] - 20.0).abs() < 1e-9);
}

#[test]
fn double_track_across_square_boundary_does_not_double_and_propagates_across_cut() {
    let model = test_model();
    let (lat, lon) = (40.75, -98.84);
    let mut net = Network::default();
    let n1_a = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, -100.0));
    let n1_cut = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 100.0));
    let n1_c = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 300.0));
    let n2_a = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 4.2, -100.0));
    let n2_b = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 4.2, 100.0));

    let sq_a = [115, 192];
    let sq_b = [116, 192];
    let sq_c = [115, 191];
    add_edge(&mut net, n1_a, n1_cut, 1, 0, sq_a, [0.0, 200.0]);
    add_edge(&mut net, n1_cut, n1_c, 1, 1, sq_c, [200.0, 400.0]);
    add_edge(&mut net, n2_a, n2_b, 2, 0, sq_b, [0.0, 200.0]);
    net.index();

    let squares = HashSet::from([sq_a, sq_b, sq_c]);
    let crossing = crossing_at("GIBBON", [lat, lon], [34.0, 10.0, 30.0], Some(0.0), 2);
    let claims = match_crossings(&net, &[crossing], 111, *b"US", &squares);
    assert_eq!(claims.len(), 3);

    let finalize_one = |sq: [u16; 2], way: i64, p0: [f64; 2], p1: [f64; 2], ext: [f64; 2]| -> f64 {
        let ivs: Vec<Interval> = claims
            .iter()
            .filter(|(s, iv)| *s == sq && iv.osm_id == way)
            .map(|(_, iv)| iv.clone())
            .collect();
        let mut rows = vec![expanded_row(&model, way, p0, p1, ext, &ivs, *b"US")];
        allocate_over_parallel_tracks(&model, &mut rows, Square { x: sq[0], y: sq[1] });
        rows[0].child.traffic.freight.periods.iter().sum()
    };

    let t1_a = finalize_one(sq_a, 1, net.nodes[n1_a], net.nodes[n1_cut], [0.0, 200.0]);
    let t2_b = finalize_one(sq_b, 2, net.nodes[n2_a], net.nodes[n2_b], [0.0, 200.0]);
    let t1_c = finalize_one(sq_c, 1, net.nodes[n1_cut], net.nodes[n1_c], [200.0, 400.0]);
    assert!((t1_a - 37.0).abs() < 1e-6);
    assert!((t2_b - 37.0).abs() < 1e-6);
    assert!((t1_a + t2_b - 74.0).abs() < 1e-6);
    assert!((t1_c - 37.0).abs() < 1e-6);
}

#[test]
fn jonesboro_cross_square_passing_siding_conserves_total_across_boundary() {
    // Reproduces the BNSF Thayer South (way 166063669 + parallel 12774906) boundary geometry:
    // a single-track crossing (mt=1, 18/day) stamps Track 1 while its parallel track crosses
    // the square boundary 50 m earlier, leaving Track 1's 50 m boundary piece alone in sq_a.
    // Both sides of the boundary must carry 9 + 9 = 18, never 18 + 9 = 27.
    let model = test_model();
    let (lat, lon) = (35.8434, -90.7034);
    let mut net = Network::default();
    let t1_0 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, -150.0, 0.0));
    let t1_1 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, -50.0, 0.0));
    let t1_cut = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    let t1_2 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 150.0, 0.0));
    // Parallel track crosses into sq_b at x = -50 (50 m before Track 1's cut at x = 0).
    let t2_0 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, -150.0, 4.5));
    let t2_cut = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, -50.0, 4.5));
    let t2_2 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 150.0, 4.5));

    let sq_a = [126, 201];
    let sq_b = [127, 201];
    add_edge_named(
        &mut net,
        [t1_0, t1_1],
        166063669,
        3,
        sq_a,
        [0.0, 100.0],
        "BNSF",
    );
    add_edge_named(
        &mut net,
        [t1_1, t1_cut],
        166063669,
        4,
        sq_a,
        [100.0, 150.0],
        "BNSF",
    );
    add_edge_named(
        &mut net,
        [t1_cut, t1_2],
        166063669,
        5,
        sq_b,
        [150.0, 300.0],
        "BNSF",
    );
    add_edge_named(
        &mut net,
        [t2_0, t2_cut],
        12774906,
        1,
        sq_a,
        [0.0, 100.0],
        "BNSF",
    );
    add_edge_named(
        &mut net,
        [t2_cut, t2_2],
        12774906,
        0,
        sq_b,
        [100.0, 300.0],
        "BNSF",
    );
    net.index();

    let squares = HashSet::from([sq_a, sq_b]);
    let crossing = crossing_at(
        "672518M",
        meters_to_latlon(lat, lon, -100.0, 0.0),
        [9.0, 3.0, 6.0],
        Some(0.0),
        1,
    );
    let claims = match_crossings(&net, &[crossing], 111, *b"US", &squares);

    // Finalize sq_a and sq_b independently, exactly as prepare-railways does.
    let finalize_sq = |sq: [u16; 2], edges: &[(i64, i16, usize, usize, [f64; 2])]| -> Vec<f64> {
        let mut rows: Vec<Expanded> = edges
            .iter()
            .map(|&(way, seg, a, b, ext)| {
                let ivs: Vec<Interval> = claims
                    .iter()
                    .filter(|(s, iv)| *s == sq && iv.osm_id == way && iv.segment_idx == seg)
                    .map(|(_, iv)| iv.clone())
                    .collect();
                expanded_row_named(
                    &model,
                    way,
                    net.nodes[a],
                    net.nodes[b],
                    ext,
                    &ivs,
                    *b"US",
                    "BNSF",
                )
            })
            .collect();
        allocate_over_parallel_tracks(&model, &mut rows, Square { x: sq[0], y: sq[1] });
        rows.iter()
            .map(|row| row.child.traffic.freight.periods.iter().sum())
            .collect()
    };

    let sq_a_frt = finalize_sq(
        sq_a,
        &[
            (166063669, 3, t1_0, t1_1, [0.0, 100.0]),
            (166063669, 4, t1_1, t1_cut, [100.0, 150.0]),
            (12774906, 1, t2_0, t2_cut, [0.0, 100.0]),
        ],
    );
    let sq_b_frt = finalize_sq(
        sq_b,
        &[
            (166063669, 5, t1_cut, t1_2, [150.0, 300.0]),
            (12774906, 0, t2_cut, t2_2, [100.0, 300.0]),
        ],
    );
    // Every piece on both sides of the boundary must carry 9.0 (9 + 9 = 18, never 18 + 9 = 27).
    assert!((sq_a_frt[0] - 9.0).abs() < 1e-6, "seg 3: {}", sq_a_frt[0]);
    assert!(
        (sq_a_frt[1] - 9.0).abs() < 1e-6,
        "seg 4 boundary piece: {}",
        sq_a_frt[1]
    );
    assert!(
        (sq_a_frt[2] - 9.0).abs() < 1e-6,
        "parallel seg 1: {}",
        sq_a_frt[2]
    );
    assert!((sq_b_frt[0] - 9.0).abs() < 1e-6, "seg 5: {}", sq_b_frt[0]);
    assert!(
        (sq_b_frt[1] - 9.0).abs() < 1e-6,
        "parallel seg 0: {}",
        sq_b_frt[1]
    );
}

#[test]
fn third_running_track_beside_one_matched_run_never_inflates_corridor_total() {
    let model = test_model();
    let (lat, lon) = (41.4, -87.6);
    let sq = Square { x: 131, y: 191 };
    let a0 = meters_to_latlon(lat, lon, 0.0, 0.0);
    let a1 = meters_to_latlon(lat, lon, 0.0, 200.0);
    // Track C lies 12 m east of Run A (within 15 m tokenless gate) and 16 m from Run B (outside).
    let c0 = meters_to_latlon(lat, lon, 12.0, 0.0);
    let c1 = meters_to_latlon(lat, lon, 12.0, 200.0);
    let b0 = meters_to_latlon(lat, lon, -4.0, 0.0);
    let b1 = meters_to_latlon(lat, lon, -4.0, 200.0);

    let make_iv = |way: i64| Interval {
        missing_calendar_estimate: [0.0; 2],
        osm_id: way,
        segment_idx: 0,
        from_m: 0.0,
        to_m: 200.0,
        country: *b"US",
        source_id: 111,
        passenger: 0.0,
        freight: 37.0,
        passenger_status: STATUS_ESTIMATED,
        freight_status: STATUS_ESTIMATED,
        matching: 2,
        stop: 0,
        crossing: Some(CrossingTotal {
            periods_per_track: [17.0, 5.0, 15.0],
            passenger_per_track: Some(0.0),
            measured_periods: true,
            matched_tracks: 2,
        }),
    };
    // Run A (ref "Main 1", 37) pairs with unnamed Track C; Run B (ref "Main 2", 37) stays separate.
    // Total across A + C + B must conserve 74 (18.5 + 18.5 + 37), never inflate to 111.
    let mut rows = vec![
        expanded_row_named(
            &model,
            1,
            a0,
            a1,
            [0.0, 200.0],
            &[make_iv(1)],
            *b"US",
            "Main 1",
        ),
        expanded_row_named(&model, 3, c0, c1, [0.0, 200.0], &[], *b"US", ""),
        expanded_row_named(
            &model,
            2,
            b0,
            b1,
            [0.0, 200.0],
            &[make_iv(2)],
            *b"US",
            "Main 2",
        ),
    ];
    allocate_over_parallel_tracks(&model, &mut rows, sq);
    let frt: Vec<f64> = rows
        .iter()
        .map(|row| row.child.traffic.freight.periods.iter().sum())
        .collect();
    assert!((frt[0] - 18.5).abs() < 1e-6, "Run A: {}", frt[0]);
    assert!((frt[1] - 18.5).abs() < 1e-6, "Track C: {}", frt[1]);
    assert!((frt[2] - 37.0).abs() < 1e-6, "Run B: {}", frt[2]);
    assert!((frt.iter().sum::<f64>() - 74.0).abs() < 1e-6);
}

#[test]
fn below_prior_timetable_on_incomplete_section_keeps_fra_counts_instead_of_prior() {
    let model = test_model();
    let (lat, lon) = (41.4, -73.6);
    let p0 = meters_to_latlon(lat, lon, 0.0, 0.0);
    let p1 = meters_to_latlon(lat, lon, 0.0, 200.0);
    let q0 = meters_to_latlon(lat, lon, 4.0, 0.0);
    let q1 = meters_to_latlon(lat, lon, 4.0, 200.0);
    let sq = Square { x: 151, y: 191 };

    let fra_iv = Interval {
        missing_calendar_estimate: [0.0; 2],
        osm_id: 1,
        segment_idx: 0,
        from_m: 0.0,
        to_m: 200.0,
        country: *b"US",
        source_id: 111,
        passenger: 2.0,
        freight: 10.0,
        passenger_status: STATUS_ESTIMATED,
        freight_status: STATUS_ESTIMATED,
        matching: 2,
        stop: 0,
        crossing: Some(CrossingTotal {
            periods_per_track: [5.0, 2.0, 3.0],
            passenger_per_track: Some(2.0),
            measured_periods: true,
            matched_tracks: 1,
        }),
    };
    let gtfs_iv = Interval {
        missing_calendar_estimate: [0.0; 2],
        osm_id: 1,
        segment_idx: 0,
        from_m: 0.0,
        to_m: 200.0,
        country: *b"US",
        source_id: 100,
        passenger: 2.0,
        freight: 0.0,
        passenger_status: STATUS_ESTIMATED,
        freight_status: STATUS_UNKNOWN,
        matching: 2,
        stop: 0,
        crossing: None,
    };
    // Passing siding with Track 1 uncrossed: adding a 2-train/day timetable confirming FRA's
    // 2 passengers must keep 2 passenger + 8 freight across the section, never 80 + 20.
    let mut rows = vec![
        expanded_row(&model, 1, p0, p1, [0.0, 200.0], &[fra_iv, gtfs_iv], *b"US"),
        expanded_row(&model, 2, q0, q1, [0.0, 200.0], &[], *b"US"),
    ];
    allocate_over_parallel_tracks(&model, &mut rows, sq);
    let pax_total: f64 = rows
        .iter()
        .map(|row| row.child.traffic.passenger.periods.iter().sum::<f64>())
        .sum();
    let frt_total: f64 = rows
        .iter()
        .map(|row| row.child.traffic.freight.periods.iter().sum::<f64>())
        .sum();
    assert!((pax_total - 2.0).abs() < 1e-6, "pax_total={pax_total}");
    assert!((frt_total - 8.0).abs() < 1e-6, "frt_total={frt_total}");
    assert_eq!(rows[0].child.traffic.freight.source_id, 111);
    assert_eq!(rows[0].child.traffic.passenger.source_id, 111);
}

#[test]
fn long_straight_rural_segment_matches_crossing_far_from_its_endpoints() {
    let (lat, lon) = (42.0, -93.6);
    let mut net = Network::default();
    let n0 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, -3000.0));
    let n1 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 3000.0));
    let sq = [120, 190];
    add_edge(&mut net, n0, n1, 50, 0, sq, [0.0, 6000.0]);
    net.index();

    let claims = match_crossings(
        &net,
        &[crossing_at(
            "PRAIRIE",
            [lat, lon],
            [10.0, 2.0, 6.0],
            Some(0.0),
            1,
        )],
        111,
        *b"US",
        &HashSet::from([sq]),
    );
    assert_eq!(claims.len(), 1);
    assert!((claims[0].1.freight - 18.0).abs() < 1e-9);
}

#[test]
fn active_run_ignores_explicit_zero_and_takes_upper_middle_even_median() {
    let (lat, lon) = (41.0, -90.0);
    let mut net = Network::default();
    let n0 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, -200.0));
    let n1 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 200.0));
    let sq = [125, 192];
    add_edge(&mut net, n0, n1, 70, 0, sq, [0.0, 400.0]);
    net.index();
    let squares = HashSet::from([sq]);

    // [0, 14] must produce 14, never 0.
    let active = match_crossings(
        &net,
        &[
            crossing_at(
                "Z0",
                meters_to_latlon(lat, lon, 0.0, -50.0),
                [0.0; 3],
                Some(0.0),
                1,
            ),
            crossing_at(
                "A14",
                meters_to_latlon(lat, lon, 0.0, 50.0),
                [8.0, 2.0, 4.0],
                Some(0.0),
                1,
            ),
        ],
        111,
        *b"US",
        &squares,
    );
    assert!((active[0].1.freight - 14.0).abs() < 1e-9);

    // [10, 14] takes upper-middle (14), not lower-of-two (10).
    let even = match_crossings(
        &net,
        &[
            crossing_at(
                "A10",
                meters_to_latlon(lat, lon, 0.0, -50.0),
                [6.0, 1.0, 3.0],
                Some(0.0),
                1,
            ),
            crossing_at(
                "A14",
                meters_to_latlon(lat, lon, 0.0, 50.0),
                [8.0, 2.0, 4.0],
                Some(0.0),
                1,
            ),
        ],
        111,
        *b"US",
        &squares,
    );
    assert!((even[0].1.freight - 14.0).abs() < 1e-9);

    // All-zero run remains explicit zero.
    let zero = match_crossings(
        &net,
        &[crossing_at("Z0", [lat, lon], [0.0; 3], Some(0.0), 1)],
        111,
        *b"US",
        &squares,
    );
    assert_eq!(zero[0].1.freight, 0.0);
}

#[test]
fn period_subtraction_preserves_night_freight_and_unsplit_weekly_uses_freight_model_shares() {
    assert_eq!(
        subtract_passenger_periods([12.0, 4.0, 8.0], [12.0, 0.0, 0.0], 12.0),
        [0.0, 4.0, 8.0]
    );
    assert_eq!(
        subtract_passenger_periods([10.0, 0.0, 2.0], [8.0, 2.0, 2.0], 0.0),
        [0.0, 0.0, 0.0]
    );
    let mixed = subtract_passenger_periods([10.0, 1.0, 5.0], [8.0, 2.0, 2.0], 4.0);
    assert!((mixed[0] - 1.6).abs() < 1e-9);
    assert_eq!(mixed[1], 0.0);
    assert!((mixed[2] - 2.4).abs() < 1e-9);

    // End-to-end production path with modelled passenger_periods [0.7, 0.2, 0.1]:
    // measured FRA [12, 4, 8] minus 12 GTFS passengers ([8.4, 2.4, 1.2]) yields [3.6, 1.6, 6.8].
    let model = test_model();
    let (lat, lon) = (41.4, -73.6);
    let p0 = meters_to_latlon(lat, lon, 0.0, 0.0);
    let p1 = meters_to_latlon(lat, lon, 0.0, 200.0);
    let sq = Square { x: 151, y: 191 };
    let gtfs_12 = Interval {
        missing_calendar_estimate: [0.0; 2],
        osm_id: 1,
        segment_idx: 0,
        from_m: 0.0,
        to_m: 200.0,
        country: *b"US",
        source_id: 100,
        passenger: 12.0,
        freight: 0.0,
        passenger_status: STATUS_ESTIMATED,
        freight_status: STATUS_UNKNOWN,
        matching: 2,
        stop: 0,
        crossing: None,
    };
    let fra_measured = Interval {
        missing_calendar_estimate: [0.0; 2],
        osm_id: 1,
        segment_idx: 0,
        from_m: 0.0,
        to_m: 200.0,
        country: *b"US",
        source_id: 111,
        passenger: 12.0,
        freight: 24.0,
        passenger_status: STATUS_ESTIMATED,
        freight_status: STATUS_ESTIMATED,
        matching: 2,
        stop: 0,
        crossing: Some(CrossingTotal {
            periods_per_track: [12.0, 4.0, 8.0],
            passenger_per_track: Some(12.0),
            measured_periods: true,
            matched_tracks: 1,
        }),
    };
    let mut measured = vec![expanded_row(
        &model,
        1,
        p0,
        p1,
        [0.0, 200.0],
        &[gtfs_12.clone(), fra_measured],
        *b"US",
    )];
    allocate_over_parallel_tracks(&model, &mut measured, sq);
    let mf = measured[0].child.traffic.freight.periods;
    assert!((mf[0] - 3.6).abs() < 1e-6);
    assert!((mf[1] - 1.6).abs() < 1e-6);
    assert!((mf[2] - 6.8).abs() < 1e-6);

    // Unsplit weekly total (measured_periods = false) must spread daily freight remainder (10)
    // using model.freight_periods [0.4, 0.2, 0.4] -> [4.0, 2.0, 4.0], never subtracting from flat.
    let fra_weekly = Interval {
        missing_calendar_estimate: [0.0; 2],
        osm_id: 1,
        segment_idx: 0,
        from_m: 0.0,
        to_m: 200.0,
        country: *b"US",
        source_id: 111,
        passenger: 0.0,
        freight: 22.0,
        passenger_status: STATUS_ESTIMATED,
        freight_status: STATUS_ESTIMATED,
        matching: 2,
        stop: 0,
        crossing: Some(CrossingTotal {
            periods_per_track: [11.0, 11.0 / 3.0, 22.0 / 3.0],
            passenger_per_track: Some(0.0),
            measured_periods: false,
            matched_tracks: 1,
        }),
    };
    let mut weekly = vec![expanded_row(
        &model,
        1,
        p0,
        p1,
        [0.0, 200.0],
        &[gtfs_12, fra_weekly],
        *b"US",
    )];
    allocate_over_parallel_tracks(&model, &mut weekly, sq);
    let wf = weekly[0].child.traffic.freight.periods;
    assert!((wf[0] - 4.0).abs() < 1e-6);
    assert!((wf[1] - 2.0).abs() < 1e-6);
    assert!((wf[2] - 4.0).abs() < 1e-6);
}

#[test]
fn parallel_sharing_ranking_commuter_cap_ambiguous_fallback_and_canada_isolation() {
    let model = test_model();
    let (lat, lon) = (41.4, -73.6);
    let p0 = meters_to_latlon(lat, lon, 0.0, 0.0);
    let p1 = meters_to_latlon(lat, lon, 0.0, 200.0);
    let q0 = meters_to_latlon(lat, lon, 4.0, 0.0);
    let q1 = meters_to_latlon(lat, lon, 4.0, 200.0);
    let sq = Square { x: 151, y: 191 };

    let crossing_iv = |way: i64, periods: [f64; 3], pax: Option<f64>, k: u8| Interval {
        missing_calendar_estimate: [0.0; 2],
        osm_id: way,
        segment_idx: 0,
        from_m: 0.0,
        to_m: 200.0,
        country: *b"US",
        source_id: 111,
        passenger: pax.unwrap_or(0.0),
        freight: periods.iter().sum(),
        passenger_status: if pax.is_some() {
            STATUS_ESTIMATED
        } else {
            STATUS_UNKNOWN
        },
        freight_status: STATUS_ESTIMATED,
        matching: 2,
        stop: 0,
        crossing: Some(CrossingTotal {
            periods_per_track: periods,
            passenger_per_track: pax,
            measured_periods: true,
            matched_tracks: k,
        }),
    };

    // 1. Passing siding: single-track crossing (20/day, k=1) shared across 2 parallel tracks -> 10 + 10.
    let mut siding = vec![
        expanded_row(
            &model,
            1,
            p0,
            p1,
            [0.0, 200.0],
            &[crossing_iv(1, [10.0, 4.0, 6.0], Some(0.0), 1)],
            *b"US",
        ),
        expanded_row(&model, 2, q0, q1, [0.0, 200.0], &[], *b"US"),
    ];
    allocate_over_parallel_tracks(&model, &mut siding, sq);
    assert!((siding[0].child.traffic.freight.periods.iter().sum::<f64>() - 10.0).abs() < 1e-6);
    assert!((siding[1].child.traffic.freight.periods.iter().sum::<f64>() - 10.0).abs() < 1e-6);

    // 2. Commuter line (Metro-North Harlem: FRA total=52, pax=52; GTFS annual avg=41) -> 0 freight.
    let gtfs_iv = Interval {
        missing_calendar_estimate: [0.0; 2],
        osm_id: 1,
        segment_idx: 0,
        from_m: 0.0,
        to_m: 200.0,
        country: *b"US",
        source_id: 100,
        passenger: 41.0,
        freight: 0.0,
        passenger_status: STATUS_ESTIMATED,
        freight_status: STATUS_UNKNOWN,
        matching: 2,
        stop: 0,
        crossing: None,
    };
    let mut commuter = vec![expanded_row(
        &model,
        1,
        p0,
        p1,
        [0.0, 200.0],
        &[gtfs_iv, crossing_iv(1, [30.0, 8.0, 14.0], Some(52.0), 1)],
        *b"US",
    )];
    allocate_over_parallel_tracks(&model, &mut commuter, sq);
    assert_eq!(commuter[0].child.traffic.passenger.source_id, 100);
    assert_eq!(commuter[0].child.traffic.freight.periods, [0.0; 3]);

    // 3. Higher-rank freight (source 2079, rank 6) is never overwritten by FRA (rank 4).
    let high_freight = Interval {
        missing_calendar_estimate: [0.0; 2],
        osm_id: 1,
        segment_idx: 0,
        from_m: 0.0,
        to_m: 200.0,
        country: *b"US",
        source_id: 2079,
        passenger: 0.0,
        freight: 50.0,
        passenger_status: STATUS_UNKNOWN,
        freight_status: STATUS_ESTIMATED,
        matching: 2,
        stop: 0,
        crossing: None,
    };
    let mut ranked = vec![expanded_row(
        &model,
        1,
        p0,
        p1,
        [0.0, 200.0],
        &[high_freight, crossing_iv(1, [10.0, 2.0, 4.0], Some(0.0), 1)],
        *b"US",
    )];
    allocate_over_parallel_tracks(&model, &mut ranked, sq);
    assert_eq!(ranked[0].child.traffic.freight.source_id, 2079);
    assert!((ranked[0].child.traffic.freight.periods.iter().sum::<f64>() - 50.0).abs() < 1e-6);

    // 4. Unknown passenger_daily (None) without GTFS falls back to freight class prior (20.0).
    let mut ambig = vec![expanded_row(
        &model,
        1,
        p0,
        p1,
        [0.0, 200.0],
        &[crossing_iv(1, [10.0, 2.0, 4.0], None, 1)],
        *b"US",
    )];
    allocate_over_parallel_tracks(&model, &mut ambig, sq);
    assert_eq!(ambig[0].child.traffic.freight.source_id, 0);
    assert!((ambig[0].child.traffic.freight.periods.iter().sum::<f64>() - 20.0).abs() < 1e-6);

    // 5. Canadian row ignores US FRA source 111 completely (stays on class prior).
    let mut canada = vec![expanded_row(
        &model,
        1,
        p0,
        p1,
        [0.0, 200.0],
        &[crossing_iv(1, [40.0, 10.0, 20.0], Some(0.0), 1)],
        *b"CA",
    )];
    allocate_over_parallel_tracks(&model, &mut canada, sq);
    assert_eq!(canada[0].child.traffic.freight.source_id, 0);
    assert_eq!(canada[0].child.traffic.passenger.source_id, 0);
}

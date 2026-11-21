//! Regression tests for global-graph crossing matching, unified cross-section allocation, and period subtraction.
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
use std::collections::{HashMap, HashSet};

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

#[allow(clippy::too_many_arguments)]
fn add_edge_full(
    net: &mut Network,
    nodes: [usize; 2],
    way: i64,
    segment: i16,
    square: [u16; 2],
    extent: [f64; 2],
    usage: u8,
    country: [u8; 2],
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
        usage,
        traffic_mode: 0,
        country,
        corridor: corridor.to_owned(),
        corridor_name: String::new(),
        traversal_only: false,
    });
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
    add_edge_full(
        net, nodes, way, segment, square, extent, 0, *b"US", corridor,
    );
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

fn match_no_yards(
    model: &RailModel,
    network: &Network,
    crossings: &[Crossing],
    source_id: u16,
    country: [u8; 2],
    squares: &HashSet<[u16; 2]>,
    existing_claims: &Claims,
) -> Vec<([u16; 2], Interval)> {
    match_crossings(
        model,
        network,
        crossings,
        source_id,
        country,
        squares,
        existing_claims,
        &HashSet::new(),
    )
}

fn crossing_at(
    id: &str,
    point: [f64; 2],
    total_periods: [f64; 3],
    passenger_daily: Option<f64>,
    _main_tracks: u8,
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
        passenger_lt1: false,
        measured_periods: true,
    }
}

fn timetable_iv(way: i64, segment: i16, extent: [f64; 2], source_id: u16, pax: f64) -> Interval {
    Interval {
        missing_calendar_estimate: [0.0; 2],
        osm_id: way,
        segment_idx: segment,
        from_m: extent[0],
        to_m: extent[1],
        country: *b"US",
        source_id,
        passenger: pax,
        freight: 0.0,
        passenger_status: STATUS_ESTIMATED,
        freight_status: STATUS_UNKNOWN,
        matching: 2,
        stop: 0,
        resolved_section: None,
    }
}

fn claim_freight(iv: &Interval) -> f64 {
    iv.resolved_section
        .expect("expected resolved_section")
        .freight
        .periods
        .iter()
        .sum()
}

fn claim_passenger(iv: &Interval) -> f64 {
    iv.resolved_section
        .expect("expected resolved_section")
        .passenger
        .periods
        .iter()
        .sum()
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
    let resolved_section = crate::railways::merge::overlapping(intervals, extent[0], extent[1])
        .find_map(|iv| iv.resolved_section);
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
            resolved_section,
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
fn switch_turnout_and_unrelated_railroad_are_excluded_from_crossing_section() {
    let model = test_model();
    let (lat, lon) = (41.5, -93.6);
    let mut net = Network::default();
    let n_s = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, -50.0));
    let n_sw = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 4.0));
    let n_n = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 60.0));
    let n_turn = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, -15.0, 50.0));
    let n_t2_s = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 4.2, -50.0));
    let n_t2_m = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 4.2, 4.0));
    let n_t2_n = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 4.2, 60.0));
    let n_far_s = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 45.0, -50.0));
    let n_far_n = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 45.0, 60.0));

    let sq = [100, 100];
    add_edge(&mut net, n_s, n_sw, 10, 0, sq, [0.0, 54.0]);
    add_edge(&mut net, n_sw, n_n, 11, 0, sq, [0.0, 56.0]);
    add_edge_named(&mut net, [n_sw, n_turn], 12, 0, sq, [0.0, 48.0], "");
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

    let squares = HashSet::from([sq]);
    let crossing = crossing_at("SW01", [lat, lon], [37.0, 10.0, 27.0], Some(0.0), 2);
    let claims = match_no_yards(
        &model,
        &net,
        &[crossing],
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );
    let by_key: HashMap<(i64, i16), &Interval> = claims
        .iter()
        .map(|(_, iv)| ((iv.osm_id, iv.segment_idx), iv))
        .collect();
    assert!((claim_freight(by_key[&(10, 0)]) - 37.0).abs() < 1e-9);
    assert!((claim_freight(by_key[&(20, 0)]) - 37.0).abs() < 1e-9);
    assert!(!by_key.contains_key(&(12, 0)), "18-deg turnout excluded");
    assert!(
        !by_key.contains_key(&(30, 0)),
        "unrelated railroad excluded"
    );

    // 24-degree diverging switch at (40, 7) with parallel twin at x = 45.
    let mut sw24 = Network::default();
    let s0 = sw24.nodes.len();
    sw24.nodes.push(meters_to_latlon(lat, lon, 40.0, -50.0));
    let s_sw = sw24.nodes.len();
    sw24.nodes.push(meters_to_latlon(lat, lon, 40.0, 7.0));
    let s_nw = sw24.nodes.len();
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
    let sw24_claims = match_no_yards(
        &model,
        &sw24,
        &[crossing_at(
            "SW24",
            meters_to_latlon(lat, lon, 40.0, 0.0),
            [37.0, 10.0, 27.0],
            Some(0.0),
            2,
        )],
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );
    let sw24_by: HashMap<i64, f64> = sw24_claims
        .iter()
        .map(|(_, iv)| (iv.osm_id, claim_freight(iv)))
        .collect();
    assert!((sw24_by[&100] - 37.0).abs() < 1e-9);
    assert!((sw24_by[&200] - 37.0).abs() < 1e-9);
    assert!(!sw24_by.contains_key(&101));

    // Different-named railroad 4 m away must NOT steal a slot from the same-named twin 8 m away.
    let mut close_rr = Network::default();
    let b1a = close_rr.nodes.len();
    close_rr.nodes.push(meters_to_latlon(lat, lon, 0.0, -50.0));
    let b1b = close_rr.nodes.len();
    close_rr.nodes.push(meters_to_latlon(lat, lon, 0.0, 60.0));
    let u0 = close_rr.nodes.len();
    close_rr.nodes.push(meters_to_latlon(lat, lon, 4.0, -50.0));
    let u1 = close_rr.nodes.len();
    close_rr.nodes.push(meters_to_latlon(lat, lon, 4.0, 60.0));
    let b2a = close_rr.nodes.len();
    close_rr.nodes.push(meters_to_latlon(lat, lon, 8.0, -50.0));
    let b2b = close_rr.nodes.len();
    close_rr.nodes.push(meters_to_latlon(lat, lon, 8.0, 60.0));
    add_edge_named(&mut close_rr, [b1a, b1b], 1, 0, sq, [0.0, 110.0], "BNSF");
    add_edge_named(&mut close_rr, [u0, u1], 2, 0, sq, [0.0, 110.0], "UP");
    add_edge_named(&mut close_rr, [b2a, b2b], 3, 0, sq, [0.0, 110.0], "BNSF");
    close_rr.index();
    let close_claims = match_no_yards(
        &model,
        &close_rr,
        &[crossing_at(
            "CLOSE",
            [lat, lon],
            [20.0, 8.0, 12.0],
            Some(0.0),
            2,
        )],
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );
    let close_by: HashMap<i64, f64> = close_claims
        .iter()
        .map(|(_, iv)| (iv.osm_id, claim_freight(iv)))
        .collect();
    assert!((close_by[&1] - 20.0).abs() < 1e-9);
    assert!((close_by[&3] - 20.0).abs() < 1e-9);
    assert!(
        !close_by.contains_key(&2),
        "UP at 4 m must not take BNSF trains"
    );
}

#[test]
fn same_name_twins_20m_apart_and_balloon_loop_mt1_conserve_corridor_totals() {
    let model = test_model();
    let (lat, lon) = (41.5, -93.6);
    let sq = [100, 100];
    let squares = HashSet::from([sq]);

    // Grok #1: two same-name mains 20 m apart, each with a 40-train mt=2 crossing -> 20 + 20 = 40.
    let mut wide = Network::default();
    let a0 = wide.nodes.len();
    wide.nodes.push(meters_to_latlon(lat, lon, 0.0, -100.0));
    let a1 = wide.nodes.len();
    wide.nodes.push(meters_to_latlon(lat, lon, 0.0, 100.0));
    let b0 = wide.nodes.len();
    wide.nodes.push(meters_to_latlon(lat, lon, 20.0, -100.0));
    let b1 = wide.nodes.len();
    wide.nodes.push(meters_to_latlon(lat, lon, 20.0, 100.0));
    add_edge_named(&mut wide, [a0, a1], 1, 0, sq, [0.0, 200.0], "SubA");
    add_edge_named(&mut wide, [b0, b1], 2, 0, sq, [0.0, 200.0], "SubA");
    wide.index();
    let wide_claims = match_no_yards(
        &model,
        &wide,
        &[
            crossing_at(
                "X1",
                meters_to_latlon(lat, lon, 0.0, -10.0),
                [20.0, 4.0, 16.0],
                Some(0.0),
                2,
            ),
            crossing_at(
                "X2",
                meters_to_latlon(lat, lon, 20.0, 10.0),
                [20.0, 4.0, 16.0],
                Some(0.0),
                2,
            ),
        ],
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );
    let wide_by: HashMap<i64, f64> = wide_claims
        .iter()
        .map(|(_, iv)| (iv.osm_id, claim_freight(iv)))
        .collect();
    assert!((wide_by[&1] - 20.0).abs() < 1e-9);
    assert!((wide_by[&2] - 20.0).abs() < 1e-9);
    assert!((wide_by[&1] + wide_by[&2] - 40.0).abs() < 1e-9);

    // Grok #1b: single degree-2 balloon loop passing the crossing twice (both mt=1 and mt=2).
    let mut loop_net = Network::default();
    let n0 = loop_net.nodes.len();
    loop_net.nodes.push(meters_to_latlon(lat, lon, 0.0, -50.0));
    let n1 = loop_net.nodes.len();
    loop_net.nodes.push(meters_to_latlon(lat, lon, 0.0, 50.0));
    let n2 = loop_net.nodes.len();
    loop_net.nodes.push(meters_to_latlon(lat, lon, 5.0, 50.0));
    let n3 = loop_net.nodes.len();
    loop_net.nodes.push(meters_to_latlon(lat, lon, 5.0, -50.0));
    add_edge(&mut loop_net, n0, n1, 1, 0, sq, [0.0, 100.0]);
    add_edge(&mut loop_net, n1, n2, 2, 0, sq, [0.0, 5.0]);
    add_edge(&mut loop_net, n2, n3, 3, 0, sq, [0.0, 100.0]);
    add_edge(&mut loop_net, n3, n0, 4, 0, sq, [0.0, 5.0]);
    loop_net.index();
    let layout = CorridorLayout::build(&loop_net, *b"US", &HashSet::new());
    assert_eq!(layout.edge_run[0].unwrap().0, layout.edge_run[2].unwrap().0);
    for mt in [1u8, 2u8] {
        let claims = match_no_yards(
            &model,
            &loop_net,
            &[crossing_at(
                "LOOP",
                [lat, lon],
                [10.0, 2.0, 8.0],
                Some(0.0),
                mt,
            )],
            111,
            *b"US",
            &squares,
            &Claims::new(),
        );
        let by_way: HashMap<i64, f64> = claims
            .iter()
            .map(|(_, iv)| (iv.osm_id, claim_freight(iv)))
            .collect();
        assert!((by_way[&1] - 10.0).abs() < 1e-9, "mt={mt}: {}", by_way[&1]);
        assert!((by_way[&3] - 10.0).abs() < 1e-9, "mt={mt}: {}", by_way[&3]);
        assert!((by_way[&1] + by_way[&3] - 20.0).abs() < 1e-9);
    }
}

#[test]
fn usage_branch_beside_main_never_steals_mainline_crossing_slot() {
    // Grok #2: main (usage 0) at 0 m, branch (usage 1) at 8 m, twin main (same name, usage 0) at 30 m.
    let model = test_model();
    let (lat, lon) = (41.5, -93.6);
    let sq = [100, 100];
    let mut net = Network::default();
    let m1a = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, -100.0));
    let m1b = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 100.0));
    let ba = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 8.0, -100.0));
    let bb = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 8.0, 100.0));
    let m2a = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 30.0, -100.0));
    let m2b = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 30.0, 100.0));
    add_edge_full(
        &mut net,
        [m1a, m1b],
        1,
        0,
        sq,
        [0.0, 200.0],
        0,
        *b"US",
        "Main",
    );
    add_edge_full(
        &mut net,
        [ba, bb],
        2,
        0,
        sq,
        [0.0, 200.0],
        1,
        *b"US",
        "Main",
    );
    add_edge_full(
        &mut net,
        [m2a, m2b],
        3,
        0,
        sq,
        [0.0, 200.0],
        0,
        *b"US",
        "Main",
    );
    net.index();

    let claims = match_no_yards(
        &model,
        &net,
        &[crossing_at(
            "BR01",
            [lat, lon],
            [20.0, 4.0, 16.0],
            Some(0.0),
            2,
        )],
        111,
        *b"US",
        &HashSet::from([sq]),
        &Claims::new(),
    );
    let by_way: HashMap<i64, f64> = claims
        .iter()
        .map(|(_, iv)| (iv.osm_id, claim_freight(iv)))
        .collect();
    assert!((by_way[&1] - 20.0).abs() < 1e-9);
    assert!((by_way[&3] - 20.0).abs() < 1e-9);
    assert!(!by_way.contains_key(&2), "branch must stay untouched");
}

#[test]
fn converging_tracks_ahead_of_switch_share_crossing() {
    // Astra Other #1: two main tracks 6 m apart at the crossing converging to a switch 100 m ahead.
    let model = test_model();
    let (lat, lon) = (41.5, -93.6);
    let sq = [100, 100];
    let mut net = Network::default();
    let t1_s = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    let sw = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 100.0));
    let t2_s = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 6.0, 0.0));
    add_edge_named(&mut net, [t1_s, sw], 1, 0, sq, [0.0, 100.0], "");
    add_edge_named(&mut net, [t2_s, sw], 2, 0, sq, [0.0, 100.2], "");
    net.index();

    let claims = match_no_yards(
        &model,
        &net,
        &[crossing_at(
            "CONV",
            [lat, lon],
            [20.0, 4.0, 16.0],
            Some(0.0),
            2,
        )],
        111,
        *b"US",
        &HashSet::from([sq]),
        &Claims::new(),
    );
    let by_way: HashMap<i64, f64> = claims
        .iter()
        .map(|(_, iv)| (iv.osm_id, claim_freight(iv)))
        .collect();
    assert!((by_way[&1] - 20.0).abs() < 1e-9);
    assert!((by_way[&2] - 20.0).abs() < 1e-9);
}

#[test]
fn three_unnamed_parallel_tracks_conserve_total_through_global_matching() {
    // Astra #3: three unnamed tracks at 0, 10, 20 m with one 60-train mt=1 crossing on the middle
    // track must produce 20 + 20 + 20 = 60 via transitive global cross-sections, never 30+20+30=80.
    let model = test_model();
    let (lat, lon) = (41.4, -87.6);
    let sq = [131, 191];
    let mut net = Network::default();
    for (way, x) in [(1i64, 0.0), (2, 10.0), (3, 20.0)] {
        let a = net.nodes.len();
        net.nodes.push(meters_to_latlon(lat, lon, x, 0.0));
        let b = net.nodes.len();
        net.nodes.push(meters_to_latlon(lat, lon, x, 200.0));
        add_edge_named(&mut net, [a, b], way, 0, sq, [0.0, 200.0], "");
    }
    net.index();

    let claims = match_no_yards(
        &model,
        &net,
        &[crossing_at(
            "THREE",
            meters_to_latlon(lat, lon, 10.0, 100.0),
            [30.0, 10.0, 20.0],
            Some(0.0),
            1,
        )],
        111,
        *b"US",
        &HashSet::from([sq]),
        &Claims::new(),
    );
    let mut rows: Vec<Expanded> = [1i64, 2, 3]
        .into_iter()
        .enumerate()
        .map(|(i, way)| {
            let ivs: Vec<Interval> = claims
                .iter()
                .filter(|(_, iv)| iv.osm_id == way)
                .map(|(_, iv)| iv.clone())
                .collect();
            let edge = &net.edges[i];
            expanded_row_named(
                &model,
                way,
                edge.points[0],
                edge.points[1],
                edge.extent,
                &ivs,
                *b"US",
                "",
            )
        })
        .collect();
    allocate_over_parallel_tracks(&model, &mut rows, Square { x: sq[0], y: sq[1] });
    let frt: Vec<f64> = rows
        .iter()
        .map(|row| row.child.traffic.freight.periods.iter().sum())
        .collect();
    assert!((frt[0] - 20.0).abs() < 1e-6, "track 0: {}", frt[0]);
    assert!((frt[1] - 20.0).abs() < 1e-6, "track 1: {}", frt[1]);
    assert!((frt[2] - 20.0).abs() < 1e-6, "track 2: {}", frt[2]);
    assert!((frt.iter().sum::<f64>() - 60.0).abs() < 1e-6);
}

#[test]
fn raritan_valley_square_cut_keeps_timetable_and_freight_constant_across_boundary() {
    // Astra #2: Track 1 (way 202665789) carries NJT 46.57 pax across sq_a -> sq_b while Track 2
    // (way 241859599) lies in sq_a beside both segments. Global allocation must give 23.285 pax
    // (source 2079) and identical freight on every segment across the square boundary.
    let model = test_model();
    let (lat, lon) = (40.62, -74.43);
    let sq_a = [149, 192];
    let sq_b = [150, 192];
    let mut net = Network::default();
    let a0 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, -100.0, 0.0));
    let a1 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    let a2 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 80.0, 0.0));
    let b0 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, -100.0, 4.2));
    let b1 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 80.0, 4.2));
    add_edge_named(&mut net, [a0, a1], 202665789, 9, sq_a, [0.0, 100.0], "RVL");
    add_edge_named(
        &mut net,
        [a1, a2],
        202665789,
        10,
        sq_b,
        [100.0, 180.0],
        "RVL",
    );
    add_edge_named(&mut net, [b0, b1], 241859599, 0, sq_a, [0.0, 180.0], "RVL");
    net.index();

    let mut existing = Claims::new();
    existing
        .entry(sq_a)
        .or_default()
        .push((timetable_iv(202665789, 9, [0.0, 100.0], 2079, 46.57), 0));
    existing
        .entry(sq_b)
        .or_default()
        .push((timetable_iv(202665789, 10, [100.0, 180.0], 2079, 46.57), 0));

    let crossing = crossing_at(
        "RVL1",
        meters_to_latlon(lat, lon, -50.0, 0.0),
        [33.0, 10.0, 10.0], // 53 total, 49 passenger -> 4 freight across 2 tracks = 2/track
        Some(49.0),
        2,
    );
    let claims = match_no_yards(
        &model,
        &net,
        &[crossing],
        111,
        *b"US",
        &HashSet::from([sq_a, sq_b]),
        &existing,
    );
    let by_key: HashMap<(i64, i16), &Interval> = claims
        .iter()
        .map(|(_, iv)| ((iv.osm_id, iv.segment_idx), iv))
        .collect();
    for key in [(202665789, 9), (202665789, 10), (241859599, 0)] {
        let rt = by_key[&key].resolved_section.unwrap();
        assert_eq!(rt.passenger.source_id, 2079, "key {key:?}");
        assert_eq!(rt.freight.source_id, 111, "key {key:?}");
        let pax: f64 = rt.passenger.periods.iter().sum();
        let frt: f64 = rt.freight.periods.iter().sum();
        assert!((pax - 23.285).abs() < 1e-6, "key {key:?}: pax={pax}");
        assert!((frt - 2.0).abs() < 1e-6, "key {key:?}: frt={frt}");
    }
}

#[test]
fn below_prior_timetable_outranks_inventory_passenger_split() {
    // Astra #4: 6 scheduled passengers on one of two parallel tracks + FRA 10 total / 0 pax
    // must keep 6 passenger (timetable source 100) + 4 freight (FRA 111), never 0 + 10 or 80 prior.
    let model = test_model();
    let (lat, lon) = (41.4, -73.6);
    let sq = [151, 191];
    let mut net = Network::default();
    let a0 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    let a1 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 200.0));
    let b0 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 4.0, 0.0));
    let b1 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 4.0, 200.0));
    add_edge(&mut net, a0, a1, 1, 0, sq, [0.0, 200.0]);
    add_edge(&mut net, b0, b1, 2, 0, sq, [0.0, 200.0]);
    net.index();

    let mut existing = Claims::new();
    existing
        .entry(sq)
        .or_default()
        .push((timetable_iv(1, 0, [0.0, 200.0], 100, 6.0), 0));

    let claims = match_no_yards(
        &model,
        &net,
        &[crossing_at(
            "TT6",
            meters_to_latlon(lat, lon, 0.0, 100.0),
            [5.0, 2.0, 3.0],
            Some(0.0),
            1,
        )],
        111,
        *b"US",
        &HashSet::from([sq]),
        &existing,
    );
    assert_eq!(claims.len(), 2);
    for (_, iv) in &claims {
        let rt = iv.resolved_section.unwrap();
        assert_eq!(rt.passenger.source_id, 100);
        assert_eq!(rt.freight.source_id, 111);
        assert!((claim_passenger(iv) - 3.0).abs() < 1e-6);
        assert!((claim_freight(iv) - 2.0).abs() < 1e-6);
    }
}

#[test]
fn measured_sibling_keeps_daylight_shape_when_shared_with_weekly_sibling() {
    // Astra #5b: Track 1 carries measured [12, 0, 0] (0 eve, 0 night) and Track 2 carries a
    // weekly-only 1/7 observation. Sharing across the 2 tracks must keep the 12 daylight trains
    // in daytime and spread ONLY the weekly 1/7 across freight shares [0.4, 0.2, 0.4].
    let model = test_model();
    let (lat, lon) = (41.4, -73.6);
    let sq = [151, 191];
    let mut net = Network::default();
    let a0 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    let a1 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 200.0));
    let a2 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 400.0));
    let b0 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 4.0, 200.0));
    let b1 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 4.0, 400.0));
    let b2 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 4.0, 600.0));
    // Track 1 alone on y=0..200 (gets measured 12), Track 2 alone on y=400..600 (gets weekly 1/7),
    // both parallel on y=200..400.
    add_edge(&mut net, a0, a1, 1, 0, sq, [0.0, 200.0]);
    add_edge(&mut net, a1, a2, 1, 1, sq, [200.0, 400.0]);
    add_edge(&mut net, b0, b1, 2, 0, sq, [0.0, 200.0]);
    add_edge(&mut net, b1, b2, 2, 1, sq, [200.0, 400.0]);
    net.index();

    let c_measured = crossing_at(
        "DAY12",
        meters_to_latlon(lat, lon, 0.0, 100.0),
        [12.0, 0.0, 0.0],
        Some(0.0),
        1,
    );
    let mut c_weekly = crossing_at(
        "WK1",
        meters_to_latlon(lat, lon, 4.0, 500.0),
        [0.5 / 7.0, 1.0 / 42.0, 2.0 / 42.0],
        Some(0.0),
        1,
    );
    c_weekly.measured_periods = false;

    let claims = match_no_yards(
        &model,
        &net,
        &[c_measured, c_weekly],
        111,
        *b"US",
        &HashSet::from([sq]),
        &Claims::new(),
    );
    let by_key: HashMap<(i64, i16), &Interval> = claims
        .iter()
        .map(|(_, iv)| ((iv.osm_id, iv.segment_idx), iv))
        .collect();
    let p = by_key[&(1, 1)].resolved_section.unwrap().freight.periods;
    // Per track on the 2-track overlap: daylight = 6 + (1/14)*0.4, evening = (1/14)*0.2, night = (1/14)*0.4.
    assert!((p[0] - (6.0 + 0.4 / 14.0)).abs() < 1e-6, "day={}", p[0]);
    assert!((p[1] - (0.2 / 14.0)).abs() < 1e-6, "eve={}", p[1]);
    assert!((p[2] - (0.4 / 14.0)).abs() < 1e-6, "night={}", p[2]);
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
    let claims = match_no_yards(
        &model,
        &net,
        &[crossing],
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );
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
    let claims = match_no_yards(
        &model,
        &net,
        &[crossing],
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );

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
    for (idx, val) in sq_a_frt.iter().chain(sq_b_frt.iter()).enumerate() {
        assert!((*val - 9.0).abs() < 1e-6, "piece {idx}: {val}");
    }
}

#[test]
fn long_straight_rural_segment_matches_crossing_far_from_its_endpoints() {
    let model = test_model();
    let (lat, lon) = (42.0, -93.6);
    let mut net = Network::default();
    let n0 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, -3000.0));
    let n1 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 3000.0));
    let sq = [120, 190];
    add_edge(&mut net, n0, n1, 50, 0, sq, [0.0, 6000.0]);
    net.index();

    let claims = match_no_yards(
        &model,
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
        &Claims::new(),
    );
    assert_eq!(claims.len(), 1);
    assert!((claim_freight(&claims[0].1) - 18.0).abs() < 1e-9);
}

#[test]
fn active_run_median_prefers_positive_known_split_and_measured_periods() {
    let model = test_model();
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

    // [0, 14] -> 14.
    let active = match_no_yards(
        &model,
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
        &Claims::new(),
    );
    assert!((claim_freight(&active[0].1) - 14.0).abs() < 1e-9);

    // [10, 14] -> upper-middle (14).
    let even = match_no_yards(
        &model,
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
        &Claims::new(),
    );
    assert!((claim_freight(&even[0].1) - 14.0).abs() < 1e-9);

    // Astra Other #2: tie at total=10 between known passenger split (Some(2)) and missing (None)
    // must pick the known split (2 passenger + 8 freight), never dropping to class prior.
    let tied = match_no_yards(
        &model,
        &net,
        &[
            crossing_at(
                "K10",
                meters_to_latlon(lat, lon, 0.0, -50.0),
                [6.0, 1.0, 3.0],
                Some(2.0),
                1,
            ),
            crossing_at(
                "M10",
                meters_to_latlon(lat, lon, 0.0, 50.0),
                [6.0, 1.0, 3.0],
                None,
                1,
            ),
        ],
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );
    assert_eq!(tied.len(), 1);
    assert!((claim_passenger(&tied[0].1) - 2.0).abs() < 1e-9);
    assert!((claim_freight(&tied[0].1) - 8.0).abs() < 1e-9);

    // All-zero run remains explicit zero.
    let zero = match_no_yards(
        &model,
        &net,
        &[crossing_at("Z0", [lat, lon], [0.0; 3], Some(0.0), 1)],
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );
    assert_eq!(claim_freight(&zero[0].1), 0.0);
}

#[test]
fn period_subtraction_and_unsplit_weekly_preserve_expected_period_shares() {
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

    let model = test_model();
    let (lat, lon) = (41.4, -73.6);
    let sq = [151, 191];
    let mut net = Network::default();
    let a0 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    let a1 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 200.0));
    add_edge(&mut net, a0, a1, 1, 0, sq, [0.0, 200.0]);
    net.index();

    let mut existing = Claims::new();
    existing
        .entry(sq)
        .or_default()
        .push((timetable_iv(1, 0, [0.0, 200.0], 100, 12.0), 0));

    // Measured [12, 4, 8] minus 12 GTFS ([8.4, 2.4, 1.2]) -> [3.6, 1.6, 6.8].
    let measured = match_no_yards(
        &model,
        &net,
        &[crossing_at(
            "M24",
            [lat, lon],
            [12.0, 4.0, 8.0],
            Some(12.0),
            1,
        )],
        111,
        *b"US",
        &HashSet::from([sq]),
        &existing,
    );
    let mf = measured[0].1.resolved_section.unwrap().freight.periods;
    assert!((mf[0] - 3.6).abs() < 1e-6);
    assert!((mf[1] - 1.6).abs() < 1e-6);
    assert!((mf[2] - 6.8).abs() < 1e-6);

    // Unsplit weekly total (22/day) minus 12 GTFS -> freight remainder 10 * [0.4, 0.2, 0.4].
    let mut weekly_crossing = crossing_at(
        "W22",
        [lat, lon],
        [11.0, 11.0 / 3.0, 22.0 / 3.0],
        Some(0.0),
        1,
    );
    weekly_crossing.measured_periods = false;
    let weekly = match_no_yards(
        &model,
        &net,
        &[weekly_crossing],
        111,
        *b"US",
        &HashSet::from([sq]),
        &existing,
    );
    let wf = weekly[0].1.resolved_section.unwrap().freight.periods;
    assert!((wf[0] - 4.0).abs() < 1e-6);
    assert!((wf[1] - 2.0).abs() < 1e-6);
    assert!((wf[2] - 4.0).abs() < 1e-6);
}

#[test]
fn ranking_commuter_cap_ambiguous_fallback_canada_isolation_and_newly_stamped_yard() {
    let model = test_model();
    let (lat, lon) = (41.4, -73.6);
    let sq = [151, 191];
    let mut net = Network::default();
    let a0 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    let a1 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 200.0));
    add_edge(&mut net, a0, a1, 1, 0, sq, [0.0, 200.0]);
    net.index();
    let squares = HashSet::from([sq]);

    // 1. Commuter line (Harlem: FRA 52 total / 52 pax, GTFS 41) -> 41 pax (100) + 0 frt (111).
    let mut gtfs_claims = Claims::new();
    gtfs_claims
        .entry(sq)
        .or_default()
        .push((timetable_iv(1, 0, [0.0, 200.0], 100, 41.0), 0));
    let commuter = match_no_yards(
        &model,
        &net,
        &[crossing_at(
            "HAR",
            [lat, lon],
            [30.0, 8.0, 14.0],
            Some(52.0),
            1,
        )],
        111,
        *b"US",
        &squares,
        &gtfs_claims,
    );
    let rt = commuter[0].1.resolved_section.unwrap();
    assert_eq!(rt.passenger.source_id, 100);
    assert!((claim_passenger(&commuter[0].1) - 41.0).abs() < 1e-6);
    assert_eq!(rt.freight.periods, [0.0; 3]);

    // 2. Higher-rank freight (source 2079, rank 6) is never overwritten by FRA (rank 4).
    let mut high_frt = Claims::new();
    high_frt.entry(sq).or_default().push((
        Interval {
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
            resolved_section: None,
        },
        0,
    ));
    let ranked = match_no_yards(
        &model,
        &net,
        &[crossing_at(
            "RANK",
            [lat, lon],
            [10.0, 2.0, 4.0],
            Some(4.0),
            1,
        )],
        111,
        *b"US",
        &squares,
        &high_frt,
    );
    assert_eq!(ranked.len(), 1);
    let ranked_rt = ranked[0].1.resolved_section.unwrap();
    assert_eq!(
        ranked_rt.freight.source_id, 2079,
        "higher-rank freight is preserved"
    );
    assert!((claim_freight(&ranked[0].1) - 50.0).abs() < 1e-6);
    assert_eq!(
        ranked_rt.passenger.source_id, 111,
        "valid FRA passenger evidence is kept alongside higher-rank freight"
    );
    assert!((claim_passenger(&ranked[0].1) - 4.0).abs() < 1e-6);

    // 3. Unknown passenger_daily (None) without timetable keeps the measured total (16) split
    //    by the line's class-prior ratio (80/20 on main -> 12.8 pax + 3.2 freight), never 80+20.
    let ambig = match_no_yards(
        &model,
        &net,
        &[crossing_at("AMB", [lat, lon], [10.0, 2.0, 4.0], None, 1)],
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );
    assert_eq!(ambig.len(), 1);
    assert!((claim_passenger(&ambig[0].1) - 12.8).abs() < 1e-6);
    assert!((claim_freight(&ambig[0].1) - 3.2).abs() < 1e-6);

    // 4. Canadian edge never receives US source-111 claims.
    let mut ca_net = Network::default();
    let c0 = ca_net.nodes.len();
    ca_net.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    let c1 = ca_net.nodes.len();
    ca_net.nodes.push(meters_to_latlon(lat, lon, 0.0, 200.0));
    add_edge_full(
        &mut ca_net,
        [c0, c1],
        1,
        0,
        sq,
        [0.0, 200.0],
        0,
        *b"CA",
        "Main",
    );
    ca_net.index();
    let ca = match_no_yards(
        &model,
        &ca_net,
        &[crossing_at(
            "CA1",
            [lat, lon],
            [40.0, 10.0, 20.0],
            Some(0.0),
            1,
        )],
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );
    assert!(ca.is_empty());

    // 5. Astra Other #5: newly stamped yard track drops crossing traffic cleanly (0 pax, 0 frt).
    let yard_claims = match_no_yards(
        &model,
        &net,
        &[crossing_at(
            "YRD",
            [lat, lon],
            [5.0, 2.0, 3.0],
            Some(2.0),
            1,
        )],
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );
    assert_eq!(yard_claims.len(), 1);
    let (sgx, sgy) = lonlat_to_grid(net.nodes[a0][1], net.nodes[a0][0]);
    let (egx, egy) = lonlat_to_grid(net.nodes[a1][1], net.nodes[a1][0]);
    let schema = std::sync::Arc::new(arrow_schema::Schema::new(vec![
        arrow_schema::Field::new("osm_id", arrow_schema::DataType::Int64, false),
        arrow_schema::Field::new("segment_idx", arrow_schema::DataType::Int16, false),
        arrow_schema::Field::new("start_gx", arrow_schema::DataType::Int32, false),
        arrow_schema::Field::new("start_gy", arrow_schema::DataType::Int32, false),
        arrow_schema::Field::new("end_gx", arrow_schema::DataType::Int32, false),
        arrow_schema::Field::new("end_gy", arrow_schema::DataType::Int32, false),
        arrow_schema::Field::new("length_m", arrow_schema::DataType::Float32, false),
        arrow_schema::Field::new("rail_type", arrow_schema::DataType::UInt8, false),
        arrow_schema::Field::new("usage", arrow_schema::DataType::UInt8, false),
        arrow_schema::Field::new("service", arrow_schema::DataType::UInt8, false),
        arrow_schema::Field::new("country_iso", arrow_schema::DataType::UInt16, false),
        arrow_schema::Field::new("ref", arrow_schema::DataType::Utf8, true),
        arrow_schema::Field::new("name", arrow_schema::DataType::Utf8, true),
    ]));
    let batch = arrow_array::RecordBatch::try_new(
        schema,
        vec![
            std::sync::Arc::new(arrow_array::Int64Array::from(vec![1i64])),
            std::sync::Arc::new(arrow_array::Int16Array::from(vec![0i16])),
            std::sync::Arc::new(arrow_array::Int32Array::from(vec![sgx])),
            std::sync::Arc::new(arrow_array::Int32Array::from(vec![sgy])),
            std::sync::Arc::new(arrow_array::Int32Array::from(vec![egx])),
            std::sync::Arc::new(arrow_array::Int32Array::from(vec![egy])),
            std::sync::Arc::new(arrow_array::Float32Array::from(vec![200.0f32])),
            std::sync::Arc::new(arrow_array::UInt8Array::from(vec![0u8])),
            std::sync::Arc::new(arrow_array::UInt8Array::from(vec![3u8])),
            std::sync::Arc::new(arrow_array::UInt8Array::from(vec![1u8])), // stamped service=yard
            std::sync::Arc::new(arrow_array::UInt16Array::from(vec![u16::from_le_bytes(
                *b"US",
            )])),
            std::sync::Arc::new(arrow_array::StringArray::from(vec![""])),
            std::sync::Arc::new(arrow_array::StringArray::from(vec![""])),
        ],
    )
    .unwrap();
    let intervals = HashMap::from([((1i64, 0i16), vec![yard_claims[0].1.clone()])]);
    let expanded = crate::railways::write::expand_rows(
        &model,
        &batch,
        &intervals,
        &HashMap::new(),
        Square { x: sq[0], y: sq[1] },
    )
    .unwrap();
    assert_eq!(expanded.len(), 1);
    assert_eq!(expanded[0].child.traffic.passenger.periods, [0.0; 3]);
    assert_eq!(expanded[0].child.traffic.freight.periods, [0.0; 3]);
    assert_eq!(expanded[0].child.traffic.freight.source_id, 0);
}

#[test]
fn lirr_close_twins_and_lehigh_ref_name_match_pair_cleanly() {
    let model = test_model();
    let (lat, lon) = (40.70, -73.80);
    let sq = [151, 192];
    let squares = HashSet::from([sq]);

    // LIRR: two same-name tracks 2.36 m apart with 95.125 timetable pax and 62 FRA total ->
    // 47.5625 pax (source 2079) + 0 freight on BOTH tracks, never 95.125 + 62.
    let mut lirr = Network::default();
    let a0 = lirr.nodes.len();
    lirr.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    let a1 = lirr.nodes.len();
    lirr.nodes.push(meters_to_latlon(lat, lon, 0.0, 200.0));
    let b0 = lirr.nodes.len();
    lirr.nodes.push(meters_to_latlon(lat, lon, 2.36, 0.0));
    let b1 = lirr.nodes.len();
    lirr.nodes.push(meters_to_latlon(lat, lon, 2.36, 200.0));
    add_edge_named(&mut lirr, [a0, a1], 227618293, 24, sq, [0.0, 200.0], "LIRR");
    add_edge_named(&mut lirr, [b0, b1], 29469598, 18, sq, [0.0, 200.0], "LIRR");
    lirr.index();
    let mut existing = Claims::new();
    existing
        .entry(sq)
        .or_default()
        .push((timetable_iv(227618293, 24, [0.0, 200.0], 2079, 95.125), 0));
    let lirr_claims = match_no_yards(
        &model,
        &lirr,
        &[crossing_at(
            "LIRR",
            meters_to_latlon(lat, lon, 0.0, 100.0),
            [36.0, 12.0, 14.0],
            Some(62.0),
            2,
        )],
        111,
        *b"US",
        &squares,
        &existing,
    );
    assert_eq!(lirr_claims.len(), 2);
    for (_, iv) in &lirr_claims {
        let rt = iv.resolved_section.unwrap();
        assert_eq!(rt.passenger.source_id, 2079);
        assert!((claim_passenger(iv) - 47.5625).abs() < 1e-6);
        assert_eq!(claim_freight(iv), 0.0);
    }

    // Lehigh Line: bridge 150705682 has ref="LEHL", name="Lehigh Line"; twin 150705659 (4.5 m
    // away) has ref="", name="Lehigh Line". They must pair into one 2-track section.
    let mut lehigh = Network::default();
    let c0 = lehigh.nodes.len();
    lehigh.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    let c1 = lehigh.nodes.len();
    lehigh.nodes.push(meters_to_latlon(lat, lon, 0.0, 80.0));
    let d0 = lehigh.nodes.len();
    lehigh.nodes.push(meters_to_latlon(lat, lon, 4.5, 0.0));
    let d1 = lehigh.nodes.len();
    lehigh.nodes.push(meters_to_latlon(lat, lon, 4.5, 80.0));
    add_edge_named(&mut lehigh, [c0, c1], 150705682, 0, sq, [0.0, 80.0], "LEHL");
    lehigh.edges[0].corridor_name = "Lehigh Line".into();
    add_edge_named(
        &mut lehigh,
        [d0, d1],
        150705659,
        0,
        sq,
        [0.0, 80.0],
        "Lehigh Line",
    );
    lehigh.index();
    let mut lh_claims = Claims::new();
    lh_claims
        .entry(sq)
        .or_default()
        .push((timetable_iv(150705659, 0, [0.0, 80.0], 2079, 47.27), 0));
    let lh_out = match_no_yards(
        &model,
        &lehigh,
        &[crossing_at(
            "LEHL",
            meters_to_latlon(lat, lon, 4.5, 40.0),
            [20.0, 10.0, 10.0],
            Some(40.0),
            2,
        )],
        111,
        *b"US",
        &squares,
        &lh_claims,
    );
    assert_eq!(lh_out.len(), 2);
    for (_, iv) in &lh_out {
        assert!((claim_passenger(iv) - 23.635).abs() < 1e-6);
        assert_eq!(claim_freight(iv), 0.0);
    }
}

#[test]
fn partial_overlap_cut_invariance_12deg_turnout_and_disconnected_16m() {
    let model = test_model();
    let (lat, lon) = (41.5, -93.6);
    let sq = [100, 100];
    let squares = HashSet::from([sq]);

    // 1. 200 m + 80 m overlap: uncut vs cut right through the middle of the overlap must match.
    let build_200_80 = |cut: bool| {
        let mut net = Network::default();
        let a0 = net.nodes.len();
        net.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
        let a2 = net.nodes.len();
        net.nodes.push(meters_to_latlon(lat, lon, 0.0, 200.0));
        let b0 = net.nodes.len();
        net.nodes.push(meters_to_latlon(lat, lon, 5.0, 60.0));
        let b2 = net.nodes.len();
        net.nodes.push(meters_to_latlon(lat, lon, 5.0, 140.0));
        if cut {
            let a1 = net.nodes.len();
            net.nodes.push(meters_to_latlon(lat, lon, 0.0, 100.0));
            let b1 = net.nodes.len();
            net.nodes.push(meters_to_latlon(lat, lon, 5.0, 100.0));
            add_edge(&mut net, a0, a1, 1, 0, sq, [0.0, 100.0]);
            add_edge(&mut net, a1, a2, 1, 1, sq, [100.0, 200.0]);
            add_edge(&mut net, b0, b1, 2, 0, sq, [0.0, 40.0]);
            add_edge(&mut net, b1, b2, 2, 1, sq, [40.0, 80.0]);
        } else {
            add_edge(&mut net, a0, a2, 1, 0, sq, [0.0, 200.0]);
            add_edge(&mut net, b0, b2, 2, 0, sq, [0.0, 80.0]);
        }
        net.index();
        // Crossing 40 on the 1-track west approach: 40 on 0..60 and 140..200, 20+20=40 on 60..140.
        match_no_yards(
            &model,
            &net,
            &[crossing_at(
                "P40",
                meters_to_latlon(lat, lon, 0.0, 20.0),
                [20.0, 8.0, 12.0],
                Some(0.0),
                1,
            )],
            111,
            *b"US",
            &squares,
            &Claims::new(),
        )
    };
    let frt_at = |claims: &[([u16; 2], Interval)], way: i64, m: f64| -> f64 {
        claims
            .iter()
            .find(|(_, iv)| iv.osm_id == way && iv.from_m <= m && m <= iv.to_m)
            .map(|(_, iv)| claim_freight(iv))
            .unwrap()
    };
    let uncut = build_200_80(false);
    let cut = build_200_80(true);
    for (way, m, expected) in [
        (1, 30.0, 40.0),
        (1, 80.0, 20.0),
        (1, 120.0, 20.0),
        (1, 170.0, 40.0),
        (2, 20.0, 20.0),
        (2, 60.0, 20.0),
    ] {
        assert!((frt_at(&uncut, way, m) - expected).abs() < 1e-6);
        assert!((frt_at(&cut, way, m) - expected).abs() < 1e-6);
    }

    // 2. 12-degree named turnout split into two fragments still triggers the run-level 10-deg gate.
    let mut turn = Network::default();
    let t0 = turn.nodes.len();
    turn.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    let t1 = turn.nodes.len();
    turn.nodes.push(meters_to_latlon(lat, lon, 0.0, 200.0));
    let rad = 12.0_f64.to_radians();
    let br_mid = turn.nodes.len();
    turn.nodes.push(meters_to_latlon(
        lat,
        lon,
        50.0 * rad.sin(),
        50.0 * rad.cos(),
    ));
    let br_end = turn.nodes.len();
    turn.nodes.push(meters_to_latlon(
        lat,
        lon,
        150.0 * rad.sin(),
        150.0 * rad.cos(),
    ));
    add_edge_named(&mut turn, [t0, t1], 10, 0, sq, [0.0, 200.0], "Main");
    add_edge_named(&mut turn, [t0, br_mid], 11, 0, sq, [0.0, 50.0], "Main");
    add_edge_named(
        &mut turn,
        [br_mid, br_end],
        11,
        1,
        sq,
        [50.0, 150.0],
        "Main",
    );
    turn.index();
    let turn_claims = match_no_yards(
        &model,
        &turn,
        &[crossing_at(
            "T12",
            meters_to_latlon(lat, lon, 0.0, 100.0),
            [20.0, 0.0, 0.0],
            Some(0.0),
            1,
        )],
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );
    assert_eq!(turn_claims.len(), 1);
    assert_eq!(turn_claims[0].1.osm_id, 10);
    assert!((claim_freight(&turn_claims[0].1) - 20.0).abs() < 1e-6);

    // 3. Two disconnected 16 m overlaps (50 m apart) on long runs must NOT pass the 30 m bar.
    let mut scraps = Network::default();
    let m0 = scraps.nodes.len();
    scraps.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    let m1 = scraps.nodes.len();
    scraps.nodes.push(meters_to_latlon(lat, lon, 0.0, 200.0));
    // Track 2 comes within 10 m on y=0..16, swings out to 30 m on y=16..100, returns on y=100..116.
    let s0 = scraps.nodes.len();
    scraps.nodes.push(meters_to_latlon(lat, lon, 10.0, 0.0));
    let s1 = scraps.nodes.len();
    scraps.nodes.push(meters_to_latlon(lat, lon, 10.0, 16.0));
    let s2 = scraps.nodes.len();
    scraps.nodes.push(meters_to_latlon(lat, lon, 30.0, 50.0));
    let s3 = scraps.nodes.len();
    scraps.nodes.push(meters_to_latlon(lat, lon, 30.0, 80.0));
    let s4 = scraps.nodes.len();
    scraps.nodes.push(meters_to_latlon(lat, lon, 10.0, 100.0));
    let s5 = scraps.nodes.len();
    scraps.nodes.push(meters_to_latlon(lat, lon, 10.0, 116.0));
    add_edge_named(&mut scraps, [m0, m1], 1, 0, sq, [0.0, 200.0], "");
    add_edge_named(&mut scraps, [s0, s1], 2, 0, sq, [0.0, 16.0], "");
    add_edge_named(&mut scraps, [s1, s2], 2, 1, sq, [16.0, 56.0], "");
    add_edge_named(&mut scraps, [s2, s3], 2, 2, sq, [56.0, 86.0], "");
    add_edge_named(&mut scraps, [s3, s4], 2, 3, sq, [86.0, 114.0], "");
    add_edge_named(&mut scraps, [s4, s5], 2, 4, sq, [114.0, 130.0], "");
    scraps.index();
    let scrap_claims = match_no_yards(
        &model,
        &scraps,
        &[crossing_at(
            "SCRAP",
            meters_to_latlon(lat, lon, 0.0, 10.0),
            [20.0, 0.0, 0.0],
            Some(0.0),
            1,
        )],
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );
    assert_eq!(scrap_claims.len(), 1);
    assert!((claim_freight(&scrap_claims[0].1) - 20.0).abs() < 1e-6);
}

#[test]
fn inclined_middle_track_and_sibling_timetable_cut_resolve_consistently() {
    let model = test_model();
    let (lat, lon) = (41.4, -87.6);
    let sq = [131, 191];
    let squares = HashSet::from([sq]);

    // Astra #1: two horizontal 80 m tracks at y=0 and y=12 m, middle track inclined 5 deg through
    // midpoint (40, 6). Timetable cut at x=39.75 on Track 3 splits ALL THREE tracks identically.
    let mut net = Network::default();
    let a0 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    let a1 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 80.0, 0.0));
    let dy = 40.0 * 5.0_f64.to_radians().tan();
    let b0 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 6.0 - dy));
    let b1 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 80.0, 6.0 + dy));
    let c0 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 12.0));
    let c1 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 80.0, 12.0));
    add_edge_named(&mut net, [a0, a1], 1, 0, sq, [0.0, 80.0], "");
    add_edge_named(
        &mut net,
        [b0, b1],
        2,
        0,
        sq,
        [0.0, (80.0_f64).hypot(2.0 * dy)],
        "",
    );
    add_edge_named(&mut net, [c0, c1], 3, 0, sq, [0.0, 80.0], "");
    net.index();

    let mut existing = Claims::new();
    existing
        .entry(sq)
        .or_default()
        .push((timetable_iv(3, 0, [0.0, 39.75], 2079, 30.0), 0));
    let claims = match_no_yards(
        &model,
        &net,
        &[crossing_at(
            "INC5",
            meters_to_latlon(lat, lon, 20.0, 0.0),
            [30.0, 15.0, 15.0],
            Some(0.0),
            3,
        )],
        111,
        *b"US",
        &squares,
        &existing,
    );
    for way in [1i64, 2, 3] {
        let mut w_claims: Vec<&Interval> = claims
            .iter()
            .filter(|(_, iv)| iv.osm_id == way)
            .map(|(_, iv)| iv)
            .collect();
        w_claims.sort_by(|a, b| a.from_m.total_cmp(&b.from_m));
        assert_eq!(w_claims.len(), 2, "way {way} must split once at 39.75");
        assert!((claim_passenger(w_claims[0]) - 10.0).abs() < 1e-6);
        assert!((claim_freight(w_claims[0]) - 10.0).abs() < 1e-6);
        assert_eq!(claim_passenger(w_claims[1]), 0.0);
        assert!((claim_freight(w_claims[1]) - 20.0).abs() < 1e-6);
    }
}

#[test]
fn section_first_direct_totals_and_single_way_balloon_loop_conserve() {
    let model = test_model();
    let (lat, lon) = (41.5, -93.6);
    let sq = [100, 100];
    let squares = HashSet::from([sq]);

    // Astra #2: A alone reports 30, B alone reports 10, shared A+B reports 40 ->
    // A alone = 30, B alone = 10, shared A+B = 20 + 20 = 40 (never 25 + 25 = 50).
    let mut net = Network::default();
    let a0 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    let a1 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 200.0));
    let b0 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 5.0, 100.0));
    let b1 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 5.0, 300.0));
    add_edge(&mut net, a0, a1, 1, 0, sq, [0.0, 200.0]);
    add_edge(&mut net, b0, b1, 2, 0, sq, [0.0, 200.0]);
    net.index();
    let claims = match_no_yards(
        &model,
        &net,
        &[
            crossing_at(
                "A30",
                meters_to_latlon(lat, lon, 0.0, 50.0),
                [15.0, 5.0, 10.0],
                Some(0.0),
                1,
            ),
            crossing_at(
                "B10",
                meters_to_latlon(lat, lon, 5.0, 250.0),
                [5.0, 2.0, 3.0],
                Some(0.0),
                1,
            ),
            crossing_at(
                "AB40",
                meters_to_latlon(lat, lon, 0.0, 150.0),
                [20.0, 8.0, 12.0],
                Some(0.0),
                2,
            ),
        ],
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );
    let at = |way: i64, m: f64| {
        claims
            .iter()
            .find(|(_, iv)| iv.osm_id == way && iv.from_m <= m && m <= iv.to_m)
            .map(|(_, iv)| claim_freight(iv))
            .unwrap()
    };
    assert!((at(1, 50.0) - 30.0).abs() < 1e-6);
    assert!((at(1, 150.0) - 20.0).abs() < 1e-6);
    assert!((at(2, 50.0) - 20.0).abs() < 1e-6);
    assert!((at(2, 150.0) - 10.0).abs() < 1e-6);

    // Single-way balloon loop: one way (osm_id=99) folding back 5 m apart over a 100 m neck
    // joined by rounded 90-degree-per-step end loops (one continuous degree-2 closed run).
    let mut bl = Network::default();
    let pts = [(0.0, 0.0), (0.0, 100.0), (5.0, 100.0), (5.0, 0.0)];
    let nodes: Vec<usize> = pts
        .iter()
        .map(|&(x, y)| {
            let id = bl.nodes.len();
            bl.nodes.push(meters_to_latlon(lat, lon, x, y));
            id
        })
        .collect();
    let seg_lens = [100.0, 5.0, 100.0, 5.0];
    let mut cum = 0.0;
    for (seg, &len) in seg_lens.iter().enumerate() {
        add_edge(
            &mut bl,
            nodes[seg],
            nodes[(seg + 1) % nodes.len()],
            99,
            seg as i16,
            sq,
            [cum, cum + len],
        );
        cum += len;
    }
    bl.index();
    let bl_layout = CorridorLayout::build(&bl, *b"US", &HashSet::new());
    assert_eq!(
        bl_layout.runs.len(),
        1,
        "balloon loop must stay one closed run"
    );

    // Case A: crossing 20 on neck only -> 10 on each leg AND 10 on the curve.
    let neck_only = match_no_yards(
        &model,
        &bl,
        &[crossing_at(
            "NECK",
            meters_to_latlon(lat, lon, 0.0, 50.0),
            [10.0, 4.0, 6.0],
            Some(0.0),
            1,
        )],
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );
    for (_, iv) in &neck_only {
        assert!(
            (claim_freight(iv) - 10.0).abs() < 1e-6,
            "seg {}",
            iv.segment_idx
        );
    }
    assert_eq!(neck_only.len(), 4);

    // Case B: crossing 10 on curve only -> 10 on curve AND 10+10=20 on neck.
    let curve_only = match_no_yards(
        &model,
        &bl,
        &[crossing_at(
            "CURVE",
            meters_to_latlon(lat, lon, 2.5, 100.0),
            [5.0, 2.0, 3.0],
            Some(0.0),
            1,
        )],
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );
    for (_, iv) in &curve_only {
        assert!(
            (claim_freight(iv) - 10.0).abs() < 1e-6,
            "seg {}",
            iv.segment_idx
        );
    }
    assert_eq!(curve_only.len(), 4);

    // Case C: crossing 20 on neck AND crossing 10 on curve -> 10 everywhere.
    let both = match_no_yards(
        &model,
        &bl,
        &[
            crossing_at(
                "NECK",
                meters_to_latlon(lat, lon, 0.0, 50.0),
                [10.0, 4.0, 6.0],
                Some(0.0),
                1,
            ),
            crossing_at(
                "CURVE",
                meters_to_latlon(lat, lon, 2.5, 100.0),
                [5.0, 2.0, 3.0],
                Some(0.0),
                1,
            ),
        ],
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );
    for (_, iv) in &both {
        assert!(
            (claim_freight(iv) - 10.0).abs() < 1e-6,
            "seg {}",
            iv.segment_idx
        );
    }
    assert_eq!(both.len(), 4);
}

#[test]
fn competing_names_resolve_locally_and_yard_prepass_excludes_yard_tracks() {
    let model = test_model();
    let (lat, lon) = (41.4, -87.6);
    let sq = [131, 191];
    let squares = HashSet::from([sq]);

    // 1. Concurrent A (0 m) / unnamed U (8 m) / C (20 m): U joins nearer A ({A, U} gets 30+30=60; C untouched).
    let mut conc = Network::default();
    for (way, x, name) in [(1i64, 0.0, "A"), (2, 8.0, ""), (3, 20.0, "C")] {
        let n0 = conc.nodes.len();
        conc.nodes.push(meters_to_latlon(lat, lon, x, 0.0));
        let n1 = conc.nodes.len();
        conc.nodes.push(meters_to_latlon(lat, lon, x, 100.0));
        add_edge_named(&mut conc, [n0, n1], way, 0, sq, [0.0, 100.0], name);
    }
    conc.index();
    let conc_claims = match_no_yards(
        &model,
        &conc,
        &[crossing_at(
            "AUC",
            meters_to_latlon(lat, lon, 0.0, 50.0),
            [30.0, 10.0, 20.0],
            Some(0.0),
            2,
        )],
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );
    let by_way: HashMap<i64, f64> = conc_claims
        .iter()
        .map(|(_, iv)| (iv.osm_id, claim_freight(iv)))
        .collect();
    assert!((by_way[&1] - 30.0).abs() < 1e-6);
    assert!((by_way[&2] - 30.0).abs() < 1e-6);
    assert!(!by_way.contains_key(&3));

    // 2. Unnamed U runs beside A in the west (y=0..100) and beside C in the east (y=100..200).
    let mut we = Network::default();
    let u0 = we.nodes.len();
    we.nodes.push(meters_to_latlon(lat, lon, 8.0, 0.0));
    let u1 = we.nodes.len();
    we.nodes.push(meters_to_latlon(lat, lon, 8.0, 200.0));
    let a0 = we.nodes.len();
    we.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    let a1 = we.nodes.len();
    we.nodes.push(meters_to_latlon(lat, lon, 0.0, 100.0));
    let c0 = we.nodes.len();
    we.nodes.push(meters_to_latlon(lat, lon, 16.0, 100.0));
    let c1 = we.nodes.len();
    we.nodes.push(meters_to_latlon(lat, lon, 16.0, 200.0));
    add_edge_named(&mut we, [u0, u1], 1, 0, sq, [0.0, 200.0], "");
    add_edge_named(&mut we, [a0, a1], 2, 0, sq, [0.0, 100.0], "A");
    add_edge_named(&mut we, [c0, c1], 3, 0, sq, [0.0, 100.0], "C");
    we.index();
    let we_claims = match_no_yards(
        &model,
        &we,
        &[
            crossing_at(
                "WEST",
                meters_to_latlon(lat, lon, 0.0, 50.0),
                [20.0, 0.0, 0.0],
                Some(0.0),
                2,
            ),
            crossing_at(
                "EAST",
                meters_to_latlon(lat, lon, 16.0, 150.0),
                [40.0, 0.0, 0.0],
                Some(0.0),
                2,
            ),
        ],
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );
    let we_at = |way: i64, m: f64| {
        we_claims
            .iter()
            .find(|(_, iv)| iv.osm_id == way && iv.from_m <= m && m <= iv.to_m)
            .map(|(_, iv)| claim_freight(iv))
            .unwrap()
    };
    assert!((we_at(2, 50.0) - 10.0).abs() < 1e-6);
    assert!((we_at(1, 50.0) - 10.0).abs() < 1e-6);
    assert!((we_at(1, 150.0) - 20.0).abs() < 1e-6);
    assert!((we_at(3, 50.0) - 20.0).abs() < 1e-6);

    // 3. Yard track excluded before allocation: main keeps full 40, yard track gets nothing.
    let mut yd = Network::default();
    let m0 = yd.nodes.len();
    yd.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    let m1 = yd.nodes.len();
    yd.nodes.push(meters_to_latlon(lat, lon, 0.0, 100.0));
    let y0 = yd.nodes.len();
    yd.nodes.push(meters_to_latlon(lat, lon, 4.0, 0.0));
    let y1 = yd.nodes.len();
    yd.nodes.push(meters_to_latlon(lat, lon, 4.0, 100.0));
    add_edge_full(
        &mut yd,
        [m0, m1],
        10,
        0,
        sq,
        [0.0, 100.0],
        0,
        *b"US",
        "Main",
    );
    add_edge_full(&mut yd, [y0, y1], 11, 0, sq, [0.0, 100.0], 3, *b"US", "");
    yd.index();
    let yard_keys = HashSet::from([(sq, 11i64, 0i16)]);
    let yd_claims = match_crossings(
        &model,
        &yd,
        &[crossing_at(
            "YD40",
            meters_to_latlon(lat, lon, 0.0, 50.0),
            [20.0, 8.0, 12.0],
            Some(0.0),
            1,
        )],
        111,
        *b"US",
        &squares,
        &Claims::new(),
        &yard_keys,
    );
    assert_eq!(yd_claims.len(), 1);
    assert_eq!(yd_claims[0].1.osm_id, 10);
    assert!((claim_freight(&yd_claims[0].1) - 40.0).abs() < 1e-6);
}

#[test]
fn unknown_split_lt1_cap_same_total_borrowing_known_floor_and_period_waterfilling() {
    let model = test_model();
    let (lat, lon) = (35.6, -106.0);
    let sq = [105, 201];
    let squares = HashSet::from([sq]);

    // 1. Period water-filling: 719605C [1.8333, 0, 0.1667] with 1 pax -> [0.875, 0, 0.125] (zero eve)
    //    and overflow cap [1.95, 0, 0.05] with 1 pax -> [0.95, 0, 0.05].
    let p719 = allocate_passenger_in_periods([11.0 / 6.0, 0.0, 1.0 / 6.0], 1.0, [0.7, 0.2, 0.1]);
    assert!((p719[0] - 0.875).abs() < 1e-9);
    assert_eq!(p719[1], 0.0);
    assert!((p719[2] - 0.125).abs() < 1e-9);
    let pov = allocate_passenger_in_periods([1.95, 0.0, 0.05], 1.0, [0.7, 0.2, 0.1]);
    assert!((pov[0] - 0.95).abs() < 1e-9);
    assert_eq!(pov[1], 0.0);
    assert!((pov[2] - 0.05).abs() < 1e-9);

    // 2. 013800Y: 1/7 weekly branch + LT1 -> ~0.1224 pax + ~0.0204 freight (never 30+5).
    let mut branch = Network::default();
    let b0 = branch.nodes.len();
    branch.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    let b1 = branch.nodes.len();
    branch.nodes.push(meters_to_latlon(lat, lon, 0.0, 200.0));
    add_edge_full(
        &mut branch,
        [b0, b1],
        14605191,
        0,
        sq,
        [0.0, 200.0],
        1,
        *b"US",
        "",
    );
    branch.index();
    let mut c_sfs = crossing_at(
        "013800Y",
        meters_to_latlon(lat, lon, 0.0, 100.0),
        [0.5 / 7.0, 1.0 / 42.0, 2.0 / 42.0],
        None,
        1,
    );
    c_sfs.measured_periods = false;
    c_sfs.passenger_lt1 = true;
    let sfs_claims = match_no_yards(
        &model,
        &branch,
        &[c_sfs],
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );
    assert_eq!(sfs_claims.len(), 1);
    assert!((claim_passenger(&sfs_claims[0].1) - (1.0 / 7.0) * (30.0 / 35.0)).abs() < 1e-6);
    assert!((claim_freight(&sfs_claims[0].1) - (1.0 / 7.0) * (5.0 / 35.0)).abs() < 1e-6);

    // 3. 028285V: 18/day on 2-track main + LT1=Yes -> crossing pax < 1 (0.80 pax + 17.20 frt),
    //    so each track gets 0.40 pax (< 0.5) + 8.60 frt.
    let mut main2 = Network::default();
    for (way, x) in [(1i64, 0.0), (2, 5.0)] {
        let n0 = main2.nodes.len();
        main2.nodes.push(meters_to_latlon(lat, lon, x, 0.0));
        let n1 = main2.nodes.len();
        main2.nodes.push(meters_to_latlon(lat, lon, x, 200.0));
        add_edge(&mut main2, n0, n1, way, 0, sq, [0.0, 200.0]);
    }
    main2.index();
    let mut c_lt1 = crossing_at(
        "028285V",
        meters_to_latlon(lat, lon, 0.0, 100.0),
        [9.0, 3.0, 6.0],
        None,
        2,
    );
    c_lt1.passenger_lt1 = true;
    let lt1_claims = match_no_yards(
        &model,
        &main2,
        &[c_lt1],
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );
    assert_eq!(lt1_claims.len(), 2);
    for (_, iv) in &lt1_claims {
        assert!((claim_passenger(iv) - 0.4).abs() < 1e-6);
        assert!((claim_freight(iv) - 8.6).abs() < 1e-6);
    }

    // 4. Same-total borrowing: [unk, unk, Some(2)] at total=10 -> 2 pax + 8 frt;
    //    unequal totals [Some(2) at tot=2, unk at tot=18] must NOT borrow 100% onto 18!
    let mut single = Network::default();
    let s0 = single.nodes.len();
    single.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    let s1 = single.nodes.len();
    single.nodes.push(meters_to_latlon(lat, lon, 0.0, 200.0));
    add_edge(&mut single, s0, s1, 5, 0, sq, [0.0, 200.0]);
    single.index();
    let eq_claims = match_no_yards(
        &model,
        &single,
        &[
            crossing_at(
                "U1",
                meters_to_latlon(lat, lon, 0.0, 40.0),
                [6.0, 1.0, 3.0],
                None,
                1,
            ),
            crossing_at(
                "U2",
                meters_to_latlon(lat, lon, 0.0, 80.0),
                [6.0, 1.0, 3.0],
                None,
                1,
            ),
            crossing_at(
                "K2",
                meters_to_latlon(lat, lon, 0.0, 120.0),
                [6.0, 1.0, 3.0],
                Some(2.0),
                1,
            ),
        ],
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );
    assert!((claim_passenger(&eq_claims[0].1) - 2.0).abs() < 1e-6);
    assert!((claim_freight(&eq_claims[0].1) - 8.0).abs() < 1e-6);

    let mut c_lt1_18 = crossing_at(
        "U18",
        meters_to_latlon(lat, lon, 0.0, 120.0),
        [9.0, 3.0, 6.0],
        None,
        1,
    );
    c_lt1_18.passenger_lt1 = true;
    let uneq_claims = match_no_yards(
        &model,
        &single,
        &[
            crossing_at(
                "P2",
                meters_to_latlon(lat, lon, 0.0, 40.0),
                [2.0, 0.0, 0.0],
                Some(2.0),
                1,
            ),
            c_lt1_18,
        ],
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );
    assert!((claim_passenger(&uneq_claims[0].1) - 0.8).abs() < 1e-6);
    assert!((claim_freight(&uneq_claims[0].1) - 17.2).abs() < 1e-6);

    // 5. Known passenger floor without `all_carriers_known`:
    //    R1 alone carries 10 (8 known pax), R2 alone carries 10 (unknown pax), shared R1+R2 has
    //    timetable 2 pax -> freight on R1+R2 is capped at 20 - max(2, 8) = 12 (6/track), never 18.
    let mut fl = Network::default();
    let r1a = fl.nodes.len();
    fl.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    let r1b = fl.nodes.len();
    fl.nodes.push(meters_to_latlon(lat, lon, 0.0, 200.0));
    let r2a = fl.nodes.len();
    fl.nodes.push(meters_to_latlon(lat, lon, 5.0, 100.0));
    let r2b = fl.nodes.len();
    fl.nodes.push(meters_to_latlon(lat, lon, 5.0, 300.0));
    add_edge(&mut fl, r1a, r1b, 1, 0, sq, [0.0, 200.0]);
    add_edge(&mut fl, r2a, r2b, 2, 0, sq, [0.0, 200.0]);
    fl.index();
    let mut tt2 = Claims::new();
    tt2.entry(sq)
        .or_default()
        .push((timetable_iv(1, 0, [100.0, 200.0], 2079, 2.0), 0));
    let fl_claims = match_no_yards(
        &model,
        &fl,
        &[
            crossing_at(
                "R1",
                meters_to_latlon(lat, lon, 0.0, 50.0),
                [6.0, 2.0, 2.0],
                Some(8.0),
                1,
            ),
            crossing_at(
                "R2",
                meters_to_latlon(lat, lon, 5.0, 250.0),
                [6.0, 2.0, 2.0],
                None,
                1,
            ),
        ],
        111,
        *b"US",
        &squares,
        &tt2,
    );
    let shared_frt: f64 = fl_claims
        .iter()
        .filter(|(_, iv)| {
            (iv.osm_id == 1 && iv.from_m >= 99.0) || (iv.osm_id == 2 && iv.to_m <= 101.0)
        })
        .map(|(_, iv)| claim_freight(iv))
        .sum();
    assert!((shared_frt - 12.0).abs() < 1e-6, "shared_frt={shared_frt}");
}

#[test]
fn final_review_allocator_and_corridor_regressions() {
    let model = test_model();
    let (lat, lon) = (40.0, -74.0);
    let sq = [150, 193];
    let squares = HashSet::from([sq]);

    // 1. Oblique sibling timetable cut stays at its exact originating coordinate (no round-trip shift).
    let mut obl = Network::default();
    let angle = 5.0_f64.to_radians();
    let a0 = obl.nodes.len();
    obl.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    let a1 = obl.nodes.len();
    obl.nodes.push(meters_to_latlon(lat, lon, 0.0, 200.0));
    let b0 = obl.nodes.len();
    obl.nodes.push(meters_to_latlon(lat, lon, 10.0, 0.0));
    let b1 = obl.nodes.len();
    obl.nodes.push(meters_to_latlon(
        lat,
        lon,
        10.0 + 200.0 * angle.sin(),
        200.0 * angle.cos(),
    ));
    add_edge(&mut obl, a0, a1, 1, 0, sq, [0.0, 200.0]);
    add_edge(&mut obl, b0, b1, 2, 0, sq, [0.0, 200.0]);
    obl.index();
    let mut obl_tt = Claims::new();
    obl_tt
        .entry(sq)
        .or_default()
        .push((timetable_iv(2, 0, [0.0, 100.0], 2079, 10.0), 0));
    let obl_claims = match_no_yards(
        &model,
        &obl,
        &[crossing_at(
            "OBL",
            meters_to_latlon(lat, lon, 0.0, 50.0),
            [12.0, 4.0, 4.0],
            Some(0.0),
            2,
        )],
        111,
        *b"US",
        &squares,
        &obl_tt,
    );
    let leg2_tt = obl_claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 2 && iv.resolved_section.unwrap().passenger.source_id == 2079)
        .expect("leg 2 timetable interval");
    assert!(
        (leg2_tt.1.to_m - 100.0).abs() < 1e-4,
        "sibling cut drifted to {}",
        leg2_tt.1.to_m
    );

    // 2. Equal-total ties sort by passenger count (not crossing ID), and LT1 never borrows >= 1.
    let mut single = Network::default();
    let s0 = single.nodes.len();
    single.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    let s1 = single.nodes.len();
    single.nodes.push(meters_to_latlon(lat, lon, 0.0, 200.0));
    add_edge(&mut single, s0, s1, 1, 0, sq, [0.0, 200.0]);
    single.index();
    let id_order = match_no_yards(
        &model,
        &single,
        &[
            crossing_at(
                "A",
                meters_to_latlon(lat, lon, 0.0, 40.0),
                [9.0, 3.0, 6.0],
                Some(0.0),
                1,
            ),
            crossing_at(
                "B",
                meters_to_latlon(lat, lon, 0.0, 100.0),
                [9.0, 3.0, 6.0],
                Some(12.0),
                1,
            ),
            crossing_at(
                "C",
                meters_to_latlon(lat, lon, 0.0, 160.0),
                [9.0, 3.0, 6.0],
                Some(0.0),
                1,
            ),
        ],
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );
    assert!(claim_passenger(&id_order[0].1).abs() < 1e-6);
    assert!((claim_freight(&id_order[0].1) - 18.0).abs() < 1e-6);

    let mut lt1_a = crossing_at(
        "L1",
        meters_to_latlon(lat, lon, 0.0, 40.0),
        [9.0, 3.0, 6.0],
        None,
        1,
    );
    lt1_a.passenger_lt1 = true;
    let mut lt1_b = crossing_at(
        "L2",
        meters_to_latlon(lat, lon, 0.0, 100.0),
        [9.0, 3.0, 6.0],
        None,
        1,
    );
    lt1_b.passenger_lt1 = true;
    let lt1_claims = match_no_yards(
        &model,
        &single,
        &[
            lt1_a,
            lt1_b,
            crossing_at(
                "P2",
                meters_to_latlon(lat, lon, 0.0, 160.0),
                [9.0, 3.0, 6.0],
                Some(2.0),
                1,
            ),
        ],
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );
    assert!(claim_passenger(&lt1_claims[0].1) < 1.0);
    assert!((claim_passenger(&lt1_claims[0].1) - 0.8).abs() < 1e-6);

    // Skewed-total Swanton case: 5x(2,0) + 2x(2,2) + 3x(4,0) -> median total is 2 and median
    // within total==2 is pax=0 (must not pick the minority pax=2 pushed to the upper boundary).
    let mut skewed = Vec::new();
    for i in 0..5 {
        skewed.push(crossing_at(
            &format!("F{i}"),
            meters_to_latlon(lat, lon, 0.0, 20.0 + i as f64 * 10.0),
            [1.0, 0.0, 1.0],
            Some(0.0),
            1,
        ));
    }
    for i in 0..2 {
        skewed.push(crossing_at(
            &format!("P{i}"),
            meters_to_latlon(lat, lon, 0.0, 80.0 + i as f64 * 10.0),
            [1.0, 0.0, 1.0],
            Some(2.0),
            1,
        ));
    }
    for i in 0..3 {
        skewed.push(crossing_at(
            &format!("H{i}"),
            meters_to_latlon(lat, lon, 0.0, 110.0 + i as f64 * 10.0),
            [2.0, 0.0, 2.0],
            Some(0.0),
            1,
        ));
    }
    let skewed_claims = match_no_yards(
        &model,
        &single,
        &skewed,
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );
    assert!(claim_passenger(&skewed_claims[0].1).abs() < 1e-6);
    assert!((claim_freight(&skewed_claims[0].1) - 2.0).abs() < 1e-6);

    // 3. Coincident duplicate OSM ways (UP Gila 98/207) share one physical track slot while a 5 m
    //    parallel track gets its full physical share (15.5 + (7.75 + 7.75) = 31.0).
    let mut dup = Network::default();
    for (way, x) in [(1i64, 0.0), (2, 0.0), (3, 5.0)] {
        let n0 = dup.nodes.len();
        dup.nodes.push(meters_to_latlon(lat, lon, x, 0.0));
        let n1 = dup.nodes.len();
        dup.nodes.push(meters_to_latlon(lat, lon, x, 200.0));
        add_edge(&mut dup, n0, n1, way, 0, sq, [0.0, 200.0]);
    }
    dup.index();
    let dup_claims = match_no_yards(
        &model,
        &dup,
        &[crossing_at(
            "GILA",
            meters_to_latlon(lat, lon, 0.0, 100.0),
            [15.0, 6.0, 10.0],
            Some(0.0),
            2,
        )],
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );
    let frt_way = |way: i64| {
        dup_claims
            .iter()
            .find(|(_, iv)| iv.osm_id == way)
            .map(|(_, iv)| claim_freight(iv))
            .unwrap()
    };
    assert!((frt_way(1) - 7.75).abs() < 1e-6);
    assert!((frt_way(2) - 7.75).abs() < 1e-6);
    assert!((frt_way(3) - 15.5).abs() < 1e-6);

    // 4. Passing siding (200 m + 80 m) with sole 40-train crossing INSIDE overlap conserves 40 on
    //    both single-track tails; river split (both legs continuing) still emits 20 on each tail.
    let mut sid = Network::default();
    let m0 = sid.nodes.len();
    sid.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    let m1 = sid.nodes.len();
    sid.nodes.push(meters_to_latlon(lat, lon, 0.0, 200.0));
    let p0 = sid.nodes.len();
    sid.nodes.push(meters_to_latlon(lat, lon, 5.0, 60.0));
    let p1 = sid.nodes.len();
    sid.nodes.push(meters_to_latlon(lat, lon, 5.0, 140.0));
    add_edge(&mut sid, m0, m1, 1, 0, sq, [0.0, 200.0]);
    add_edge(&mut sid, p0, p1, 2, 0, sq, [0.0, 80.0]);
    sid.index();
    let sid_claims = match_no_yards(
        &model,
        &sid,
        &[crossing_at(
            "IN",
            meters_to_latlon(lat, lon, 0.0, 100.0),
            [20.0, 8.0, 12.0],
            Some(0.0),
            2,
        )],
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );
    let sid_at = |way: i64, m: f64| {
        sid_claims
            .iter()
            .find(|(_, iv)| iv.osm_id == way && iv.from_m <= m && m <= iv.to_m)
            .map(|(_, iv)| claim_freight(iv))
            .unwrap()
    };
    assert!((sid_at(1, 30.0) - 40.0).abs() < 1e-6);
    assert!((sid_at(1, 100.0) - 20.0).abs() < 1e-6);
    assert!((sid_at(2, 40.0) - 20.0).abs() < 1e-6);
    assert!((sid_at(1, 170.0) - 40.0).abs() < 1e-6);

    // 5. Closed 40 m x 5 m rectangle with mid-side origin wraps seam on s_b -> 10 per leg.
    let mut rect = Network::default();
    let pts = [
        (0.0, 20.0),
        (0.0, 40.0),
        (5.0, 40.0),
        (5.0, 0.0),
        (0.0, 0.0),
    ];
    let rnodes: Vec<usize> = pts
        .iter()
        .map(|&(x, y)| {
            let id = rect.nodes.len();
            rect.nodes.push(meters_to_latlon(lat, lon, x, y));
            id
        })
        .collect();
    let mut off = 0.0;
    for (seg, &(i, j, len)) in [
        (0usize, 1usize, 20.0),
        (1, 2, 5.0),
        (2, 3, 40.0),
        (3, 4, 5.0),
        (4, 0, 20.0),
    ]
    .iter()
    .enumerate()
    {
        add_edge(
            &mut rect,
            rnodes[i],
            rnodes[j],
            1,
            seg as i16,
            sq,
            [off, off + len],
        );
        off += len;
    }
    rect.index();
    let rect_claims = match_no_yards(
        &model,
        &rect,
        &[crossing_at(
            "LP",
            meters_to_latlon(lat, lon, 0.0, 30.0),
            [10.0, 4.0, 6.0],
            Some(0.0),
            1,
        )],
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );
    for seg in [0i16, 2, 4] {
        let frt = rect_claims
            .iter()
            .find(|(_, iv)| iv.segment_idx == seg)
            .map(|(_, iv)| claim_freight(iv))
            .unwrap();
        assert!(
            (frt - 10.0).abs() < 1e-6,
            "mid-side rect seg={seg} frt={frt}"
        );
    }

    // 6. Mixed freight-only (mode=2) + mixed (mode=0) tracks with unknown split keep 0 pax on
    //    freight-only track and conserve total; higher-rank freight keeps measured FRA remainder.
    let mut tm = Network::default();
    let t0 = tm.nodes.len();
    tm.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    let t1 = tm.nodes.len();
    tm.nodes.push(meters_to_latlon(lat, lon, 0.0, 200.0));
    let f0 = tm.nodes.len();
    tm.nodes.push(meters_to_latlon(lat, lon, 5.0, 0.0));
    let f1 = tm.nodes.len();
    tm.nodes.push(meters_to_latlon(lat, lon, 5.0, 200.0));
    add_edge(&mut tm, t0, t1, 1, 0, sq, [0.0, 200.0]);
    add_edge(&mut tm, f0, f1, 2, 0, sq, [0.0, 200.0]);
    tm.edges[1].traffic_mode = 2; // freight-only
    tm.index();
    let tm_claims = match_no_yards(
        &model,
        &tm,
        &[crossing_at(
            "TRN",
            meters_to_latlon(lat, lon, 0.0, 100.0),
            [12.0, 4.0, 4.0],
            None,
            2,
        )],
        111,
        *b"US",
        &squares,
        &Claims::new(),
    );
    let leg1 = tm_claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 1)
        .unwrap()
        .1
        .clone();
    let leg2 = tm_claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 2)
        .unwrap()
        .1
        .clone();
    assert!(
        claim_passenger(&leg2).abs() < 1e-6,
        "freight-only leg got pax={}",
        claim_passenger(&leg2)
    );
    assert!((claim_passenger(&leg1) - 16.0).abs() < 1e-6);
    assert!((claim_freight(&leg1) - 2.0).abs() < 1e-6);
    assert!((claim_freight(&leg2) - 2.0).abs() < 1e-6);

    let mut hf = Claims::new();
    hf.entry(sq).or_default().push((
        Interval {
            missing_calendar_estimate: [0.0; 2],
            osm_id: 1,
            segment_idx: 0,
            from_m: 0.0,
            to_m: 200.0,
            country: *b"US",
            source_id: 2079,
            passenger: 0.0,
            freight: 12.0,
            passenger_status: STATUS_UNKNOWN,
            freight_status: STATUS_ESTIMATED,
            matching: 2,
            stop: 0,
            resolved_section: None,
        },
        0,
    ));
    let hf_claims = match_no_yards(
        &model,
        &single,
        &[crossing_at(
            "HF",
            meters_to_latlon(lat, lon, 0.0, 100.0),
            [12.0, 4.0, 4.0],
            None,
            1,
        )],
        111,
        *b"US",
        &squares,
        &hf,
    );
    assert_eq!(hf_claims.len(), 1);
    assert!((claim_freight(&hf_claims[0].1) - 12.0).abs() < 1e-6);
    assert!((claim_passenger(&hf_claims[0].1) - 8.0).abs() < 1e-6);

    // 7. Competing-name symmetry: (R1/Main, Main, R2/Main) produces the same partition regardless
    //    of which way sorts first.
    let run_perm = |ids: [i64; 3]| -> (f64, f64, f64) {
        let mut net = Network::default();
        let specs = [
            (ids[0], 0.0, "R1", "Main"),
            (ids[1], 5.0, "Main", ""),
            (ids[2], 10.0, "R2", "Main"),
        ];
        for (way, x, corr, corr_name) in specs {
            let n0 = net.nodes.len();
            net.nodes.push(meters_to_latlon(lat, lon, x, 0.0));
            let n1 = net.nodes.len();
            net.nodes.push(meters_to_latlon(lat, lon, x, 200.0));
            add_edge(&mut net, n0, n1, way, 0, sq, [0.0, 200.0]);
            let e = net.edges.last_mut().unwrap();
            e.corridor = corr.into();
            e.corridor_name = corr_name.into();
        }
        net.index();
        let cl = match_no_yards(
            &model,
            &net,
            &[crossing_at(
                "C1",
                meters_to_latlon(lat, lon, 0.0, 100.0),
                [10.0, 4.0, 6.0],
                Some(0.0),
                1,
            )],
            111,
            *b"US",
            &squares,
            &Claims::new(),
        );
        let get = |w: i64| {
            cl.iter()
                .find(|(_, iv)| iv.osm_id == w)
                .map_or(0.0, |(_, iv)| claim_freight(iv))
        };
        (get(ids[0]), get(ids[1]), get(ids[2]))
    };
    let expected = (10.0, 10.0, 0.0);
    for perm in [
        [1, 2, 3],
        [2, 1, 3],
        [3, 1, 2],
        [3, 2, 1],
        [1, 3, 2],
        [2, 3, 1],
    ] {
        assert_eq!(run_perm(perm), expected, "perm={perm:?}");
    }
}

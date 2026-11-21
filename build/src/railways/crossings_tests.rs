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
        &[],
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

    // 2. Higher-rank freight (source 2079, rank 6) is never overwritten by FRA (rank 4); explicit
    //    FRA passenger is kept only within remaining measured period capacity (never exceeding the
    //    crossing's measured totals when authoritative freight already fills every period).
    let high_frt_with = |freight: f64| {
        let mut claims = Claims::new();
        claims.entry(sq).or_default().push((
            Interval {
                missing_calendar_estimate: [0.0; 2],
                osm_id: 1,
                segment_idx: 0,
                from_m: 0.0,
                to_m: 200.0,
                country: *b"US",
                source_id: 2079,
                passenger: 0.0,
                freight,
                passenger_status: STATUS_UNKNOWN,
                freight_status: STATUS_ESTIMATED,
                matching: 2,
                stop: 0,
                resolved_section: None,
            },
            0,
        ));
        claims
    };
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
        &high_frt_with(50.0),
    );
    assert_eq!(ranked.len(), 1);
    let ranked_rt = ranked[0].1.resolved_section.unwrap();
    assert_eq!(
        ranked_rt.freight.source_id, 2079,
        "higher-rank freight is preserved"
    );
    assert!((claim_freight(&ranked[0].1) - 50.0).abs() < 1e-6);
    assert_eq!(ranked_rt.passenger.source_id, 111);
    assert!(
        claim_passenger(&ranked[0].1).abs() < 1e-6,
        "explicit FRA passenger clamps to zero when higher-rank freight fills every period"
    );
    let ranked_room = match_no_yards(
        &model,
        &net,
        &[crossing_at(
            "RANK-ROOM",
            [lat, lon],
            [10.0, 2.0, 4.0],
            Some(4.0),
            1,
        )],
        111,
        *b"US",
        &squares,
        &high_frt_with(10.0),
    );
    assert_eq!(ranked_room.len(), 1);
    assert!((claim_freight(&ranked_room[0].1) - 10.0).abs() < 1e-6);
    assert!(
        (claim_passenger(&ranked_room[0].1) - 4.0).abs() < 1e-6,
        "valid FRA passenger evidence is kept within remaining period capacity"
    );

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
        &[],
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

#[test]
fn interstation_timetable_gap_recovery_and_higher_rank_freight_period_subtraction() {
    let model = test_model();
    let (lat, lon) = (40.90, -74.70);
    let sq = [149, 192];
    let squares = HashSet::from([sq]);

    // 1. Morristown-like switch-free interstation gap:
    //    Main run [0, 6500 m]: west [0, 1500] has 13.3, [1500, 1540] has 26.6 stop-overlap spike,
    //    [1540, 5440] is a 3.9 km GTFS gap across 1-track [1540, 4900] + 2-track [4900, 5440]
    //    (parallel passing track [4900, 5440] 5 m north), east [5440, 6500] has 17.5.
    let mut net = Network::default();
    let xs = [0.0, 1540.0, 4900.0, 5440.0, 6500.0];
    let main_nodes: Vec<usize> = xs
        .iter()
        .map(|&x| {
            let id = net.nodes.len();
            net.nodes.push(meters_to_latlon(lat, lon, x, 0.0));
            id
        })
        .collect();
    for seg in 0..4 {
        add_edge(
            &mut net,
            main_nodes[seg],
            main_nodes[seg + 1],
            100,
            seg as i16,
            sq,
            [xs[seg], xs[seg + 1]],
        );
    }
    let p0 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 4900.0, 5.0));
    let p1 = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 5440.0, 5.0));
    add_edge(&mut net, p0, p1, 200, 0, sq, [0.0, 540.0]);
    net.index();

    let mut tt_claims = Claims::new();
    tt_claims
        .entry(sq)
        .or_default()
        .push((timetable_iv(100, 0, [0.0, 1500.0], 2079, 13.3), 0));
    tt_claims
        .entry(sq)
        .or_default()
        .push((timetable_iv(100, 0, [1500.0, 1540.0], 2079, 26.6), 0));
    tt_claims
        .entry(sq)
        .or_default()
        .push((timetable_iv(100, 3, [5440.0, 6500.0], 2079, 17.5), 0));

    let crossings = [
        crossing_at(
            "MT1",
            meters_to_latlon(lat, lon, 3000.0, 0.0),
            [1.0, 0.4, 0.6],
            Some(0.0),
            1,
        ),
        crossing_at(
            "MT2",
            meters_to_latlon(lat, lon, 5200.0, 0.0),
            [1.0, 0.4, 0.6],
            Some(0.0),
            2,
        ),
    ];
    let failed_nj = [FailedServiceLeg::from_shape(
        2079,
        *b"US",
        13.3,
        STATUS_ESTIMATED,
        [0.0; 2],
        vec![
            meters_to_latlon(lat, lon, 1500.0, 1.0),
            meters_to_latlon(lat, lon, 5480.0, 1.0),
        ],
    )];
    let claims = match_crossings(
        &model,
        &net,
        &crossings,
        111,
        *b"US",
        &squares,
        &tt_claims,
        &failed_nj,
        &HashSet::new(),
    );
    let one_trk = claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 100 && iv.segment_idx == 1)
        .expect("1-track gap piece")
        .1
        .clone();
    let rs1 = one_trk.resolved_section.unwrap();
    assert_eq!(rs1.passenger.source_id, 2079);
    assert!(
        (claim_passenger(&one_trk) - 13.3).abs() < 1e-6,
        "1-track gap must recover 13.3 (not 26.6 spike), got {}",
        claim_passenger(&one_trk)
    );
    assert!(claim_freight(&one_trk).abs() < 1e-6);

    let two_trk_a = claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 100 && iv.segment_idx == 2)
        .expect("2-track main gap piece")
        .1
        .clone();
    let two_trk_b = claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 200 && iv.segment_idx == 0)
        .expect("2-track parallel gap piece")
        .1
        .clone();
    assert_eq!(
        two_trk_a.resolved_section.unwrap().passenger.source_id,
        2079
    );
    assert!((claim_passenger(&two_trk_a) - 6.65).abs() < 1e-6);
    assert!((claim_passenger(&two_trk_b) - 6.65).abs() < 1e-6);
    assert!(claim_freight(&two_trk_a).abs() < 1e-6);

    // 2. One-sided terminal stub (Fox Lake / Hackettstown): must NOT bridge beyond terminus.
    let mut term = Network::default();
    let t0 = term.nodes.len();
    term.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    let t1 = term.nodes.len();
    term.nodes.push(meters_to_latlon(lat, lon, 1000.0, 0.0));
    let t2 = term.nodes.len();
    term.nodes.push(meters_to_latlon(lat, lon, 1200.0, 0.0));
    add_edge(&mut term, t0, t1, 10, 0, sq, [0.0, 1000.0]);
    add_edge(&mut term, t1, t2, 10, 1, sq, [1000.0, 1200.0]);
    term.index();
    let mut term_tt = Claims::new();
    term_tt
        .entry(sq)
        .or_default()
        .push((timetable_iv(10, 0, [0.0, 1000.0], 2079, 10.0), 0));
    let term_failed = [FailedServiceLeg::from_shape(
        2079,
        *b"US",
        10.0,
        STATUS_ESTIMATED,
        [0.0; 2],
        vec![
            meters_to_latlon(lat, lon, 900.0, 0.0),
            meters_to_latlon(lat, lon, 1200.0, 0.0),
        ],
    )];
    let term_claims = match_crossings(
        &model,
        &term,
        &[crossing_at(
            "TERM",
            meters_to_latlon(lat, lon, 1100.0, 0.0),
            [2.0, 1.0, 1.0],
            Some(0.0),
            1,
        )],
        111,
        *b"US",
        &squares,
        &term_tt,
        &term_failed,
        &HashSet::new(),
    );
    let stub = term_claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 10 && iv.segment_idx == 1)
        .unwrap()
        .1
        .clone();
    assert!(
        claim_passenger(&stub).abs() < 1e-6,
        "terminus stub must stay 0 pax"
    );
    assert!((claim_freight(&stub) - 4.0).abs() < 1e-6);

    // 3. Sub-5 m sliver bleed (0.1 m on one side) and >20 km gap must NOT bridge.
    let mut bleed = Network::default();
    let bxs = [0.0, 500.0, 2500.0, 2500.1];
    let bnodes: Vec<usize> = bxs
        .iter()
        .map(|&x| {
            let id = bleed.nodes.len();
            bleed.nodes.push(meters_to_latlon(lat, lon, x, 0.0));
            id
        })
        .collect();
    for seg in 0..3 {
        add_edge(
            &mut bleed,
            bnodes[seg],
            bnodes[seg + 1],
            20,
            seg as i16,
            sq,
            [bxs[seg], bxs[seg + 1]],
        );
    }
    bleed.index();
    let mut bleed_tt = Claims::new();
    bleed_tt
        .entry(sq)
        .or_default()
        .push((timetable_iv(20, 0, [0.0, 500.0], 2079, 30.0), 0));
    bleed_tt
        .entry(sq)
        .or_default()
        .push((timetable_iv(20, 2, [2500.0, 2500.1], 2079, 30.0), 0));
    let bleed_failed = [FailedServiceLeg::from_shape(
        2079,
        *b"US",
        30.0,
        STATUS_ESTIMATED,
        [0.0; 2],
        vec![
            meters_to_latlon(lat, lon, 500.0, 0.0),
            meters_to_latlon(lat, lon, 2500.1, 0.0),
        ],
    )];
    let bleed_claims = match_crossings(
        &model,
        &bleed,
        &[crossing_at(
            "BLD",
            meters_to_latlon(lat, lon, 1500.0, 0.0),
            [6.0, 3.0, 5.0],
            Some(0.0),
            1,
        )],
        111,
        *b"US",
        &squares,
        &bleed_tt,
        &bleed_failed,
        &HashSet::new(),
    );
    let mid = bleed_claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 20 && iv.segment_idx == 1)
        .unwrap()
        .1
        .clone();
    assert!(
        claim_passenger(&mid).abs() < 1e-6,
        "0.1 m sliver must not bridge"
    );
    assert!((claim_freight(&mid) - 14.0).abs() < 1e-6);

    let mut long_gap = Network::default();
    let lxs = [0.0, 500.0, 25_500.0, 26_000.0];
    let lnodes: Vec<usize> = lxs
        .iter()
        .map(|&x| {
            let id = long_gap.nodes.len();
            long_gap.nodes.push(meters_to_latlon(lat, lon, x, 0.0));
            id
        })
        .collect();
    for seg in 0..3 {
        add_edge(
            &mut long_gap,
            lnodes[seg],
            lnodes[seg + 1],
            21,
            seg as i16,
            sq,
            [lxs[seg], lxs[seg + 1]],
        );
    }
    long_gap.index();
    let mut long_tt = Claims::new();
    long_tt
        .entry(sq)
        .or_default()
        .push((timetable_iv(21, 0, [0.0, 500.0], 2079, 20.0), 0));
    long_tt
        .entry(sq)
        .or_default()
        .push((timetable_iv(21, 2, [25_500.0, 26_000.0], 2079, 20.0), 0));
    let long_failed = [FailedServiceLeg::from_shape(
        2079,
        *b"US",
        20.0,
        STATUS_ESTIMATED,
        [0.0; 2],
        vec![
            meters_to_latlon(lat, lon, 500.0, 0.0),
            meters_to_latlon(lat, lon, 25_500.0, 0.0),
        ],
    )];
    let long_claims = match_crossings(
        &model,
        &long_gap,
        &[crossing_at(
            "LNG",
            meters_to_latlon(lat, lon, 12_000.0, 0.0),
            [5.0, 2.0, 3.0],
            Some(0.0),
            1,
        )],
        111,
        *b"US",
        &squares,
        &long_tt,
        &long_failed,
        &HashSet::new(),
    );
    let long_mid = long_claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 21 && iv.segment_idx == 1)
        .unwrap()
        .1
        .clone();
    assert!(
        claim_passenger(&long_mid).abs() < 1e-6,
        ">20 km gap must not bridge"
    );
    assert!((claim_freight(&long_mid) - 10.0).abs() < 1e-6);

    // 4. Freight-only (traffic_mode = 2) edge inside a bracketed gap stays 0 passenger.
    let mut frt_only = Network::default();
    let fxs = [0.0, 200.0, 600.0, 800.0];
    let fnodes: Vec<usize> = fxs
        .iter()
        .map(|&x| {
            let id = frt_only.nodes.len();
            frt_only.nodes.push(meters_to_latlon(lat, lon, x, 0.0));
            id
        })
        .collect();
    for seg in 0..3 {
        add_edge(
            &mut frt_only,
            fnodes[seg],
            fnodes[seg + 1],
            30,
            seg as i16,
            sq,
            [fxs[seg], fxs[seg + 1]],
        );
    }
    frt_only.edges[1].traffic_mode = 2;
    frt_only.index();
    let mut fo_tt = Claims::new();
    fo_tt
        .entry(sq)
        .or_default()
        .push((timetable_iv(30, 0, [0.0, 200.0], 2079, 10.0), 0));
    fo_tt
        .entry(sq)
        .or_default()
        .push((timetable_iv(30, 2, [600.0, 800.0], 2079, 10.0), 0));
    let fo_failed = [FailedServiceLeg::from_shape(
        2079,
        *b"US",
        10.0,
        STATUS_ESTIMATED,
        [0.0; 2],
        vec![
            meters_to_latlon(lat, lon, 200.0, 0.0),
            meters_to_latlon(lat, lon, 600.0, 0.0),
        ],
    )];
    let fo_claims = match_crossings(
        &model,
        &frt_only,
        &[crossing_at(
            "FO",
            meters_to_latlon(lat, lon, 400.0, 0.0),
            [4.0, 2.0, 4.0],
            None,
            1,
        )],
        111,
        *b"US",
        &squares,
        &fo_tt,
        &fo_failed,
        &HashSet::new(),
    );
    let fo_mid = fo_claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 30 && iv.segment_idx == 1)
        .unwrap()
        .1
        .clone();
    assert!(claim_passenger(&fo_mid).abs() < 1e-6);
    assert!((claim_freight(&fo_mid) - 10.0).abs() < 1e-6);

    // 5. Higher-ranked freight subtracts per measured period (keeping 0 in zero-measured evening).
    let mut single = Network::default();
    let s0 = single.nodes.len();
    single.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    let s1 = single.nodes.len();
    single.nodes.push(meters_to_latlon(lat, lon, 200.0, 0.0));
    add_edge(&mut single, s0, s1, 40, 0, sq, [0.0, 200.0]);
    single.index();
    let mut hf = Claims::new();
    hf.entry(sq).or_default().push((
        Interval {
            missing_calendar_estimate: [0.0; 2],
            osm_id: 40,
            segment_idx: 0,
            from_m: 0.0,
            to_m: 200.0,
            country: *b"US",
            source_id: 2079,
            passenger: 0.0,
            freight: 10.0,
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
            "HF_PER",
            meters_to_latlon(lat, lon, 100.0, 0.0),
            [10.0, 0.0, 10.0],
            None,
            1,
        )],
        111,
        *b"US",
        &squares,
        &hf,
    );
    let pax_p = hf_claims[0].1.resolved_section.unwrap().passenger.periods;
    // Higher-rank freight 10 splits by freight_periods [0.4, 0.2, 0.4] -> [4, 2, 4].
    // Subtracted from measured [10, 0, 10] -> raw [6, 0, 6], scaled to rem=10 -> [5, 0, 5]
    // (0 in evening, where measured total is 0, rather than default pax_shares [7, 2, 1]).
    assert!((pax_p[0] - 5.0).abs() < 1e-6, "pax_p={pax_p:?}");
    assert!(
        pax_p[1].abs() < 1e-6,
        "zero-measured evening must stay 0: {pax_p:?}"
    );
    assert!((pax_p[2] - 5.0).abs() < 1e-6, "pax_p={pax_p:?}");

    // 6. Higher-ranked freight with explicit FRA passenger allocates within remaining period
    //    capacity so pax + frt never exceeds measured period totals.
    let mut hf_exp_claims_in = Claims::new();
    hf_exp_claims_in.entry(sq).or_default().push((
        Interval {
            missing_calendar_estimate: [0.0; 2],
            osm_id: 40,
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
    // Measured totals [10, 3.333..., 6.666...] (sum 20), explicit FRA passenger 8,
    // higher-rank freight 12 -> [4.8, 2.4, 4.8] by freight_periods [0.4, 0.2, 0.4] ->
    // remaining capacity [5.2, 0.933..., 1.866...] (sum 8.0); pax must fit inside remaining
    // capacity in every period without exceeding measured totals.
    let meas_tot = [10.0, 10.0 / 3.0, 20.0 / 3.0];
    let hf_exp = match_no_yards(
        &model,
        &single,
        &[crossing_at(
            "HF_EXP",
            meters_to_latlon(lat, lon, 100.0, 0.0),
            meas_tot,
            Some(8.0),
            1,
        )],
        111,
        *b"US",
        &squares,
        &hf_exp_claims_in,
    );
    let rs_exp = hf_exp[0].1.resolved_section.unwrap();
    for p in 0..3 {
        assert!(
            rs_exp.passenger.periods[p] + rs_exp.freight.periods[p] <= meas_tot[p] + 1e-6,
            "period {p} overflow: pax={:?} frt={:?} tot={meas_tot:?}",
            rs_exp.passenger.periods,
            rs_exp.freight.periods
        );
    }
    assert!((rs_exp.passenger.periods.iter().sum::<f64>() - 8.0).abs() < 1e-6);

    // 7. Off-alignment bypass (700 m from failed-leg shape) and crossover-split track (svc=4
    //    switch at gap end) must NOT inherit timetable traffic.
    let mut bypass = Network::default();
    let b0 = bypass.nodes.len();
    bypass.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    let b1 = bypass.nodes.len();
    bypass.nodes.push(meters_to_latlon(lat, lon, 300.0, 0.0));
    let b2 = bypass.nodes.len();
    bypass.nodes.push(meters_to_latlon(lat, lon, 1300.0, 700.0));
    let b3 = bypass.nodes.len();
    bypass.nodes.push(meters_to_latlon(lat, lon, 2300.0, 0.0));
    let b4 = bypass.nodes.len();
    bypass.nodes.push(meters_to_latlon(lat, lon, 2600.0, 0.0));
    add_edge_named(&mut bypass, [b0, b1], 50, 0, sq, [0.0, 300.0], "Escondido");
    add_edge_named(
        &mut bypass,
        [b1, b2],
        51,
        0,
        sq,
        [0.0, 1220.0],
        "Bypass Freight Track",
    );
    add_edge_named(
        &mut bypass,
        [b2, b3],
        51,
        1,
        sq,
        [1220.0, 2440.0],
        "Bypass Freight Track",
    );
    add_edge_named(&mut bypass, [b3, b4], 52, 0, sq, [0.0, 300.0], "Escondido");
    bypass.index();
    let mut bp_tt = Claims::new();
    bp_tt
        .entry(sq)
        .or_default()
        .push((timetable_iv(50, 0, [0.0, 300.0], 2079, 64.0), 0));
    bp_tt
        .entry(sq)
        .or_default()
        .push((timetable_iv(52, 0, [0.0, 300.0], 2079, 64.0), 0));
    let bp_failed = [FailedServiceLeg::from_shape(
        2079,
        *b"US",
        64.0,
        STATUS_ESTIMATED,
        [0.0; 2],
        vec![
            meters_to_latlon(lat, lon, 300.0, 0.0),
            meters_to_latlon(lat, lon, 2300.0, 0.0),
        ],
    )];
    let bp_claims = match_crossings(
        &model,
        &bypass,
        &[crossing_at(
            "BP",
            meters_to_latlon(lat, lon, 1300.0, 700.0),
            [4.0, 2.0, 4.0],
            Some(0.0),
            1,
        )],
        111,
        *b"US",
        &squares,
        &bp_tt,
        &bp_failed,
        &HashSet::new(),
    );
    let bp_mid = bp_claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 51)
        .unwrap()
        .1
        .clone();
    assert_eq!(bp_mid.resolved_section.unwrap().passenger.source_id, 111);
    assert!(claim_passenger(&bp_mid).abs() < 1e-6);

    // Crossover switch (traversal_only edge) at gap edge blocks recovery even on-alignment.
    let mut xover = Network::default();
    let x0 = xover.nodes.len();
    xover.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    let x1 = xover.nodes.len();
    xover.nodes.push(meters_to_latlon(lat, lon, 300.0, 0.0));
    let x2 = xover.nodes.len();
    xover.nodes.push(meters_to_latlon(lat, lon, 1300.0, 0.0));
    let x3 = xover.nodes.len();
    xover.nodes.push(meters_to_latlon(lat, lon, 1600.0, 0.0));
    let x4 = xover.nodes.len();
    xover.nodes.push(meters_to_latlon(lat, lon, 400.0, 30.0));
    add_edge(&mut xover, x0, x1, 60, 0, sq, [0.0, 300.0]);
    add_edge(&mut xover, x1, x2, 60, 1, sq, [300.0, 1300.0]);
    add_edge(&mut xover, x2, x3, 60, 2, sq, [1300.0, 1600.0]);
    add_edge(&mut xover, x1, x4, 61, 0, sq, [0.0, 105.0]);
    xover.edges.last_mut().unwrap().traversal_only = true;
    xover.index();
    let mut xo_tt = Claims::new();
    xo_tt
        .entry(sq)
        .or_default()
        .push((timetable_iv(60, 0, [0.0, 300.0], 2079, 3.0), 0));
    xo_tt
        .entry(sq)
        .or_default()
        .push((timetable_iv(60, 2, [1300.0, 1600.0], 2079, 3.0), 0));
    let xo_failed = [FailedServiceLeg::from_shape(
        2079,
        *b"US",
        3.0,
        STATUS_ESTIMATED,
        [0.0; 2],
        vec![
            meters_to_latlon(lat, lon, 300.0, 0.0),
            meters_to_latlon(lat, lon, 1300.0, 0.0),
        ],
    )];
    let xo_claims = match_crossings(
        &model,
        &xover,
        &[crossing_at(
            "XO",
            meters_to_latlon(lat, lon, 800.0, 0.0),
            [8.0, 4.0, 4.0],
            Some(0.0),
            1,
        )],
        111,
        *b"US",
        &squares,
        &xo_tt,
        &xo_failed,
        &HashSet::new(),
    );
    let xo_mid = xo_claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 60 && iv.segment_idx == 1)
        .unwrap()
        .1
        .clone();
    assert_eq!(xo_mid.resolved_section.unwrap().passenger.source_id, 111);
    assert!(claim_passenger(&xo_mid).abs() < 1e-6);

    // 8. Multi-track: (a) directional sibling (10 direct on Track B, 10 failed on Track A) yields
    //    10 per track (no halving); (b) shared 10/day failed leg across two gapped parallel tracks
    //    yields 5 per track (no doubling).
    let mut dt = Network::default();
    let a_nodes: Vec<usize> = [0.0, 200.0, 1200.0, 1400.0]
        .iter()
        .map(|&x| {
            let id = dt.nodes.len();
            dt.nodes.push(meters_to_latlon(lat, lon, x, 0.0));
            id
        })
        .collect();
    let b_nodes: Vec<usize> = [0.0, 200.0, 1200.0, 1400.0]
        .iter()
        .map(|&x| {
            let id = dt.nodes.len();
            dt.nodes.push(meters_to_latlon(lat, lon, x, 5.0));
            id
        })
        .collect();
    for seg in 0..3 {
        let xs = [0.0, 200.0, 1200.0, 1400.0];
        add_edge(
            &mut dt,
            a_nodes[seg],
            a_nodes[seg + 1],
            70,
            seg as i16,
            sq,
            [xs[seg], xs[seg + 1]],
        );
        add_edge(
            &mut dt,
            b_nodes[seg],
            b_nodes[seg + 1],
            71,
            seg as i16,
            sq,
            [xs[seg], xs[seg + 1]],
        );
    }
    dt.index();
    let dt_failed = [FailedServiceLeg::from_shape(
        2079,
        *b"US",
        10.0,
        STATUS_ESTIMATED,
        [0.0; 2],
        vec![
            meters_to_latlon(lat, lon, 200.0, 0.0),
            meters_to_latlon(lat, lon, 1200.0, 0.0),
        ],
    )];
    // (a) Track B already holds continuous 10/day across the section -> no section-wide gap, so
    //     a coupled/duplicate failed leg over Track A does NOT stack onto Track B's 10/day total
    //     (5/track, not 10/track).
    let mut dt_dir = Claims::new();
    dt_dir
        .entry(sq)
        .or_default()
        .push((timetable_iv(70, 0, [0.0, 200.0], 2079, 10.0), 0));
    dt_dir
        .entry(sq)
        .or_default()
        .push((timetable_iv(70, 2, [1200.0, 1400.0], 2079, 10.0), 0));
    for seg in 0..3 {
        let xs = [0.0, 200.0, 1200.0, 1400.0];
        dt_dir.entry(sq).or_default().push((
            timetable_iv(71, seg as i16, [xs[seg], xs[seg + 1]], 2079, 10.0),
            0,
        ));
    }
    let dt_dir_claims = match_crossings(
        &model,
        &dt,
        &[crossing_at(
            "DT1",
            meters_to_latlon(lat, lon, 700.0, 2.5),
            [12.0, 4.0, 4.0],
            None,
            2,
        )],
        111,
        *b"US",
        &squares,
        &dt_dir,
        &dt_failed,
        &HashSet::new(),
    );
    let dt_a = dt_dir_claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 70 && iv.segment_idx == 1)
        .unwrap()
        .1
        .clone();
    let dt_b = dt_dir_claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 71 && iv.segment_idx == 1)
        .unwrap()
        .1
        .clone();
    assert!(
        (claim_passenger(&dt_a) - 5.0).abs() < 1e-6,
        "already-claimed section must not stack coupled failed legs: got {}",
        claim_passenger(&dt_a)
    );
    assert!(
        (claim_passenger(&dt_b) - 5.0).abs() < 1e-6,
        "already-claimed section must not stack coupled failed legs: got {}",
        claim_passenger(&dt_b)
    );

    // (b) Both tracks gapped under one 10/day failed leg -> 10 total = 5/track (no doubling).
    let mut dt_both = Claims::new();
    for way in [70, 71] {
        dt_both
            .entry(sq)
            .or_default()
            .push((timetable_iv(way, 0, [0.0, 200.0], 2079, 5.0), 0));
        dt_both
            .entry(sq)
            .or_default()
            .push((timetable_iv(way, 2, [1200.0, 1400.0], 2079, 5.0), 0));
    }
    let dt_both_claims = match_crossings(
        &model,
        &dt,
        &[crossing_at(
            "DT2",
            meters_to_latlon(lat, lon, 700.0, 2.5),
            [10.0, 2.0, 2.0],
            Some(0.0),
            2,
        )],
        111,
        *b"US",
        &squares,
        &dt_both,
        &dt_failed,
        &HashSet::new(),
    );
    let b_a = dt_both_claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 70 && iv.segment_idx == 1)
        .unwrap()
        .1
        .clone();
    let b_b = dt_both_claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 71 && iv.segment_idx == 1)
        .unwrap()
        .1
        .clone();
    assert!(
        (claim_passenger(&b_a) - 5.0).abs() < 1e-6,
        "got {}",
        claim_passenger(&b_a)
    );
    assert!(
        (claim_passenger(&b_b) - 5.0).abs() < 1e-6,
        "got {}",
        claim_passenger(&b_b)
    );

    // 9. Closed loop gap straddling the run's coordinate origin (s = 0) recovers cleanly.
    let mut lp = Network::default();
    let l0 = lp.nodes.len();
    lp.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    let l1 = lp.nodes.len();
    lp.nodes.push(meters_to_latlon(lat, lon, 300.0, 0.0));
    let l2 = lp.nodes.len();
    lp.nodes.push(meters_to_latlon(lat, lon, 300.0, 300.0));
    let l3 = lp.nodes.len();
    lp.nodes.push(meters_to_latlon(lat, lon, 0.0, 300.0));
    add_edge(&mut lp, l0, l1, 80, 0, sq, [0.0, 300.0]);
    add_edge(&mut lp, l1, l2, 80, 1, sq, [300.0, 600.0]);
    add_edge(&mut lp, l2, l3, 80, 2, sq, [600.0, 900.0]);
    add_edge(&mut lp, l3, l0, 80, 3, sq, [900.0, 1200.0]);
    lp.index();
    // Claim [100, 1100] leaving a 200 m gap straddling l0 ([1100, 1200] + [0, 100]).
    let mut lp_tt = Claims::new();
    lp_tt
        .entry(sq)
        .or_default()
        .push((timetable_iv(80, 0, [100.0, 300.0], 2079, 8.0), 0));
    lp_tt
        .entry(sq)
        .or_default()
        .push((timetable_iv(80, 1, [300.0, 600.0], 2079, 8.0), 0));
    lp_tt
        .entry(sq)
        .or_default()
        .push((timetable_iv(80, 2, [600.0, 900.0], 2079, 8.0), 0));
    lp_tt
        .entry(sq)
        .or_default()
        .push((timetable_iv(80, 3, [900.0, 1100.0], 2079, 8.0), 0));
    let lp_failed = [FailedServiceLeg::from_shape(
        2079,
        *b"US",
        8.0,
        STATUS_ESTIMATED,
        [0.0; 2],
        vec![
            meters_to_latlon(lat, lon, 0.0, 100.0),
            meters_to_latlon(lat, lon, 0.0, 0.0),
            meters_to_latlon(lat, lon, 100.0, 0.0),
        ],
    )];
    let lp_claims = match_crossings(
        &model,
        &lp,
        &[crossing_at(
            "LP",
            meters_to_latlon(lat, lon, 50.0, 0.0),
            [6.0, 2.0, 2.0],
            Some(0.0),
            1,
        )],
        111,
        *b"US",
        &squares,
        &lp_tt,
        &lp_failed,
        &HashSet::new(),
    );
    let lp_seam = lp_claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 80 && iv.segment_idx == 0 && iv.from_m < 50.0)
        .unwrap()
        .1
        .clone();
    assert_eq!(lp_seam.resolved_section.unwrap().passenger.source_id, 2079);
    assert!((claim_passenger(&lp_seam) - 8.0).abs() < 1e-6);

    // 10. Companion-track station bracket (exact Lake Hopatcong geometry): east track carries
    //     timetable up to the station and dead-ends 500 m north; west track continues through
    //     a 3.9 km gap to the next station where timetable resumes on the west track.
    let mut lh = Network::default();
    let wxs = [0.0, 1500.0, 4900.0, 5400.0, 6400.0];
    let wn: Vec<usize> = wxs
        .iter()
        .map(|&x| {
            let id = lh.nodes.len();
            lh.nodes.push(meters_to_latlon(lat, lon, x, 0.0));
            id
        })
        .collect();
    for seg in 0..4 {
        add_edge(
            &mut lh,
            wn[seg],
            wn[seg + 1],
            90,
            seg as i16,
            sq,
            [wxs[seg], wxs[seg + 1]],
        );
    }
    let e0 = lh.nodes.len();
    lh.nodes.push(meters_to_latlon(lat, lon, 4900.0, 5.0));
    let e1 = lh.nodes.len();
    lh.nodes.push(meters_to_latlon(lat, lon, 5400.0, 5.0));
    let e2 = lh.nodes.len();
    lh.nodes.push(meters_to_latlon(lat, lon, 6400.0, 5.0));
    add_edge(&mut lh, e0, e1, 91, 0, sq, [0.0, 500.0]);
    add_edge(&mut lh, e1, e2, 91, 1, sq, [500.0, 1500.0]);
    lh.index();
    let mut lh_tt = Claims::new();
    // Timetable claimed ONLY on west track at [0, 1500] and ONLY on companion east track at [5400, 6400].
    lh_tt
        .entry(sq)
        .or_default()
        .push((timetable_iv(90, 0, [0.0, 1500.0], 2079, 13.3), 0));
    lh_tt
        .entry(sq)
        .or_default()
        .push((timetable_iv(91, 1, [500.0, 1500.0], 2079, 17.5), 0));
    let lh_failed = [FailedServiceLeg::from_shape(
        2079,
        *b"US",
        13.3,
        STATUS_ESTIMATED,
        [0.0; 2],
        vec![
            meters_to_latlon(lat, lon, 1500.0, 1.0),
            meters_to_latlon(lat, lon, 5400.0, 1.0),
        ],
    )];
    let lh_claims = match_crossings(
        &model,
        &lh,
        &[
            crossing_at(
                "LH1",
                meters_to_latlon(lat, lon, 3000.0, 0.0),
                [1.0, 0.4, 0.6],
                Some(0.0),
                1,
            ),
            crossing_at(
                "LH2",
                meters_to_latlon(lat, lon, 5150.0, 2.5),
                [1.0, 0.4, 0.6],
                Some(0.0),
                2,
            ),
        ],
        111,
        *b"US",
        &squares,
        &lh_tt,
        &lh_failed,
        &HashSet::new(),
    );
    let lh_1trk = lh_claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 90 && iv.segment_idx == 1)
        .unwrap()
        .1
        .clone();
    let lh_2w = lh_claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 90 && iv.segment_idx == 2)
        .unwrap()
        .1
        .clone();
    let lh_2e = lh_claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 91 && iv.segment_idx == 0)
        .unwrap()
        .1
        .clone();
    assert_eq!(lh_1trk.resolved_section.unwrap().passenger.source_id, 2079);
    assert!((claim_passenger(&lh_1trk) - 13.3).abs() < 1e-6);
    assert!((claim_passenger(&lh_2w) - 6.65).abs() < 1e-6);
    assert!((claim_passenger(&lh_2e) - 6.65).abs() < 1e-6);

    // 11. Higher-rank freight equal to measured period totals leaves zero remaining capacity:
    //     explicit FRA passenger must clamp to zero rather than exceeding the measured total.
    let mut sat_net = Network::default();
    sat_net.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    sat_net.nodes.push(meters_to_latlon(lat, lon, 200.0, 0.0));
    add_edge(&mut sat_net, 0, 1, 92, 0, sq, [0.0, 200.0]);
    sat_net.index();
    let mut sat_frt = Claims::new();
    sat_frt.entry(sq).or_default().push((
        Interval {
            missing_calendar_estimate: [0.0; 2],
            osm_id: 92,
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
    let sat_claims = match_no_yards(
        &model,
        &sat_net,
        &[crossing_at(
            "SAT",
            meters_to_latlon(lat, lon, 100.0, 0.0),
            [4.8, 2.4, 4.8],
            Some(3.0),
            1,
        )],
        111,
        *b"US",
        &squares,
        &sat_frt,
    );
    let sat_rt = sat_claims[0].1.resolved_section.unwrap();
    for p in 0..3 {
        assert!((sat_rt.freight.periods[p] - [4.8, 2.4, 4.8][p]).abs() < 1e-6);
        assert!(sat_rt.passenger.periods[p].abs() < 1e-6);
    }

    // 12. Closer parallel railway ownership (Homewood / Kansas City): an adjacent freight/CN track
    //     at 0.2 m from Amtrak's failed shape owns the shape even without brackets, blocking
    //     recovery onto a bracketed commuter track 20 m away.
    let mut hw = Network::default();
    for (way, y_m, cname) in [(93_i64, 0.0, "Metra Line"), (94_i64, 20.0, "CN Sub")] {
        let base = hw.nodes.len();
        for &x in &[0.0, 100.0, 300.0, 400.0] {
            hw.nodes.push(meters_to_latlon(lat, lon, x, y_m));
        }
        for seg in 0..3 {
            let from = [0.0, 100.0, 300.0][seg];
            let to = [100.0, 300.0, 400.0][seg];
            add_edge(
                &mut hw,
                base + seg,
                base + seg + 1,
                way,
                seg as i16,
                sq,
                [from, to],
            );
            hw.edges.last_mut().unwrap().corridor = cname.to_ascii_lowercase();
        }
    }
    hw.index();
    let mut hw_tt = Claims::new();
    hw_tt
        .entry(sq)
        .or_default()
        .push((timetable_iv(93, 0, [0.0, 100.0], 2079, 50.0), 0));
    hw_tt
        .entry(sq)
        .or_default()
        .push((timetable_iv(93, 2, [300.0, 400.0], 2079, 50.0), 0));
    let hw_failed = [FailedServiceLeg::from_shape(
        2079,
        *b"US",
        6.0,
        STATUS_ESTIMATED,
        [0.0; 2],
        vec![
            meters_to_latlon(lat, lon, 0.0, 20.2),
            meters_to_latlon(lat, lon, 400.0, 20.2),
        ],
    )];
    let hw_claims = match_crossings(
        &model,
        &hw,
        &[crossing_at(
            "HW",
            meters_to_latlon(lat, lon, 200.0, 0.0),
            [30.0, 10.0, 10.0],
            Some(0.0),
            1,
        )],
        111,
        *b"US",
        &squares,
        &hw_tt,
        &hw_failed,
        &HashSet::new(),
    );
    let hw_gap = hw_claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 93 && iv.segment_idx == 1)
        .unwrap()
        .1
        .clone();
    assert_eq!(
        hw_gap.resolved_section.unwrap().passenger.source_id,
        111,
        "Amtrak shape riding CN 0.2 m away must not overwrite Metra 20 m away"
    );

    // 13. Curved 2-track station boundary (Bradley Beach / Swampscott): inner and outer tracks
    //     have different arc lengths, and a direct claim ends mid-curve on only one track;
    //     geometric bracket projection prevents any overlap doubling at the station cut.
    let mut bb = Network::default();
    let curve_pts = |radius_m: f64| -> Vec<[f64; 2]> {
        (0..=8)
            .map(|k| {
                let th = (k as f64 / 8.0) * std::f64::consts::FRAC_PI_2;
                meters_to_latlon(lat, lon, radius_m * th.sin(), 306.0 - radius_m * th.cos())
            })
            .collect()
    };
    for (way, radius) in [(95_i64, 300.0), (96_i64, 306.0)] {
        let pts = curve_pts(radius);
        let base = bb.nodes.len();
        for p in &pts {
            bb.nodes.push(*p);
        }
        let mut s = 0.0;
        for seg in 0..8 {
            let step = crate::railways::network::distance(pts[seg], pts[seg + 1]);
            add_edge(
                &mut bb,
                base + seg,
                base + seg + 1,
                way,
                seg as i16,
                sq,
                [s, s + step],
            );
            s += step;
        }
    }
    bb.index();
    let mut bb_tt = Claims::new();
    // Claim inner track segments 0..2 (west bracket) and 6..8 (east bracket); outer track unclaimed
    // on segments 0..6 so bracket projection onto the longer outer arc is exercised directly.
    for seg in [0_i16, 1, 6, 7] {
        let e = bb
            .edges
            .iter()
            .find(|e| e.way == 95 && e.segment == seg)
            .unwrap();
        bb_tt
            .entry(sq)
            .or_default()
            .push((timetable_iv(95, seg, e.extent, 2079, 30.0), 0));
    }
    let bb_failed = [FailedServiceLeg::from_shape(
        2079,
        *b"US",
        30.0,
        STATUS_ESTIMATED,
        [0.0; 2],
        curve_pts(305.0),
    )];
    let bb_claims = match_crossings(
        &model,
        &bb,
        &[crossing_at(
            "BB",
            curve_pts(303.0)[4],
            [20.0, 6.0, 4.0],
            Some(0.0),
            2,
        )],
        111,
        *b"US",
        &squares,
        &bb_tt,
        &bb_failed,
        &HashSet::new(),
    );
    let bb_max_pax = bb_claims
        .iter()
        .map(|(_, iv)| claim_passenger(iv))
        .fold(0.0_f64, f64::max);
    let bb_min_pax = bb_claims
        .iter()
        .map(|(_, iv)| claim_passenger(iv))
        .fold(f64::INFINITY, f64::min);
    assert!(
        (bb_max_pax - 15.0).abs() < 1e-6 && (bb_min_pax - 15.0).abs() < 1e-6,
        "curved 2-track recovery must stay 15.0/track everywhere without overlap doubling: min={bb_min_pax} max={bb_max_pax}"
    );

    // 14. Subset invariance with overlapping separate tracks (no cross-section chaining): four
    //     parallel named tracks overlapping by 5.9 km across 60 km with eastward-improving fit
    //     produce the exact same claims in the western requested square whether or not the distant
    //     eastern track's brackets are present.
    let mut ch = Network::default();
    let specs: [(i64, f64, f64, f64, &str); 4] = [
        (101, -500.0, 19_400.0, 20.0, "Track One"),
        (102, 13_500.0, 33_400.0, 15.0, "Track Two"),
        (103, 27_500.0, 47_400.0, 10.0, "Track Three"),
        (104, 41_500.0, 61_400.0, 5.0, "Track Four"),
    ];
    let mut ch_full_tt = Claims::new();
    let mut ch_halo_tt = Claims::new();
    for &(way, x0, x1, y_m, cname) in &specs {
        let xs = [x0 - 200.0, x0, x1, x1 + 200.0];
        let base = ch.nodes.len();
        for &x in &xs {
            ch.nodes.push(meters_to_latlon(lat, lon, x, y_m));
        }
        for seg in 0..3 {
            let from = xs[seg] - (x0 - 200.0);
            let to = xs[seg + 1] - (x0 - 200.0);
            add_edge(
                &mut ch,
                base + seg,
                base + seg + 1,
                way,
                seg as i16,
                sq,
                [from, to],
            );
            ch.edges.last_mut().unwrap().corridor = cname.to_ascii_lowercase();
        }
        let b0 = timetable_iv(way, 0, [0.0, 200.0], 2079, 10.0);
        let b2 = timetable_iv(way, 2, [x1 - x0 + 200.0, x1 - x0 + 400.0], 2079, 10.0);
        ch_full_tt.entry(sq).or_default().push((b0.clone(), 0));
        ch_full_tt.entry(sq).or_default().push((b2.clone(), 0));
        if way != 104 {
            ch_halo_tt.entry(sq).or_default().push((b0, 0));
            ch_halo_tt.entry(sq).or_default().push((b2, 0));
        }
    }
    ch.index();
    let ch_failed = [FailedServiceLeg::from_shape(
        2079,
        *b"US",
        10.0,
        STATUS_ESTIMATED,
        [0.0; 2],
        vec![
            meters_to_latlon(lat, lon, -1_000.0, 0.0),
            meters_to_latlon(lat, lon, 62_000.0, 0.0),
        ],
    )];
    let ch_crossings = [crossing_at(
        "CH1",
        meters_to_latlon(lat, lon, 5_000.0, 20.0),
        [10.0, 2.0, 4.0],
        Some(0.0),
        1,
    )];
    let run_chain = |tt: &Claims| {
        let mut got: Vec<_> = match_crossings(
            &model,
            &ch,
            &ch_crossings,
            111,
            *b"US",
            &squares,
            tt,
            &ch_failed,
            &HashSet::new(),
        )
        .into_iter()
        .filter(|(_, iv)| iv.osm_id == 101)
        .map(|(_, iv)| {
            (
                iv.segment_idx,
                iv.resolved_section.unwrap().passenger.source_id,
            )
        })
        .collect();
        got.sort();
        got
    };
    assert_eq!(
        run_chain(&ch_full_tt),
        run_chain(&ch_halo_tt),
        "dropping a distant track's brackets outside the halo must not change claims on Track One"
    );

    // 15. Partial direct claim on Track B ([200, 700]) leaves a genuine section-wide gap only on
    //     [700, 1200]: recovery fills [700, 1200] (5/track = 10 total) and does not stack onto the
    //     already-claimed [200, 700] stretch (uniform 5/track across both halves).
    let mut dt_part = Claims::new();
    dt_part
        .entry(sq)
        .or_default()
        .push((timetable_iv(70, 0, [0.0, 200.0], 2079, 10.0), 0));
    dt_part
        .entry(sq)
        .or_default()
        .push((timetable_iv(70, 2, [1200.0, 1400.0], 2079, 10.0), 0));
    dt_part
        .entry(sq)
        .or_default()
        .push((timetable_iv(71, 0, [0.0, 200.0], 2079, 10.0), 0));
    dt_part
        .entry(sq)
        .or_default()
        .push((timetable_iv(71, 1, [200.0, 700.0], 2079, 10.0), 0));
    dt_part
        .entry(sq)
        .or_default()
        .push((timetable_iv(71, 2, [1200.0, 1400.0], 2079, 10.0), 0));
    let dt_part_claims = match_crossings(
        &model,
        &dt,
        &[crossing_at(
            "DTP",
            meters_to_latlon(lat, lon, 700.0, 2.5),
            [12.0, 4.0, 4.0],
            Some(0.0),
            2,
        )],
        111,
        *b"US",
        &squares,
        &dt_part,
        &dt_failed,
        &HashSet::new(),
    );
    let part_seg1: Vec<&Interval> = dt_part_claims
        .iter()
        .filter(|(_, iv)| iv.osm_id == 70 && iv.segment_idx == 1)
        .map(|(_, iv)| iv)
        .collect();
    assert!(!part_seg1.is_empty());
    let part_len: f64 = part_seg1.iter().map(|iv| iv.to_m - iv.from_m).sum();
    assert!((part_len - 1000.0).abs() < 1e-4, "part_len={part_len}");
    for iv in &part_seg1 {
        assert!(
            (claim_passenger(iv) - 5.0).abs() < 1e-6,
            "partial direct claim + section gap must stay uniform 5/track (10 total), got {}",
            claim_passenger(iv)
        );
    }

    // 16. Higher-ranked freight matching the measured daily total [9, 3, 0] (sum 12) leaves zero
    //     daily room for explicit FRA passenger even when night freight exceeds the 0 night total.
    let hf_daily_cap = match_no_yards(
        &model,
        &single,
        &[crossing_at(
            "HF_DCAP",
            meters_to_latlon(lat, lon, 100.0, 0.0),
            [9.0, 3.0, 0.0],
            Some(3.0),
            1,
        )],
        111,
        *b"US",
        &squares,
        &hf_exp_claims_in,
    );
    let rs_dcap = hf_daily_cap[0].1.resolved_section.unwrap();
    assert!(
        rs_dcap.passenger.periods.iter().sum::<f64>().abs() < 1e-6,
        "explicit FRA pax must not exceed daily remaining capacity when freight == measured daily total: {:?}",
        rs_dcap.passenger.periods
    );

    // 17. Equidistant shape between two separately named sections credits at most one section
    //     even when the larger way ID is inserted first (run 0 = way 120, run 1 = way 110).
    let mut eq = Network::default();
    let mut eq_tt = Claims::new();
    for (way, y_m, cname) in [(120_i64, -10.0, "Alpha Line"), (110_i64, 10.0, "Beta Line")] {
        let xs = [0.0, 200.0, 600.0, 800.0];
        let base = eq.nodes.len();
        for &x in &xs {
            eq.nodes.push(meters_to_latlon(lat, lon, x, y_m));
        }
        for seg in 0..3 {
            add_edge(
                &mut eq,
                base + seg,
                base + seg + 1,
                way,
                seg as i16,
                sq,
                [xs[seg], xs[seg + 1]],
            );
            eq.edges.last_mut().unwrap().corridor = cname.to_ascii_lowercase();
        }
        eq_tt
            .entry(sq)
            .or_default()
            .push((timetable_iv(way, 0, [0.0, 200.0], 2079, 8.0), 0));
        eq_tt
            .entry(sq)
            .or_default()
            .push((timetable_iv(way, 2, [600.0, 800.0], 2079, 8.0), 0));
    }
    eq.index();
    let eq_failed = [FailedServiceLeg::from_shape(
        2079,
        *b"US",
        8.0,
        STATUS_ESTIMATED,
        [0.0; 2],
        vec![
            meters_to_latlon(lat, lon, 200.0, 0.0),
            meters_to_latlon(lat, lon, 600.0, 0.0),
        ],
    )];
    let eq_claims = match_crossings(
        &model,
        &eq,
        &[
            crossing_at(
                "EQ1",
                meters_to_latlon(lat, lon, 400.0, -10.0),
                [6.0, 2.0, 2.0],
                Some(0.0),
                1,
            ),
            crossing_at(
                "EQ2",
                meters_to_latlon(lat, lon, 400.0, 10.0),
                [6.0, 2.0, 2.0],
                Some(0.0),
                1,
            ),
        ],
        111,
        *b"US",
        &squares,
        &eq_tt,
        &eq_failed,
        &HashSet::new(),
    );
    let eq_110 = eq_claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 110 && iv.segment_idx == 1)
        .unwrap()
        .1
        .clone();
    let eq_120 = eq_claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 120 && iv.segment_idx == 1)
        .unwrap()
        .1
        .clone();
    assert!(
        (claim_passenger(&eq_110) - 8.0).abs() < 1e-6 && claim_passenger(&eq_120).abs() < 1e-6,
        "equidistant shape must credit exactly one section (smaller way id 110): 110={} 120={}",
        claim_passenger(&eq_110),
        claim_passenger(&eq_120)
    );

    // 18. Failed service terminating 20 m short of a 100 m gap must NOT recover past its endpoint.
    let mut ov = Network::default();
    let oxs = [0.0, 200.0, 300.0, 500.0];
    let onodes: Vec<usize> = oxs
        .iter()
        .map(|&x| {
            let id = ov.nodes.len();
            ov.nodes.push(meters_to_latlon(lat, lon, x, 0.0));
            id
        })
        .collect();
    for seg in 0..3 {
        add_edge(
            &mut ov,
            onodes[seg],
            onodes[seg + 1],
            130,
            seg as i16,
            sq,
            [oxs[seg], oxs[seg + 1]],
        );
    }
    ov.index();
    let mut ov_tt = Claims::new();
    ov_tt
        .entry(sq)
        .or_default()
        .push((timetable_iv(130, 0, [0.0, 200.0], 2079, 10.0), 0));
    ov_tt
        .entry(sq)
        .or_default()
        .push((timetable_iv(130, 2, [300.0, 500.0], 2079, 10.0), 0));
    let ov_failed = [FailedServiceLeg::from_shape(
        2079,
        *b"US",
        10.0,
        STATUS_ESTIMATED,
        [0.0; 2],
        vec![
            meters_to_latlon(lat, lon, 200.0, 0.0),
            meters_to_latlon(lat, lon, 280.0, 0.0),
        ],
    )];
    let ov_claims = match_crossings(
        &model,
        &ov,
        &[crossing_at(
            "OV1",
            meters_to_latlon(lat, lon, 250.0, 0.0),
            [4.0, 2.0, 4.0],
            Some(0.0),
            1,
        )],
        111,
        *b"US",
        &squares,
        &ov_tt,
        &ov_failed,
        &HashSet::new(),
    );
    let ov_mid = ov_claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 130 && iv.segment_idx == 1)
        .unwrap()
        .1
        .clone();
    assert!(
        claim_passenger(&ov_mid).abs() < 1e-6,
        "failed service ending 20 m short must not recover the 100 m gap, got {}",
        claim_passenger(&ov_mid)
    );

    // 19. Positive FRA crossing passenger counts (Sandusky partial-failed 2 vs FRA 4; Tucson
    //     coupled-failed 1.286 vs FRA 0.8) must NOT be overwritten by failed_legs recovery.
    let pos_partial = match_crossings(
        &model,
        &dt,
        &[crossing_at(
            "SANDUSKY",
            meters_to_latlon(lat, lon, 700.0, 2.5),
            [20.0, 8.0, 14.0],
            Some(4.0),
            2,
        )],
        111,
        *b"US",
        &squares,
        &dt_both,
        &[FailedServiceLeg::from_shape(
            2079,
            *b"US",
            2.0,
            STATUS_ESTIMATED,
            [0.0; 2],
            vec![
                meters_to_latlon(lat, lon, 200.0, 0.0),
                meters_to_latlon(lat, lon, 1200.0, 0.0),
            ],
        )],
        &HashSet::new(),
    );
    let sandusky_a = pos_partial
        .iter()
        .find(|(_, iv)| iv.osm_id == 70 && iv.segment_idx == 1)
        .unwrap()
        .1
        .clone();
    assert_eq!(
        sandusky_a.resolved_section.unwrap().passenger.source_id,
        111
    );
    assert!(
        (claim_passenger(&sandusky_a) - 2.0).abs() < 1e-6,
        "partial failed leg must not halve FRA 4/day (2/track), got {}",
        claim_passenger(&sandusky_a)
    );
    let pos_coupled = match_crossings(
        &model,
        &dt,
        &[crossing_at(
            "TUCSON",
            meters_to_latlon(lat, lon, 700.0, 2.5),
            [20.0, 8.0, 12.0],
            Some(0.8),
            2,
        )],
        111,
        *b"US",
        &squares,
        &dt_both,
        &[FailedServiceLeg::from_shape(
            2079,
            *b"US",
            1.2857,
            STATUS_ESTIMATED,
            [0.0; 2],
            vec![
                meters_to_latlon(lat, lon, 200.0, 0.0),
                meters_to_latlon(lat, lon, 1200.0, 0.0),
            ],
        )],
        &HashSet::new(),
    );
    let tucson_a = pos_coupled
        .iter()
        .find(|(_, iv)| iv.osm_id == 70 && iv.segment_idx == 1)
        .unwrap()
        .1
        .clone();
    assert_eq!(tucson_a.resolved_section.unwrap().passenger.source_id, 111);
    assert!(
        (claim_passenger(&tucson_a) - 0.4).abs() < 1e-6,
        "coupled failed leg must not inflate FRA 0.8/day (0.4/track), got {}",
        claim_passenger(&tucson_a)
    );

    // 20. Multi-track section with a traversal-only crossover switch between two tracks (like
    //     Providence NEC): even though the 3rd parallel track is degree-2 across the 80 m gap,
    //     the sibling tracks' crossover switch splits the section, so a partial failed-leg
    //     subtotal must NOT fill the crossover hole.
    let mut xo3 = Network::default();
    for track_idx in 0..3 {
        let y = track_idx as f64 * 4.0;
        for &x in &[0.0, 200.0, 280.0, 480.0] {
            xo3.nodes.push(meters_to_latlon(lat, lon, x, y));
        }
    }
    for track_idx in 0..3 {
        let base = track_idx * 4;
        let way = 140 + track_idx as i64;
        add_edge(&mut xo3, base, base + 1, way, 0, sq, [0.0, 200.0]);
        add_edge(&mut xo3, base + 1, base + 2, way, 1, sq, [200.0, 280.0]);
        add_edge(&mut xo3, base + 2, base + 3, way, 2, sq, [280.0, 480.0]);
    }
    // Traversal-only crossover between track 0 node 1 (x=200) and track 1 node 6 (x=280).
    let pa = xo3.nodes[1];
    let pb = xo3.nodes[6];
    xo3.edges.push(Edge {
        nodes: [1, 6],
        points: [pa, pb],
        length: 80.1,
        way: 149,
        segment: 0,
        square: sq,
        extent: [0.0, 80.1],
        rail_type: 0,
        usage: 0,
        traffic_mode: 0,
        country: *b"US",
        corridor: String::new(),
        corridor_name: String::new(),
        traversal_only: true,
    });
    xo3.index();
    let mut xo3_existing = Claims::new();
    for way in [140_i64, 141, 142] {
        xo3_existing
            .entry(sq)
            .or_default()
            .push((timetable_iv(way, 0, [0.0, 200.0], 2079, 9.616), 0));
        xo3_existing
            .entry(sq)
            .or_default()
            .push((timetable_iv(way, 2, [280.0, 480.0], 2079, 9.616), 0));
    }
    let xo3_claims = match_crossings(
        &model,
        &xo3,
        &[crossing_at(
            "NEC_XO",
            meters_to_latlon(lat, lon, 240.0, 4.0),
            [2.0, 1.0, 1.0],
            Some(0.0),
            3,
        )],
        111,
        *b"US",
        &squares,
        &xo3_existing,
        &[FailedServiceLeg::from_shape(
            2079,
            *b"US",
            15.857,
            STATUS_ESTIMATED,
            [0.0; 2],
            vec![
                meters_to_latlon(lat, lon, 0.0, 4.0),
                meters_to_latlon(lat, lon, 480.0, 4.0),
            ],
        )],
        &HashSet::new(),
    );
    let xo3_t3 = xo3_claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 142 && iv.segment_idx == 1)
        .unwrap()
        .1
        .clone();
    assert_eq!(xo3_t3.resolved_section.unwrap().passenger.source_id, 111);
    assert!(
        claim_passenger(&xo3_t3).abs() < 1e-6,
        "crossover switch on sibling tracks must block failed-leg fill on 3rd track, got {}",
        claim_passenger(&xo3_t3)
    );

    // 21. Parallel companion track separating briefly from a continuously claimed sibling track
    //     (like Marceline / Connecticut River): the companion borrows brackets only at the
    //     multi-track section ends while its sibling keeps carrying source 2079 unbroken past
    //     both boundaries, so no phantom gap may be recovered onto the companion track.
    let mut split2 = Network::default();
    // Track A (continuous straight along y=0, claimed 3.0/day for the entire 1200 m).
    for &x in &[0.0, 200.0, 400.0, 800.0, 1000.0, 1200.0] {
        split2.nodes.push(meters_to_latlon(lat, lon, x, 0.0));
    }
    for seg in 0..5 {
        let xs = [0.0, 200.0, 400.0, 800.0, 1000.0, 1200.0];
        add_edge(
            &mut split2,
            seg,
            seg + 1,
            150,
            seg as i16,
            sq,
            [xs[seg], xs[seg + 1]],
        );
    }
    // Track B (paired at y=5 m on [0, 200] and [1000, 1200], swings out to y=120 m in between).
    let b_pts = [
        (0.0, 5.0),
        (200.0, 5.0),
        (400.0, 120.0),
        (800.0, 120.0),
        (1000.0, 5.0),
        (1200.0, 5.0),
    ];
    for &(x, y) in &b_pts {
        split2.nodes.push(meters_to_latlon(lat, lon, x, y));
    }
    let mut cum = 0.0;
    for seg in 0..5 {
        let len = (b_pts[seg + 1].0 - b_pts[seg].0).hypot(b_pts[seg + 1].1 - b_pts[seg].1);
        add_edge(
            &mut split2,
            6 + seg,
            7 + seg,
            151,
            seg as i16,
            sq,
            [cum, cum + len],
        );
        cum += len;
    }
    split2.index();
    let mut split2_existing = Claims::new();
    let xs = [0.0, 200.0, 400.0, 800.0, 1000.0, 1200.0];
    for seg in 0..5 {
        split2_existing.entry(sq).or_default().push((
            timetable_iv(150, seg as i16, [xs[seg], xs[seg + 1]], 2079, 3.0),
            0,
        ));
    }
    let split2_claims = match_crossings(
        &model,
        &split2,
        &[crossing_at(
            "SPLIT_COMP",
            meters_to_latlon(lat, lon, 600.0, 120.0),
            [10.0, 5.0, 5.0],
            Some(0.0),
            1,
        )],
        111,
        *b"US",
        &squares,
        &split2_existing,
        &[FailedServiceLeg::from_shape(
            2079,
            *b"US",
            3.0,
            STATUS_ESTIMATED,
            [0.0; 2],
            b_pts
                .iter()
                .map(|&(x, y)| meters_to_latlon(lat, lon, x, y))
                .collect(),
        )],
        &HashSet::new(),
    );
    let split2_b = split2_claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 151 && iv.segment_idx == 2)
        .unwrap()
        .1
        .clone();
    assert_eq!(split2_b.resolved_section.unwrap().passenger.source_id, 111);
    assert!(
        claim_passenger(&split2_b).abs() < 1e-6,
        "companion track leaving a continuously claimed sibling must not recover a phantom gap, got {}",
        claim_passenger(&split2_b)
    );

    // 22. Same-run folded balloon loop with a traversal-only turnout halfway along the return leg:
    //     both parallel legs belong to the same run_id, so the switch scan must compare leg index
    //     (not run_id) and block recovery across the switched section.
    let mut bl = Network::default();
    let bl_pts = [
        (0.0, 0.0),
        (100.0, 0.0),
        (300.0, 0.0),
        (400.0, 0.0),
        (405.0, 0.0),
        (407.5, 2.5),
        (405.0, 5.0),
        (400.0, 5.0),
        (300.0, 5.0),
        (200.0, 5.0), // node 9: turnout halfway along return leg
        (100.0, 5.0),
        (0.0, 5.0),
    ];
    for &(x, y) in &bl_pts {
        bl.nodes.push(meters_to_latlon(lat, lon, x, y));
    }
    let bl_spur_node = bl.nodes.len();
    bl.nodes.push(meters_to_latlon(lat, lon, 200.0, 35.0));
    let mut bl_cum = 0.0;
    for seg in 0..(bl_pts.len() - 1) {
        let len = (bl_pts[seg + 1].0 - bl_pts[seg].0).hypot(bl_pts[seg + 1].1 - bl_pts[seg].1);
        add_edge(
            &mut bl,
            seg,
            seg + 1,
            160,
            seg as i16,
            sq,
            [bl_cum, bl_cum + len],
        );
        bl_cum += len;
    }
    let pa = bl.nodes[9];
    let pb = bl.nodes[bl_spur_node];
    bl.edges.push(Edge {
        nodes: [9, bl_spur_node],
        points: [pa, pb],
        length: 30.0,
        way: 161,
        segment: 0,
        square: sq,
        extent: [0.0, 30.0],
        rail_type: 0,
        usage: 0,
        traffic_mode: 0,
        country: *b"US",
        corridor: String::new(),
        corridor_name: String::new(),
        traversal_only: true,
    });
    bl.index();
    let mut bl_existing = Claims::new();
    bl_existing
        .entry(sq)
        .or_default()
        .push((timetable_iv(160, 0, [0.0, 100.0], 2079, 8.0), 0));
    bl_existing
        .entry(sq)
        .or_default()
        .push((timetable_iv(160, 2, [300.0, 400.0], 2079, 8.0), 0));
    let bl_claims = match_crossings(
        &model,
        &bl,
        &[crossing_at(
            "BL_SW",
            meters_to_latlon(lat, lon, 200.0, 2.5),
            [10.0, 4.0, 4.0],
            Some(0.0),
            2,
        )],
        111,
        *b"US",
        &squares,
        &bl_existing,
        &[FailedServiceLeg::from_shape(
            2079,
            *b"US",
            8.0,
            STATUS_ESTIMATED,
            [0.0; 2],
            vec![
                meters_to_latlon(lat, lon, 0.0, 0.0),
                meters_to_latlon(lat, lon, 400.0, 0.0),
            ],
        )],
        &HashSet::new(),
    );
    let bl_out = bl_claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 160 && iv.segment_idx == 1)
        .unwrap()
        .1
        .clone();
    assert_eq!(bl_out.resolved_section.unwrap().passenger.source_id, 111);
    assert!(
        claim_passenger(&bl_out).abs() < 1e-6,
        "same-run sibling turnout must block recovery on folded balloon loop, got {}",
        claim_passenger(&bl_out)
    );

    // 23. Diverging 5-deg sibling ending at a paired section boundary where the reference track has
    //     a switch at metre 99 (past the 97.50 m perpendicular back-projection): pinned switch-span
    //     projection reaches metre 100 on the reference track and blocks recovery.
    let mut div5 = Network::default();
    let a_xs = [0.0, 20.0, 99.0, 100.0];
    for &x in &a_xs {
        div5.nodes.push(meters_to_latlon(lat, lon, x, 0.0));
    }
    for seg in 0..3 {
        add_edge(
            &mut div5,
            seg,
            seg + 1,
            170,
            seg as i16,
            sq,
            [a_xs[seg], a_xs[seg + 1]],
        );
    }
    // Traversal-only turnout at reference track node 2 (x = 99 m).
    let div_spur = div5.nodes.len();
    div5.nodes.push(meters_to_latlon(lat, lon, 99.0, -30.0));
    let (pa, pb) = (div5.nodes[2], div5.nodes[div_spur]);
    div5.edges.push(Edge {
        nodes: [2, div_spur],
        points: [pa, pb],
        length: 30.0,
        way: 171,
        segment: 0,
        square: sq,
        extent: [0.0, 30.0],
        rail_type: 0,
        usage: 0,
        traffic_mode: 0,
        country: *b"US",
        corridor: String::new(),
        corridor_name: String::new(),
        traversal_only: true,
    });
    // Sibling track starting 20 m alongside at +5 deg, continuing past the paired section end so
    // it has brackets on [0, 20] and [100, 200] around the [20, 100] gap.
    let tan5 = 5.0_f64.to_radians().tan();
    let b_xs = [0.0, 20.0, 100.0, 200.0];
    let b_base = div5.nodes.len();
    for &x in &b_xs {
        div5.nodes
            .push(meters_to_latlon(lat, lon, x, 20.0 + x * tan5));
    }
    let mut b_cum = 0.0;
    let mut b_ext = vec![0.0];
    for seg in 0..3 {
        let len = (b_xs[seg + 1] - b_xs[seg]).hypot((b_xs[seg + 1] - b_xs[seg]) * tan5);
        add_edge(
            &mut div5,
            b_base + seg,
            b_base + seg + 1,
            172,
            seg as i16,
            sq,
            [b_cum, b_cum + len],
        );
        b_cum += len;
        b_ext.push(b_cum);
    }
    div5.index();
    let mut div5_existing = Claims::new();
    div5_existing
        .entry(sq)
        .or_default()
        .push((timetable_iv(172, 0, [b_ext[0], b_ext[1]], 2079, 10.0), 0));
    div5_existing
        .entry(sq)
        .or_default()
        .push((timetable_iv(172, 2, [b_ext[2], b_ext[3]], 2079, 10.0), 0));
    let div5_claims = match_crossings(
        &model,
        &div5,
        &[crossing_at(
            "DIV5_SW",
            meters_to_latlon(lat, lon, 60.0, 12.0),
            [12.0, 4.0, 4.0],
            Some(0.0),
            2,
        )],
        111,
        *b"US",
        &squares,
        &div5_existing,
        &[FailedServiceLeg::from_shape(
            2079,
            *b"US",
            10.0,
            STATUS_ESTIMATED,
            [0.0; 2],
            vec![
                meters_to_latlon(lat, lon, 0.0, 20.0),
                meters_to_latlon(lat, lon, 200.0, 20.0 + 200.0 * tan5),
            ],
        )],
        &HashSet::new(),
    );
    let div5_b = div5_claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 172 && iv.segment_idx == 1)
        .unwrap()
        .1
        .clone();
    assert_eq!(div5_b.resolved_section.unwrap().passenger.source_id, 111);
    assert!(
        claim_passenger(&div5_b).abs() < 1e-6,
        "reference-leg switch at metre 99 inside paired diverging section must block recovery, got {}",
        claim_passenger(&div5_b)
    );

    // 24. Discontinuous sibling claims across a multi-track section end (sibling A ends at metre
    //     100; sibling C has no claim inside the section and starts a new claim right at metre
    //     100.0) must NOT falsely reject a genuine gap on companion track B.
    let mut disc = Network::default();
    // Track A: [0, 100] at y = 0 (supplies the left bracket up to metre 100, then ends).
    disc.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    disc.nodes.push(meters_to_latlon(lat, lon, 100.0, 0.0));
    add_edge(&mut disc, 0, 1, 180, 0, sq, [0.0, 100.0]);
    // Track B: [0, 100, 300, 400] at y = 5 (genuine zero-pax gap on [100, 300], direct right
    // bracket on [300, 400]).
    let b2_base = disc.nodes.len();
    for &x in &[0.0, 100.0, 300.0, 400.0] {
        disc.nodes.push(meters_to_latlon(lat, lon, x, 5.0));
    }
    let b2_xs = [0.0, 100.0, 300.0, 400.0];
    for seg in 0..3 {
        add_edge(
            &mut disc,
            b2_base + seg,
            b2_base + seg + 1,
            181,
            seg as i16,
            sq,
            [b2_xs[seg], b2_xs[seg + 1]],
        );
    }
    // Track C: paired at y = 10 on [0, 100], then swings away to y = 120 on [100, 200] where a
    // separate claim starts right at s = 100.0 (zero coverage inside [0, 100]).
    let c_pts = [(0.0, 10.0), (100.0, 10.0), (200.0, 120.0)];
    let c_base = disc.nodes.len();
    for &(x, y) in &c_pts {
        disc.nodes.push(meters_to_latlon(lat, lon, x, y));
    }
    let c_len1 = (200.0_f64 - 100.0).hypot(120.0 - 10.0);
    add_edge(&mut disc, c_base, c_base + 1, 182, 0, sq, [0.0, 100.0]);
    add_edge(
        &mut disc,
        c_base + 1,
        c_base + 2,
        182,
        1,
        sq,
        [100.0, 100.0 + c_len1],
    );
    disc.index();
    let mut disc_existing = Claims::new();
    disc_existing
        .entry(sq)
        .or_default()
        .push((timetable_iv(180, 0, [0.0, 100.0], 2079, 10.0), 0));
    disc_existing
        .entry(sq)
        .or_default()
        .push((timetable_iv(181, 2, [300.0, 400.0], 2079, 10.0), 0));
    // Sibling C's claim starts right at the section boundary (metre 100.0) with zero coverage
    // inside [0, 100]: strict positive overlap on both sides ensures C does not falsely reject B.
    disc_existing
        .entry(sq)
        .or_default()
        .push((timetable_iv(182, 1, [100.0, 100.0 + c_len1], 2079, 10.0), 0));
    let disc_claims = match_crossings(
        &model,
        &disc,
        &[crossing_at(
            "DISC_GAP",
            meters_to_latlon(lat, lon, 200.0, 5.0),
            [12.0, 4.0, 4.0],
            Some(0.0),
            1,
        )],
        111,
        *b"US",
        &squares,
        &disc_existing,
        &[FailedServiceLeg::from_shape(
            2079,
            *b"US",
            10.0,
            STATUS_ESTIMATED,
            [0.0; 2],
            vec![
                meters_to_latlon(lat, lon, 0.0, 5.0),
                meters_to_latlon(lat, lon, 400.0, 5.0),
            ],
        )],
        &HashSet::new(),
    );
    let disc_b = disc_claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 181 && iv.segment_idx == 1)
        .unwrap()
        .1
        .clone();
    assert_eq!(disc_b.resolved_section.unwrap().passenger.source_id, 2079);
    assert!(
        (claim_passenger(&disc_b) - 10.0).abs() < 1e-6,
        "boundary-starting claim on another sibling must not reject a genuine gap, got {}",
        claim_passenger(&disc_b)
    );

    // 25. Folded same-run unnamed loop whose outward (y=0) and return (y=35) tracks occupy
    //     separate locations through the middle (35 m > TOKENLESS_LATERAL_M=15 m, < 50 m shape
    //     gate): a failed shape running on the return leg must NOT be stolen by the outward leg,
    //     both when the ends are unpaired (1 section) and when paired 5 m apart (separate middle
    //     sections).
    let mut fold = Network::default();
    let fold_pts = [
        (0.0, 0.0),
        (100.0, 0.0),
        (300.0, 0.0),
        (400.0, 0.0),
        (420.0, 5.0),
        (430.0, 17.5),
        (420.0, 30.0),
        (400.0, 35.0),
        (300.0, 35.0),
        (100.0, 35.0),
        (0.0, 35.0),
    ];
    for &(x, y) in &fold_pts {
        fold.nodes.push(meters_to_latlon(lat, lon, x, y));
    }
    let mut f_cum = 0.0;
    for seg in 0..(fold_pts.len() - 1) {
        let len =
            (fold_pts[seg + 1].0 - fold_pts[seg].0).hypot(fold_pts[seg + 1].1 - fold_pts[seg].1);
        add_edge_named(
            &mut fold,
            [seg, seg + 1],
            190,
            seg as i16,
            sq,
            [f_cum, f_cum + len],
            "",
        );
        f_cum += len;
    }
    fold.index();
    let mut fold_existing = Claims::new();
    fold_existing
        .entry(sq)
        .or_default()
        .push((timetable_iv(190, 0, [0.0, 100.0], 2079, 8.0), 0));
    fold_existing
        .entry(sq)
        .or_default()
        .push((timetable_iv(190, 2, [300.0, 400.0], 2079, 8.0), 0));
    let fold_claims = match_crossings(
        &model,
        &fold,
        &[crossing_at(
            "FOLD_OUT",
            meters_to_latlon(lat, lon, 200.0, 0.0),
            [10.0, 4.0, 4.0],
            Some(0.0),
            1,
        )],
        111,
        *b"US",
        &squares,
        &fold_existing,
        &[FailedServiceLeg::from_shape(
            2079,
            *b"US",
            8.0,
            STATUS_ESTIMATED,
            [0.0; 2],
            vec![
                meters_to_latlon(lat, lon, 400.0, 35.0),
                meters_to_latlon(lat, lon, 0.0, 35.0),
            ],
        )],
        &HashSet::new(),
    );
    let fold_out = fold_claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 190 && iv.segment_idx == 1)
        .unwrap()
        .1
        .clone();
    assert_eq!(fold_out.resolved_section.unwrap().passenger.source_id, 111);
    assert!(
        claim_passenger(&fold_out).abs() < 1e-6,
        "outward leg of unpaired folded run must not steal failed shape on return leg 35 m away, got {}",
        claim_passenger(&fold_out)
    );

    // Paired-end variant: outward (y=0) and return (y=5 at [0,100] & [300,400], swinging to y=35
    // through [140,260]) form separate middle sections on the same run_id.
    let mut fold_p = Network::default();
    let fold_p_pts = [
        (0.0, 0.0),
        (100.0, 0.0),
        (300.0, 0.0),
        (400.0, 0.0),
        (405.0, 0.0),
        (407.5, 2.5),
        (405.0, 5.0),
        (400.0, 5.0),
        (300.0, 5.0),
        (260.0, 35.0),
        (140.0, 35.0),
        (100.0, 5.0),
        (0.0, 5.0),
    ];
    for &(x, y) in &fold_p_pts {
        fold_p.nodes.push(meters_to_latlon(lat, lon, x, y));
    }
    let mut fp_cum = 0.0;
    for seg in 0..(fold_p_pts.len() - 1) {
        let len = (fold_p_pts[seg + 1].0 - fold_p_pts[seg].0)
            .hypot(fold_p_pts[seg + 1].1 - fold_p_pts[seg].1);
        add_edge_named(
            &mut fold_p,
            [seg, seg + 1],
            191,
            seg as i16,
            sq,
            [fp_cum, fp_cum + len],
            "",
        );
        fp_cum += len;
    }
    fold_p.index();
    let mut fold_p_existing = Claims::new();
    fold_p_existing
        .entry(sq)
        .or_default()
        .push((timetable_iv(191, 0, [0.0, 100.0], 2079, 8.0), 0));
    fold_p_existing
        .entry(sq)
        .or_default()
        .push((timetable_iv(191, 2, [300.0, 400.0], 2079, 8.0), 0));
    let fold_p_claims = match_crossings(
        &model,
        &fold_p,
        &[crossing_at(
            "FOLD_P_OUT",
            meters_to_latlon(lat, lon, 200.0, 0.0),
            [10.0, 4.0, 4.0],
            Some(0.0),
            1,
        )],
        111,
        *b"US",
        &squares,
        &fold_p_existing,
        &[FailedServiceLeg::from_shape(
            2079,
            *b"US",
            8.0,
            STATUS_ESTIMATED,
            [0.0; 2],
            vec![
                meters_to_latlon(lat, lon, 400.0, 5.0),
                meters_to_latlon(lat, lon, 300.0, 5.0),
                meters_to_latlon(lat, lon, 260.0, 35.0),
                meters_to_latlon(lat, lon, 140.0, 35.0),
                meters_to_latlon(lat, lon, 100.0, 5.0),
                meters_to_latlon(lat, lon, 0.0, 5.0),
            ],
        )],
        &HashSet::new(),
    );
    let fold_p_out = fold_p_claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 191 && iv.segment_idx == 1)
        .unwrap()
        .1
        .clone();
    assert_eq!(
        fold_p_out.resolved_section.unwrap().passenger.source_id,
        111
    );
    assert!(
        claim_passenger(&fold_p_out).abs() < 1e-6,
        "outward leg of paired-end folded run must not steal failed shape on return leg 35 m away, got {}",
        claim_passenger(&fold_p_out)
    );

    // 26. Diverging 5-deg sibling without any switch (fixture 23 minus turnout): writing the
    //     accepted gap onto the diverging track must cover the entire [20, 100] segment with zero
    //     2.5 m zero-passenger shortfall near the paired section endpoint.
    let mut div5_clean = Network::default();
    for &x in &a_xs {
        div5_clean.nodes.push(meters_to_latlon(lat, lon, x, 0.0));
    }
    for seg in 0..3 {
        add_edge(
            &mut div5_clean,
            seg,
            seg + 1,
            200,
            seg as i16,
            sq,
            [a_xs[seg], a_xs[seg + 1]],
        );
    }
    let b3_base = div5_clean.nodes.len();
    for &x in &b_xs {
        div5_clean
            .nodes
            .push(meters_to_latlon(lat, lon, x, 20.0 + x * tan5));
    }
    let mut b3_cum = 0.0;
    let mut b3_ext = vec![0.0];
    for seg in 0..3 {
        let len = (b_xs[seg + 1] - b_xs[seg]).hypot((b_xs[seg + 1] - b_xs[seg]) * tan5);
        add_edge(
            &mut div5_clean,
            b3_base + seg,
            b3_base + seg + 1,
            201,
            seg as i16,
            sq,
            [b3_cum, b3_cum + len],
        );
        b3_cum += len;
        b3_ext.push(b3_cum);
    }
    div5_clean.index();
    let mut div5c_existing = Claims::new();
    div5c_existing
        .entry(sq)
        .or_default()
        .push((timetable_iv(201, 0, [b3_ext[0], b3_ext[1]], 2079, 10.0), 0));
    div5c_existing
        .entry(sq)
        .or_default()
        .push((timetable_iv(201, 2, [b3_ext[2], b3_ext[3]], 2079, 10.0), 0));
    let div5c_claims = match_crossings(
        &model,
        &div5_clean,
        &[crossing_at(
            "DIV5_OK",
            meters_to_latlon(lat, lon, 60.0, 12.0),
            [12.0, 4.0, 4.0],
            Some(0.0),
            2,
        )],
        111,
        *b"US",
        &squares,
        &div5c_existing,
        &[FailedServiceLeg::from_shape(
            2079,
            *b"US",
            10.0,
            STATUS_ESTIMATED,
            [0.0; 2],
            vec![
                meters_to_latlon(lat, lon, 0.0, 20.0),
                meters_to_latlon(lat, lon, 200.0, 20.0 + 200.0 * tan5),
            ],
        )],
        &HashSet::new(),
    );
    let div5c_seg1: Vec<&Interval> = div5c_claims
        .iter()
        .filter(|(_, iv)| iv.osm_id == 201 && iv.segment_idx == 1)
        .map(|(_, iv)| iv)
        .collect();
    assert!(!div5c_seg1.is_empty());
    let zero_pax_len: f64 = div5c_seg1
        .iter()
        .filter(|iv| claim_passenger(iv) <= 1e-6)
        .map(|iv| iv.to_m - iv.from_m)
        .sum();
    let pax_len: f64 = div5c_seg1
        .iter()
        .filter(|iv| iv.resolved_section.unwrap().passenger.source_id == 2079)
        .map(|iv| iv.to_m - iv.from_m)
        .sum();
    assert!(
        zero_pax_len < 1e-3 && (pax_len - (b3_ext[2] - b3_ext[1])).abs() < 1e-2,
        "accepted diverging gap must cover entire segment without 2.5 m shortfall: pax_len={pax_len:.3} zero_pax_len={zero_pax_len:.3} seg_len={:.3}",
        b3_ext[2] - b3_ext[1]
    );

    // 27. Coincident-duplicate folded loop (fixture 25 unpaired loop plus a coincident second way
    //     with distinct node IDs forming one 2-leg section around the entire fold): outward metre
    //     200 and return metre ~673 share section_id but are ~473 m apart along ref_leg, so the
    //     outward leg must NOT steal the return leg's failed shape.
    let mut fold_dup = Network::default();
    for (way_idx, &way_id) in [210_i64, 211].iter().enumerate() {
        let base = fold_dup.nodes.len();
        let dy = way_idx as f64 * 0.1; // coincident duplicate (< 0.5 m) with distinct node IDs
        for &(x, y) in &fold_pts {
            fold_dup.nodes.push(meters_to_latlon(lat, lon, x, y + dy));
        }
        let mut cum = 0.0;
        for seg in 0..(fold_pts.len() - 1) {
            let len = (fold_pts[seg + 1].0 - fold_pts[seg].0)
                .hypot(fold_pts[seg + 1].1 - fold_pts[seg].1);
            add_edge_named(
                &mut fold_dup,
                [base + seg, base + seg + 1],
                way_id,
                seg as i16,
                sq,
                [cum, cum + len],
                "",
            );
            cum += len;
        }
    }
    fold_dup.index();
    let mut fold_dup_existing = Claims::new();
    fold_dup_existing
        .entry(sq)
        .or_default()
        .push((timetable_iv(210, 0, [0.0, 100.0], 2079, 8.0), 0));
    fold_dup_existing
        .entry(sq)
        .or_default()
        .push((timetable_iv(210, 2, [300.0, 400.0], 2079, 8.0), 0));
    let fold_dup_claims = match_crossings(
        &model,
        &fold_dup,
        &[crossing_at(
            "FOLD_DUP",
            meters_to_latlon(lat, lon, 200.0, 0.0),
            [10.0, 4.0, 4.0],
            Some(0.0),
            1,
        )],
        111,
        *b"US",
        &squares,
        &fold_dup_existing,
        &[FailedServiceLeg::from_shape(
            2079,
            *b"US",
            8.0,
            STATUS_ESTIMATED,
            [0.0; 2],
            vec![
                meters_to_latlon(lat, lon, 400.0, 35.0),
                meters_to_latlon(lat, lon, 0.0, 35.0),
            ],
        )],
        &HashSet::new(),
    );
    let fold_dup_out = fold_dup_claims
        .iter()
        .find(|(_, iv)| iv.osm_id == 210 && iv.segment_idx == 1)
        .unwrap()
        .1
        .clone();
    assert_eq!(
        fold_dup_out.resolved_section.unwrap().passenger.source_id,
        111
    );
    assert!(
        claim_passenger(&fold_dup_out).abs() < 1e-6,
        "coincident-duplicate folded loop must not steal return leg's shape 473 m away, got {}",
        claim_passenger(&fold_dup_out)
    );

    // 28. Interior timetable bracket on a diverging 5-deg sibling (reference track A on [0, 100],
    //     direct brackets on B at [-60, -20] and [80, 200]): projecting only OTHER legs' claims
    //     onto B avoids round-tripping B's own interior bracket through A (which moved x=80 back
    //     to 77.656 and left 2.353 m at zero passengers).
    let mut div5_int = Network::default();
    div5_int.nodes.push(meters_to_latlon(lat, lon, 0.0, 0.0));
    div5_int.nodes.push(meters_to_latlon(lat, lon, 100.0, 0.0));
    add_edge(&mut div5_int, 0, 1, 220, 0, sq, [0.0, 100.0]);
    let bi_xs = [-60.0, -20.0, 80.0, 200.0];
    let bi_base = div5_int.nodes.len();
    for &x in &bi_xs {
        div5_int
            .nodes
            .push(meters_to_latlon(lat, lon, x, 20.0 + x * tan5));
    }
    let mut bi_cum = 0.0;
    let mut bi_ext = vec![0.0];
    for seg in 0..3 {
        let dx = bi_xs[seg + 1] - bi_xs[seg];
        let len = dx.hypot(dx * tan5);
        add_edge(
            &mut div5_int,
            bi_base + seg,
            bi_base + seg + 1,
            221,
            seg as i16,
            sq,
            [bi_cum, bi_cum + len],
        );
        bi_cum += len;
        bi_ext.push(bi_cum);
    }
    div5_int.index();
    let mut div5i_existing = Claims::new();
    div5i_existing
        .entry(sq)
        .or_default()
        .push((timetable_iv(221, 0, [bi_ext[0], bi_ext[1]], 2079, 10.0), 0));
    div5i_existing
        .entry(sq)
        .or_default()
        .push((timetable_iv(221, 2, [bi_ext[2], bi_ext[3]], 2079, 10.0), 0));
    let div5i_claims = match_crossings(
        &model,
        &div5_int,
        &[crossing_at(
            "DIV5_INT",
            meters_to_latlon(lat, lon, 40.0, 10.0),
            [12.0, 4.0, 4.0],
            Some(0.0),
            2,
        )],
        111,
        *b"US",
        &squares,
        &div5i_existing,
        &[FailedServiceLeg::from_shape(
            2079,
            *b"US",
            10.0,
            STATUS_ESTIMATED,
            [0.0; 2],
            vec![
                meters_to_latlon(lat, lon, -60.0, 20.0 - 60.0 * tan5),
                meters_to_latlon(lat, lon, 200.0, 20.0 + 200.0 * tan5),
            ],
        )],
        &HashSet::new(),
    );
    let div5i_seg1: Vec<&Interval> = div5i_claims
        .iter()
        .filter(|(_, iv)| iv.osm_id == 221 && iv.segment_idx == 1)
        .map(|(_, iv)| iv)
        .collect();
    assert!(!div5i_seg1.is_empty());
    let zero_int: f64 = div5i_seg1
        .iter()
        .filter(|iv| claim_passenger(iv) <= 1e-6)
        .map(|iv| iv.to_m - iv.from_m)
        .sum();
    let pax_int: f64 = div5i_seg1
        .iter()
        .filter(|iv| iv.resolved_section.unwrap().passenger.source_id == 2079)
        .map(|iv| iv.to_m - iv.from_m)
        .sum();
    assert!(
        zero_int < 1e-3 && (pax_int - (bi_ext[2] - bi_ext[1])).abs() < 1e-2,
        "interior bracket on diverging track must not round-trip short: pax_int={pax_int:.3} zero_int={zero_int:.3} seg_len={:.3}",
        bi_ext[2] - bi_ext[1]
    );
}

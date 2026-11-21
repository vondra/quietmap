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
        "passenger_periods": [0.65, 0.25, 0.10],
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

fn add_edge(
    net: &mut Network,
    a: usize,
    b: usize,
    way: i64,
    segment: i16,
    square: [u16; 2],
    extent: [f64; 2],
) {
    let pa = net.nodes[a];
    let pb = net.nodes[b];
    net.edges.push(Edge {
        nodes: [a, b],
        points: [pa, pb],
        length: (extent[1] - extent[0]).abs(),
        way,
        segment,
        square,
        extent,
        rail_type: 0,
        traversal_only: false,
    });
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
        corridor: "Main".into(),
        rail_type: 0,
        usage: 0,
        service: 0,
        traffic_mode: 0,
        country_iso: country,
    }
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
    // Track 2 runs parallel 4.2 m east of Track 1.
    let n_t2_s = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 4.2, -50.0));
    let n_t2_n = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 4.2, 60.0));
    // Unrelated railroad runs parallel 45 m east.
    let n_far_s = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 45.0, -50.0));
    let n_far_n = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 45.0, 60.0));

    let sq = [100, 100];
    add_edge(&mut net, n_s, n_sw, 10, 0, sq, [0.0, 54.0]);
    add_edge(&mut net, n_sw, n_n, 11, 0, sq, [0.0, 56.0]);
    add_edge(&mut net, n_sw, n_turn, 12, 0, sq, [0.0, 48.0]);
    add_edge(&mut net, n_t2_s, n_t2_n, 20, 0, sq, [0.0, 110.0]);
    add_edge(&mut net, n_far_s, n_far_n, 30, 0, sq, [0.0, 110.0]);
    net.index();

    let squares = HashSet::from([sq]);
    let crossing = crossing_at("SW01", [lat, lon], [37.0, 10.0, 27.0], Some(0.0), 2);
    let claims = match_crossings(&net, &[crossing], 111, *b"US", &squares);

    // Track 1 south (way 10) and Track 2 (way 20) share 74 -> 37 each; way 11 must not steal
    // Track 2's slot, and unrelated way 30 must receive nothing.
    let by_way: HashMap<i64, &Interval> = claims.iter().map(|(_, iv)| (iv.osm_id, iv)).collect();
    assert_eq!(by_way.len(), 2);
    assert!((by_way[&10].freight - 37.0).abs() < 1e-9);
    assert!((by_way[&20].freight - 37.0).abs() < 1e-9);
    assert_eq!(by_way[&10].crossing.unwrap().matched_tracks, 2);
    assert_eq!(by_way[&20].crossing.unwrap().matched_tracks, 2);
    assert!(!by_way.contains_key(&30));

    // With Track 2 absent, a main_tracks=2 crossing must NOT bleed onto the unrelated way 30
    // 45 m away: Track 1 keeps all 74 trains (matched_tracks = 1).
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
    add_edge(&mut single_net, b0, b1, 30, 0, sq, [0.0, 110.0]);
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
    assert_eq!(single_claims[0].1.crossing.unwrap().matched_tracks, 1);
}

#[test]
fn double_track_across_square_boundary_does_not_double_and_propagates_across_cut() {
    let model = test_model();
    let (lat, lon) = (40.75, -98.84);
    let mut net = Network::default();
    // Track 1 crosses a square cut node from Square A into Square C.
    let n1_a = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, -100.0));
    let n1_cut = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 100.0));
    let n1_c = net.nodes.len();
    net.nodes.push(meters_to_latlon(lat, lon, 0.0, 300.0));
    // Track 2 lies 4.2 m east in Square B (simulating a square boundary splitting the two tracks).
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

    // Finalize each square independently as `prepare-railways` does.
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
fn period_subtraction_preserves_night_freight_and_avoids_phantom_freight() {
    // Daytime passenger on a 24-train line: night freight stays 8, never halved to 4.
    assert_eq!(
        subtract_passenger_periods([12.0, 4.0, 8.0], [12.0, 0.0, 0.0], 12.0),
        [0.0, 4.0, 8.0]
    );
    // Equal daily totals with different period shapes yield zero phantom freight.
    assert_eq!(
        subtract_passenger_periods([10.0, 0.0, 2.0], [8.0, 2.0, 2.0], 0.0),
        [0.0, 0.0, 0.0]
    );
    // Mixed surplus rescales raw positive remainders to conserve exact daily freight (4.0).
    let mixed = subtract_passenger_periods([10.0, 1.0, 5.0], [8.0, 2.0, 2.0], 4.0);
    assert!((mixed[0] - 1.6).abs() < 1e-9);
    assert_eq!(mixed[1], 0.0);
    assert!((mixed[2] - 2.4).abs() < 1e-9);
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

    // 2. Uncrossed twin after switch (k=2, 37/track) keeps 37 + 37 = 74 instead of halving to 37.
    let mut twin = vec![
        expanded_row(
            &model,
            1,
            p0,
            p1,
            [0.0, 200.0],
            &[crossing_iv(1, [17.0, 5.0, 15.0], Some(0.0), 2)],
            *b"US",
        ),
        expanded_row(&model, 2, q0, q1, [0.0, 200.0], &[], *b"US"),
    ];
    allocate_over_parallel_tracks(&model, &mut twin, sq);
    assert!((twin[0].child.traffic.freight.periods.iter().sum::<f64>() - 37.0).abs() < 1e-6);
    assert!((twin[1].child.traffic.freight.periods.iter().sum::<f64>() - 37.0).abs() < 1e-6);

    // 3. Commuter line (Metro-North Harlem: FRA total=52, pax=52; GTFS annual avg=41) -> 0 freight.
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

    // 4. Higher-rank freight (source 2079, rank 6) is never overwritten by FRA (rank 4).
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

    // 5. Unknown passenger_daily (None) without GTFS falls back to freight class prior (20.0).
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

    // 6. Canadian row ignores US FRA source 111 completely (stays on class prior).
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

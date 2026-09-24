//! Conservation over parallel tracks: the sum over a cross-section equals the line value.

use super::allocate_over_parallel_tracks;
use crate::encode::Expanded;
use crate::merge::{class_prior, CategoryFlow, RowTraffic, STATUS_ESTIMATED, STATUS_UNKNOWN};
use crate::split::{ChildGeom, ChildRow};
use noise_compute::square_country_city::{Continent, SquareCountryCity};

const CZ: SquareCountryCity = SquareCountryCity {
    continent: Continent::Europe,
    country_iso: *b"CZ",
    city_id: 0,
};
// Registered railway sources: a CZ timetable (passages) and the CZ timetable-silent residual.
const TIMETABLE: u16 = 110;
const RESIDUAL: u16 = 9863;
/// About 4.4 m of latitude: the spacing of a double track.
const TRACK_SPACING_DEG: f64 = 0.00004;

fn flow(daily: f64, source_id: u16, matching: u8) -> CategoryFlow {
    CategoryFlow {
        periods: [daily * 0.5, daily * 0.25, daily * 0.25],
        status: STATUS_ESTIMATED,
        source_id,
        matching,
    }
}

fn track(way: i64, lane: f64, from_lon: f64, to_lon: f64, traffic: RowTraffic) -> Expanded {
    let latitude = 49.915 + lane * TRACK_SPACING_DEG;
    let (start_gx, start_gy) = grid::lonlat_to_grid(from_lon, latitude);
    let (end_gx, end_gy) = grid::lonlat_to_grid(to_lon, latitude);
    Expanded {
        parent: 0,
        child: ChildRow {
            geom: ChildGeom { start_gx, start_gy, end_gx, end_gy, length_m: 100.0 },
            traffic,
        },
        prior: class_prior(0, 0, 0, CZ),
        osm_id: way,
        corridor: String::new(),
        rail_type: 0,
        usage: 0,
        service: 0,
        country_iso: CZ.country_iso,
    }
}

fn daily(category: CategoryFlow) -> f64 {
    category.periods.iter().sum()
}

fn cross_section_sum(rows: &[Expanded], category: fn(&RowTraffic) -> CategoryFlow) -> f64 {
    rows.iter().map(|row| daily(category(&row.child.traffic))).sum()
}

#[test]
fn routed_passages_on_one_track_and_a_prior_are_each_counted_once_per_line() {
    // Revnice: the timetable walk put 144 passenger trains on one track; freight is unknown.
    let walked = RowTraffic { passenger: flow(144.0, TIMETABLE, 2), freight: CategoryFlow::default() };
    let mut rows = vec![
        track(1, 0.0, 14.230, 14.232, walked),
        track(2, 1.0, 14.230, 14.232, RowTraffic::default()),
    ];
    allocate_over_parallel_tracks(&mut rows);
    for row in &rows {
        assert!((daily(row.child.traffic.passenger) - 72.0).abs() < 1e-9);
        assert_eq!(row.child.traffic.passenger.source_id, TIMETABLE);
        assert!((daily(row.child.traffic.freight) - 10.0).abs() < 1e-9);
        assert_eq!(row.child.traffic.freight.source_id, 0);
    }
    assert!((cross_section_sum(&rows, |t| t.passenger) - 144.0).abs() < 1e-9);
    assert!((cross_section_sum(&rows, |t| t.freight) - 20.0).abs() < 1e-9);
}

#[test]
fn platform_stop_counts_of_a_measured_feed_add_up_over_the_two_directions() {
    // Warsaw Centrum: each tram track takes its own platform's departures (530 and 521).
    let platform = |count| RowTraffic { passenger: flow(count, TIMETABLE, 0), freight: CategoryFlow::default() };
    let mut rows = vec![
        track(1, 0.0, 14.23, 14.232, platform(530.0)),
        track(2, 1.0, 14.23, 14.232, platform(521.0)),
    ];
    allocate_over_parallel_tracks(&mut rows);
    assert!((cross_section_sum(&rows, |t| t.passenger) - 1051.0).abs() < 1e-9);
}

#[test]
fn whole_line_stamps_are_not_multiplied_by_the_number_of_tracks() {
    let stamp = RowTraffic { passenger: flow(2.0, RESIDUAL, 0), freight: flow(1.0, RESIDUAL, 0) };
    let mut rows: Vec<_> = (0..3).map(|lane| track(lane, lane as f64, 14.23, 14.232, stamp)).collect();
    allocate_over_parallel_tracks(&mut rows);
    assert!((cross_section_sum(&rows, |t| t.passenger) - 2.0).abs() < 1e-9);
    assert!((cross_section_sum(&rows, |t| t.freight) - 1.0).abs() < 1e-9);
}

#[test]
fn evidence_on_any_track_outranks_a_residual_and_a_known_zero_stays_zero() {
    let walked = RowTraffic { passenger: flow(60.0, TIMETABLE, 1), freight: flow(0.0, TIMETABLE, 1) };
    let residual = RowTraffic { passenger: flow(2.0, RESIDUAL, 0), freight: flow(1.0, RESIDUAL, 0) };
    let mut rows = vec![
        track(1, 0.0, 14.23, 14.232, walked),
        track(2, 1.0, 14.23, 14.232, residual),
    ];
    allocate_over_parallel_tracks(&mut rows);
    for row in &rows {
        assert!((daily(row.child.traffic.passenger) - 30.0).abs() < 1e-9);
        assert_eq!(daily(row.child.traffic.freight), 0.0);
        assert_eq!(row.child.traffic.freight.source_id, TIMETABLE);
    }
}

#[test]
fn a_timetable_residual_on_the_unwalked_twin_yields_to_the_walked_line_in_both_categories() {
    // Revnice: the walk stamps passengers on one track and knows no freight; the residual (2+1)
    // on the twin says only that the walk put no train there, not that the line lacks freight.
    let walked = RowTraffic { passenger: flow(144.0, TIMETABLE, 2), freight: CategoryFlow::default() };
    let residual = RowTraffic { passenger: flow(2.0, RESIDUAL, 0), freight: flow(1.0, RESIDUAL, 0) };
    let mut rows = vec![
        track(1, 0.0, 14.23, 14.232, walked),
        track(2, 1.0, 14.23, 14.232, residual),
    ];
    allocate_over_parallel_tracks(&mut rows);
    for row in &rows {
        assert!((daily(row.child.traffic.passenger) - 72.0).abs() < 1e-9);
        assert!((daily(row.child.traffic.freight) - 10.0).abs() < 1e-9);
        assert_eq!(row.child.traffic.freight.source_id, 0);
    }
}

#[test]
fn a_track_split_into_many_rows_still_meets_one_sibling_per_way() {
    let walked = RowTraffic { passenger: flow(90.0, TIMETABLE, 2), freight: CategoryFlow::default() };
    let mut rows = vec![track(1, 0.0, 14.230, 14.232, walked)];
    for part in 0..4 {
        let from = 14.230 + 0.0005 * part as f64;
        rows.push(track(2, 1.0, from, from + 0.0005, RowTraffic::default()));
    }
    allocate_over_parallel_tracks(&mut rows);
    assert!((daily(rows[0].child.traffic.passenger) - 45.0).abs() < 1e-9);
    for row in &rows[1..] {
        assert!((daily(row.child.traffic.passenger) - 45.0).abs() < 1e-9);
    }
}

#[test]
fn distinct_lines_spurs_distant_tracks_and_service_tracks_are_not_siblings() {
    let named = |way, lane, name: &str| {
        let mut row = track(way, lane, 14.23, 14.232, RowTraffic::default());
        row.corridor = name.to_owned();
        row
    };
    let mut rows = vec![
        named(1, 0.0, "S1"),
        named(2, 1.0, "S2"),
        track(3, 5.0, 14.232, 14.234, RowTraffic::default()),
        track(4, 6.0, 14.232, 14.234, RowTraffic::default()),
        track(5, 100.0, 14.23, 14.232, RowTraffic::default()),
    ];
    // Way 4 leaves way 3's first node, like a spur diverging from the line.
    rows[3].child.geom.start_gx = rows[2].child.geom.start_gx;
    rows[3].child.geom.start_gy = rows[2].child.geom.start_gy;
    let mut yard = track(6, 101.0, 14.23, 14.232, RowTraffic::default());
    yard.service = 1;
    yard.prior = class_prior(0, 0, 1, CZ);
    rows.push(yard);
    allocate_over_parallel_tracks(&mut rows);
    for row in &rows[..5] {
        assert!((daily(row.child.traffic.passenger) - 80.0).abs() < 1e-9, "{}", row.osm_id);
    }
    assert_eq!(rows[5].child.traffic.passenger.status, STATUS_UNKNOWN);
    assert_eq!(daily(rows[5].child.traffic.freight), 0.0);
}

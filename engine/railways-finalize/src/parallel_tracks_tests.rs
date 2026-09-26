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
const PRAGUE: grid::Square = grid::Square { x: 276, y: 173 };
// Registered railway sources: a CZ timetable (passages) and the CZ timetable-silent residual.
const TIMETABLE: u16 = 110;
const RESIDUAL: u16 = 9863;
// The global timetable (passages) behind neighbour-file cross-border claims.
const FOREIGN_TIMETABLE: u16 = 100;
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
    track_foreign(way, lane, from_lon, to_lon, traffic, RowTraffic::default())
}

fn track_at(
    way: i64,
    latitude: f64,
    from_lon: f64,
    to_lon: f64,
    traffic: RowTraffic,
    foreign: RowTraffic,
) -> Expanded {
    let (start_gx, start_gy) = grid::lonlat_to_grid(from_lon, latitude);
    let (end_gx, end_gy) = grid::lonlat_to_grid(to_lon, latitude);
    Expanded {
        parent: 0,
        child: ChildRow {
            geom: ChildGeom { start_gx, start_gy, end_gx, end_gy, length_m: 100.0 },
            traffic,
            foreign,
        },
        prior: class_prior(0, 0, 0, CZ, 0),
        osm_id: way,
        corridor: String::new(),
        rail_type: 0,
        usage: 0,
        service: 0,
        traffic_mode: 0,
        country_iso: CZ.country_iso,
    }
}

fn track_foreign(
    way: i64,
    lane: f64,
    from_lon: f64,
    to_lon: f64,
    traffic: RowTraffic,
    foreign: RowTraffic,
) -> Expanded {
    track_at(way, 49.915 + lane * TRACK_SPACING_DEG, from_lon, to_lon, traffic, foreign)
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
    allocate_over_parallel_tracks(&mut rows, PRAGUE);
    for row in &rows {
        assert!((daily(row.child.traffic.passenger) - 72.0).abs() < 1e-9);
        assert_eq!(row.child.traffic.passenger.source_id, TIMETABLE);
        assert!((daily(row.child.traffic.freight) - 6.75).abs() < 1e-9);
        assert_eq!(row.child.traffic.freight.source_id, 0);
    }
    assert!((cross_section_sum(&rows, |t| t.passenger) - 144.0).abs() < 1e-9);
    assert!((cross_section_sum(&rows, |t| t.freight) - 13.5).abs() < 1e-9);
}

#[test]
fn platform_stop_counts_of_a_measured_feed_add_up_over_the_two_directions() {
    // Warsaw Centrum: each tram track takes its own platform's departures (530 and 521).
    let platform = |count| RowTraffic { passenger: flow(count, TIMETABLE, 0), freight: CategoryFlow::default() };
    let mut rows = vec![
        track(1, 0.0, 14.23, 14.232, platform(530.0)),
        track(2, 1.0, 14.23, 14.232, platform(521.0)),
    ];
    allocate_over_parallel_tracks(&mut rows, PRAGUE);
    assert!((cross_section_sum(&rows, |t| t.passenger) - 1051.0).abs() < 1e-9);
}

#[test]
fn whole_line_stamps_are_not_multiplied_by_the_number_of_tracks() {
    let stamp = RowTraffic { passenger: flow(2.0, RESIDUAL, 0), freight: flow(1.0, RESIDUAL, 0) };
    let mut rows: Vec<_> = (0..3).map(|lane| track(lane, lane as f64, 14.23, 14.232, stamp)).collect();
    allocate_over_parallel_tracks(&mut rows, PRAGUE);
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
    allocate_over_parallel_tracks(&mut rows, PRAGUE);
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
    allocate_over_parallel_tracks(&mut rows, PRAGUE);
    for row in &rows {
        assert!((daily(row.child.traffic.passenger) - 72.0).abs() < 1e-9);
        assert!((daily(row.child.traffic.freight) - 6.75).abs() < 1e-9);
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
    allocate_over_parallel_tracks(&mut rows, PRAGUE);
    assert!((daily(rows[0].child.traffic.passenger) - 45.0).abs() < 1e-9);
    for row in &rows[1..] {
        assert!((daily(row.child.traffic.passenger) - 45.0).abs() < 1e-9);
    }
}

#[test]
fn partial_timetable_on_one_twin_keeps_the_line_prior_as_a_floor() {
    // Radebeul: one direction walked 37 trains, the twin's piece stayed empty; EBA counts 72
    // on the line, so the 37 are a lower bound and the class prior stands for the line.
    let walked = RowTraffic { passenger: flow(37.0, TIMETABLE, 2), freight: CategoryFlow::default() };
    let mut rows = vec![
        track(1, 0.0, 14.23, 14.232, walked),
        track(2, 1.0, 14.23, 14.232, RowTraffic::default()),
    ];
    allocate_over_parallel_tracks(&mut rows, PRAGUE);
    for row in &rows {
        assert!((daily(row.child.traffic.passenger) - 40.0).abs() < 1e-9);
        assert_eq!(row.child.traffic.passenger.source_id, 0);
    }
    assert!((cross_section_sum(&rows, |t| t.passenger) - 80.0).abs() < 1e-9);
}

#[test]
fn cross_border_trains_alone_do_not_set_a_domestic_line() {
    // Osterhofen: the domestic timetable missed the line; the neighbour file's 3 cross-border
    // trains bound it from below while the class prior stands (EBA counts 67 + 93).
    let abroad =
        |daily| RowTraffic { passenger: flow(daily, FOREIGN_TIMETABLE, 1), freight: CategoryFlow::default() };
    let mut rows = vec![
        track_foreign(1, 0.0, 14.23, 14.232, RowTraffic::default(), abroad(2.0)),
        track_foreign(2, 1.0, 14.23, 14.232, RowTraffic::default(), abroad(1.0)),
    ];
    allocate_over_parallel_tracks(&mut rows, PRAGUE);
    for row in &rows {
        assert!((daily(row.child.traffic.passenger) - 40.0).abs() < 1e-9);
        assert_eq!(row.child.traffic.passenger.source_id, 0);
        assert!((daily(row.child.traffic.freight) - 6.75).abs() < 1e-9);
    }
    assert!((cross_section_sum(&rows, |t| t.passenger) - 80.0).abs() < 1e-9);
}

#[test]
fn fully_walked_domestic_line_is_trusted_below_the_prior_and_foreign_ignored() {
    // Emmerich: domestic directions 37 + 17 (EBA 53); the neighbour file's overlapping 50 + 42
    // cross-border claims lose to the domestic timetable.
    let home =
        |daily| RowTraffic { passenger: flow(daily, TIMETABLE, 2), freight: CategoryFlow::default() };
    let abroad =
        |daily| RowTraffic { passenger: flow(daily, FOREIGN_TIMETABLE, 2), freight: CategoryFlow::default() };
    let mut rows = vec![
        track_foreign(1, 0.0, 14.23, 14.232, home(37.0), abroad(50.0)),
        track_foreign(2, 1.0, 14.23, 14.232, home(17.0), abroad(42.0)),
    ];
    allocate_over_parallel_tracks(&mut rows, PRAGUE);
    for row in &rows {
        assert!((daily(row.child.traffic.passenger) - 27.0).abs() < 1e-9);
        assert_eq!(row.child.traffic.passenger.source_id, TIMETABLE);
    }
    assert!((cross_section_sum(&rows, |t| t.passenger) - 54.0).abs() < 1e-9);
}

#[test]
fn foreign_trains_yield_a_no_service_stamp_beside_them() {
    // Revnice segment 2: the domestic walk missed the piece (residual 2 + 1) while neighbour
    // trains run there, so the stamp is a walk gap and the prior stands for the line. A truly
    // silent line served only from abroad overcounts here; those border stubs are rare.
    let silent = RowTraffic { passenger: flow(2.0, RESIDUAL, 0), freight: flow(1.0, RESIDUAL, 0) };
    let abroad = RowTraffic { passenger: flow(8.0, FOREIGN_TIMETABLE, 2), freight: CategoryFlow::default() };
    let mut rows = vec![
        track_foreign(1, 0.0, 14.23, 14.232, silent, abroad),
        track_foreign(2, 1.0, 14.23, 14.232, silent, RowTraffic::default()),
    ];
    allocate_over_parallel_tracks(&mut rows, PRAGUE);
    for row in &rows {
        assert!((daily(row.child.traffic.passenger) - 40.0).abs() < 1e-9);
        assert_eq!(row.child.traffic.passenger.source_id, 0);
        assert!((daily(row.child.traffic.freight) - 6.75).abs() < 1e-9);
        assert_eq!(row.child.traffic.freight.source_id, 0);
    }
}

#[test]
fn projection_and_allocation_ignore_input_row_order() {
    // A first-row latitude scale flips near-gate pairs when the input is reversed (276/173:
    // 4 rows by up to 39 passenger trains/day). The north pair sits 14.9 m apart, just inside
    // the 15 m tokenless gate under the square-centre scale; a first-row scale from the
    // square's south edge would measure it over the gate.
    let walked = RowTraffic { passenger: flow(100.0, TIMETABLE, 2), freight: CategoryFlow::default() };
    let empty = RowTraffic::default();
    let fixture = || {
        vec![
            track_at(1, 49.84, 14.230, 14.232, walked, empty),
            track_at(2, 49.84 + TRACK_SPACING_DEG, 14.230, 14.232, empty, empty),
            track_at(3, 50.28, 14.240, 14.242, walked, empty),
            track_at(4, 50.28 + 0.00013400, 14.240, 14.242, empty, empty),
        ]
    };
    let projected = super::project_tracks(&fixture(), PRAGUE);
    let mut flipped = fixture();
    flipped.reverse();
    let reprojected = super::project_tracks(&flipped, PRAGUE);
    for (forward, backward) in projected.iter().zip(reprojected.iter().rev()) {
        let (a, b) = (forward.as_ref().unwrap(), backward.as_ref().unwrap());
        assert_eq!((a.start, a.end, a.direction), (b.start, b.end, b.direction));
    }
    let mut forward = fixture();
    allocate_over_parallel_tracks(&mut forward, PRAGUE);
    // Both pairs are siblings: the walked 100 exceed the prior and split evenly.
    for row in &forward {
        assert!((daily(row.child.traffic.passenger) - 50.0).abs() < 1e-9, "{}", row.osm_id);
        assert!((daily(row.child.traffic.freight) - 6.75).abs() < 1e-9, "{}", row.osm_id);
    }
    let mut backward = fixture();
    backward.reverse();
    allocate_over_parallel_tracks(&mut backward, PRAGUE);
    for row in &backward {
        let mate = forward.iter().find(|mate| mate.osm_id == row.osm_id).unwrap();
        assert_eq!(row.child.traffic.passenger.periods, mate.child.traffic.passenger.periods);
        assert_eq!(row.child.traffic.freight.periods, mate.child.traffic.freight.periods);
    }
}

#[test]
fn a_passenger_only_twin_takes_no_freight_prior_while_its_mixed_twin_keeps_the_line() {
    // RER beside a main line: the freight prior belongs to the capable track alone.
    let mut mixed = track(1, 0.0, 14.23, 14.232, RowTraffic::default());
    mixed.traffic_mode = 3;
    mixed.prior = class_prior(0, 0, 0, CZ, 3);
    let mut suburban = track(2, 1.0, 14.23, 14.232, RowTraffic::default());
    suburban.traffic_mode = 1;
    suburban.prior = class_prior(0, 0, 0, CZ, 1);
    let mut rows = vec![mixed, suburban];
    allocate_over_parallel_tracks(&mut rows, PRAGUE);
    assert!((daily(rows[0].child.traffic.freight) - 13.5).abs() < 1e-9);
    assert_eq!(daily(rows[1].child.traffic.freight), 0.0);
    assert_eq!(rows[1].child.traffic.freight.source_id, 0);
    assert!((daily(rows[0].child.traffic.passenger) - 40.0).abs() < 1e-9);
    assert!((daily(rows[1].child.traffic.passenger) - 40.0).abs() < 1e-9);
    assert!((cross_section_sum(&rows, |t| t.freight) - 13.5).abs() < 1e-9);
}

#[test]
fn measured_trains_win_over_a_stale_traffic_mode_tag() {
    // A passenger-tagged line with timetabled freight keeps its measured trains.
    let freight = RowTraffic { passenger: CategoryFlow::default(), freight: flow(60.0, TIMETABLE, 2) };
    let mut tagged = track(1, 0.0, 14.23, 14.232, freight);
    tagged.traffic_mode = 1;
    tagged.prior = class_prior(0, 0, 0, CZ, 1);
    let mut rows = vec![tagged, track(2, 1.0, 14.23, 14.232, RowTraffic::default())];
    allocate_over_parallel_tracks(&mut rows, PRAGUE);
    assert!((cross_section_sum(&rows, |t| t.freight) - 60.0).abs() < 1e-9);
    assert_eq!(rows[0].child.traffic.freight.source_id, TIMETABLE);
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
    yard.prior = class_prior(0, 0, 1, CZ, 0);
    rows.push(yard);
    allocate_over_parallel_tracks(&mut rows, PRAGUE);
    for row in &rows[..5] {
        assert!((daily(row.child.traffic.passenger) - 80.0).abs() < 1e-9, "{}", row.osm_id);
    }
    assert_eq!(rows[5].child.traffic.passenger.status, STATUS_UNKNOWN);
    assert_eq!(daily(rows[5].child.traffic.freight), 0.0);
}

#[test]
fn parallel_siblings_need_longitudinal_overlap() {
    // 250 m track, 100 passenger trains, and a 10 m parallel scrap centred on its midpoint.
    // The midpoint foot lands on the scrap (lateral ~4 m), but the overlap is 10 m, under
    // max(30 m, 30% of 250 m). The long track keeps 100; the scrap takes the class prior.
    let walked = RowTraffic { passenger: flow(100.0, TIMETABLE, 2), freight: CategoryFlow::default() };
    let base = 14.230;
    let long = 250.0 / 71_681.54751518813;
    let scrap = 10.0 / 71_681.54751518813;
    let mid = base + long / 2.0;
    let mut rows = vec![
        track(1, 0.0, base, base + long, walked),
        track(2, 1.0, mid - scrap / 2.0, mid + scrap / 2.0, RowTraffic::default()),
    ];
    allocate_over_parallel_tracks(&mut rows, PRAGUE);
    let long = daily(rows[0].child.traffic.passenger);
    let scrap = daily(rows[1].child.traffic.passenger);
    assert!((long - 100.0).abs() < 1e-6, "long {long} scrap {scrap}");
    assert!((scrap - 80.0).abs() < 1e-6, "long {long} scrap {scrap}");
    // Asymmetric bound of the same gate: A is 0–250 m stamped 100; B is 200–300 m.
    // Overlap 50 m is 20% of A (A stays 100) and 50% of B (B renders 50).
    // 100 is above the class prior, so the partial-coverage floor does not replace it.
    let m = |metres: f64| 14.230 + metres / 71_681.54751518813;
    let mut rows = vec![
        track(1, 0.0, m(0.0), m(250.0), walked),
        track(2, 1.0, m(200.0), m(300.0), RowTraffic::default()),
    ];
    allocate_over_parallel_tracks(&mut rows, PRAGUE);
    assert!((daily(rows[0].child.traffic.passenger) - 100.0).abs() < 1e-6);
    assert!((daily(rows[1].child.traffic.passenger) - 50.0).abs() < 1e-6);
}

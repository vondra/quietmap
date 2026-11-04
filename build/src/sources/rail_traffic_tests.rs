//! Regressions for per-run railway traffic and separately identified missing-calendar estimates.
use super::*;
/// Rows meeting end to end without a switch carry their run's trains, its length-weighted
/// median: a 13 m bridge between two plain pieces of 123 takes 123 (dev4 left the whole line,
/// 369, on it at Uvaly); a third row meeting at a node (a switch) ends the run; a tram on the
/// same node is another kind.
#[test]
fn a_bridge_carries_its_runs_trains() {
    let row = |kind: u8, from: (i32, i32), to: (i32, i32), length: f64, trains: f64| {
        (kind, [from, to], length, trains)
    };
    let rows = [
        row(0, (0, 0), (100, 0), 100.0, 123.0),
        row(0, (100, 0), (113, 0), 13.0, 369.0),
        row(0, (113, 0), (213, 0), 100.0, 123.0),
        row(0, (213, 0), (300, 0), 87.0, 80.0),
        row(0, (213, 0), (213, 50), 50.0, 10.0),
        row(1, (0, 0), (100, 0), 100.0, 5.0),
    ];
    let medians = run_medians(&rows);
    assert_eq!(rows[medians[1]].3, 123.0, "the bridge");
    assert_eq!(medians[0], medians[1]);
    assert_eq!((medians[3], medians[4], medians[5]), (3, 4, 5));
}

/// Guessed heavy rail counts take their country's factors; timetable rows, trams and
/// countries without a factor keep theirs.
#[test]
fn guessed_trains_follow_the_countrys_train_km() {
    let tables = SourceTables::test_fixture();
    let off = Tier::Off;
    assert_eq!(
        train_scales(tables, RailType::Rail, *b"MK", [9013, 0], off),
        [0.082, 0.048]
    );
    assert_eq!(
        train_scales(tables, RailType::Rail, *b"NO", [0, 100], off),
        [0.265, 1.0]
    );
    assert_eq!(
        train_scales(tables, RailType::Tram, *b"RO", [0, 0], off),
        [1.0, 1.0]
    );
    assert_eq!(
        train_scales(tables, RailType::Rail, *b"CH", [0, 0], off),
        [1.0, 1.0]
    );
    // The EU27 weigh their freight by tier: a German corridor carries 8 times a line off the
    // network, a timetable row keeps its passengers.
    let [corridor, _] = train_scales(tables, RailType::Rail, *b"DE", [0, 9864], Tier::Corridor);
    let [off_network, passenger] = train_scales(tables, RailType::Rail, *b"DE", [0, 9864], off);
    assert!((corridor / off_network - 8.0).abs() < 1e-9 && passenger == 1.0);
    assert!((corridor - 0.547 * 4.0).abs() < 1e-9);
}

/// The EBA split sums to one and puts 36.8 % of EU freight in the END night.
#[test]
fn eu_freight_runs_by_the_counted_split() {
    let tables = SourceTables::test_fixture();
    assert!((tables.freight_period_shares.iter().sum::<f64>() - 1.0).abs() < 1e-12);
    let (day, night) = (0.624 / 16.0, 0.376 / 8.0);
    let expected = [12.0 * day, 3.0 * day + night, 7.0 * night + day];
    for (share, expected) in tables.freight_period_shares.iter().zip(expected) {
        assert!((share - expected).abs() < 0.001, "{share} {expected}");
    }
}

#[test]
fn calendar_estimates_keep_the_same_source_scaling_as_the_observed_trains() {
    let tables = SourceTables::test_fixture();
    let trains = Trains {
        passenger: [40.0, 0.0, 0.0],
        freight: [16.0, 0.0, 0.0],
        missing_calendar_estimate: [[30.0, 0.0, 0.0], [10.0, 0.0, 0.0]],
        status: [2, 2],
        source: [100, 9863],
    };
    let (passenger, freight, shares) = trains.scaled(
        tables,
        RailType::Rail,
        *b"MK",
        Tier::Off,
        FreightRegion::Europe,
    );
    assert_eq!(passenger, [40.0, 0.0, 0.0]);
    assert_eq!(freight, [16.0, 0.0, 0.0]);
    assert_eq!(shares, [0.75, 0.625]);
}

#[test]
fn guessed_freight_preserves_legacy_scaling_before_period_redistribution() {
    let tables = SourceTables::test_fixture();
    let trains = Trains {
        passenger: [0.0; PERIODS],
        freight: [16.1, 2.3, 0.7],
        missing_calendar_estimate: [[0.0; PERIODS]; 2],
        status: [0, 0],
        source: [0, 0],
    };
    let scale = train_scales(tables, RailType::Rail, *b"DE", [0, 0], Tier::Corridor)[0];
    let legacy_total = trains
        .freight
        .map(|value| value * scale)
        .iter()
        .sum::<f64>();
    let expected = tables
        .freight_period_shares
        .map(|share| share * legacy_total);
    let redistributed_first = tables
        .freight_period_shares
        .map(|share| share * trains.freight.iter().sum::<f64>() * scale);
    assert_ne!(expected, redistributed_first);
    let (_, actual, _) = trains.scaled(
        tables,
        RailType::Rail,
        *b"DE",
        Tier::Corridor,
        FreightRegion::Europe,
    );
    assert_eq!(actual, expected);
}

//! The bus law recovered from rows that follow it exactly, places weighing alike, and each of the
//! table's other rules on hand-computed rows.
use super::*;

fn close(a: f64, b: f64, tolerance: f64) -> bool {
    (a - b).abs() <= tolerance
}

/// Rows of a place on a grid of lines and trip ends, each `copies` times `length_m` long, running
/// `factor` times the law K 30 x (bus + 2.5 trolleybus)^0.65 x (min(te, 600,000) / 100,000)^0.5.
fn place_rows(place: usize, factor: f64, copies: usize, length_m: f64) -> Vec<FitRow> {
    let law = |lines: [f64; 2], trip_ends: f64| {
        30.0 * (lines[0] + 2.5 * lines[1]).powf(0.65)
            * (f64::min(trip_ends, 600_000.0) / 100_000.0).powf(0.5)
    };
    let mut rows = Vec::new();
    for lines in [
        [1.0, 0.0],
        [2.0, 0.0],
        [5.0, 0.0],
        [12.0, 0.0],
        [1.0, 1.0],
        [2.0, 2.0],
        [0.0, 3.0],
    ] {
        for trip_ends in [
            3_000.0, 20_000.0, 150_000.0, 450_000.0, 550_000.0, 650_000.0, 900_000.0,
        ] {
            for _ in 0..copies {
                rows.push(FitRow {
                    place,
                    buses: factor * law(lines, trip_ends),
                    lines,
                    trip_ends,
                    length_m,
                });
            }
        }
    }
    rows
}

#[test]
fn the_scan_recovers_the_law_its_bound_and_trolleybus_weight_with_every_place_weighing_alike() {
    // Place 0 runs twice the law on many short rows, place 1 half of it on a few long ones: with
    // every place weighing alike the intercept is the law's; by length alone place 1's would win.
    let mut rows = place_rows(0, 2.0, 10, 1.0);
    rows.extend(place_rows(1, 0.5, 1, 100.0));
    let weights = place_weights(&rows);
    assert!(close(weights.iter().sum::<f64>(), 2.0, 1e-9));
    let (law, by_knee) = fit_law(&rows, &weights).unwrap();
    assert_eq!(law.most_trip_ends, 600_000.0);
    assert_eq!(law.trolleybus_line_weight, 2.5);
    assert!(close(law.one_line_direction, 30.0, 1e-9), "{law:?}");
    assert!(close(law.line_directions_exponent, 0.65, 1e-12));
    assert!(close(law.service_exponent, 0.5, 1e-12));
    // Only the place factors remain: ln 2 off in each place.
    assert!(close(law.squared_error, 2.0 * 2f64.ln().powi(2), 1e-9));
    // The bounds scanned run to the first multiple of 100,000 at or above the most trip ends.
    let knees: Vec<f64> = by_knee.iter().map(|law| law.most_trip_ends).collect();
    assert_eq!(
        knees,
        (1..=9).map(|k| f64::from(k) * 1e5).collect::<Vec<_>>()
    );
    assert!(
        by_knee
            .iter()
            .all(|other| other.squared_error >= law.squared_error)
    );
}

#[test]
fn the_floor_is_where_the_timetables_stop_falling_with_the_trip_ends() {
    let law = Law {
        one_line_direction: 20.0,
        line_directions_exponent: 0.5,
        service_exponent: 0.5,
        trolleybus_line_weight: 2.0,
        most_trip_ends: 600_000.0,
        squared_error: 0.0,
    };
    let row = |trip_ends: f64, ratio: f64| FitRow {
        place: 0,
        buses: ratio * law.line_part([4.0, 0.0]),
        lines: [4.0, 0.0],
        trip_ends,
        length_m: 1.0,
    };
    // Below 1,000 trip ends (share 0.1) the timetables run a flat tenth; above, the law.
    let mut rows: Vec<FitRow> = [100.0, 200.0, 400.0, 800.0]
        .into_iter()
        .map(|trip_ends| row(trip_ends, 0.1))
        .collect();
    for trip_ends in [2_000.0, 5_000.0, 20_000.0, 100_000.0, 200_000.0] {
        rows.push(row(trip_ends, law.share(trip_ends)));
    }
    let weights = vec![1.0; rows.len()];
    // The four flat rows' mean, 0.1, is above all their shares; with the fifth it is 0.107,
    // below that row's 0.141, and it never catches up again.
    let (floor, below) = least_service(&rows, &weights, &law);
    assert!(close(floor, 0.1, 1e-12), "{floor}");
    assert_eq!(below, 4);
    // Rows that follow the law all the way down need no floor below the lowest of them.
    let exact: Vec<FitRow> = [300.0, 3_000.0, 30_000.0]
        .into_iter()
        .map(|trip_ends| row(trip_ends, 0.9 * law.share(trip_ends)))
        .collect();
    assert_eq!(
        least_service(&exact, &[1.0; 3], &law),
        (law.share(300.0), 0)
    );
    // A slow row just above a flat 0.13 (shares 0.05, 0.1, 0.12, then 0.126 running 0.08): iterating
    // "the rows below the floor run the floor" cycles between 0.115 and 0.13; the set is 0.13's.
    let cycling: Vec<FitRow> = [
        (250.0, 0.13),
        (1_000.0, 0.13),
        (1_440.0, 0.13),
        (1_587.6, 0.08),
    ]
    .into_iter()
    .map(|(trip_ends, ratio)| row(trip_ends, ratio))
    .collect();
    let (floor, below) = least_service(&cycling, &[1.0; 4], &law);
    assert!(close(floor, 0.13, 1e-12) && below == 3, "{floor} {below}");
}

#[test]
fn unmapped_buildings_take_the_geometric_mean_share_of_their_flag_and_unknown_the_mean_of_both() {
    let rows = [
        (1, 0.25, 1.0),
        (1, 1.0, 1.0),
        (2, 2.0, 3.0),
        (2, 0.5, 1.0),
        // An unknown flag's rows do not enter: unknown is the mean of rural and urban.
        (0, 9.0, 5.0),
    ];
    let [unknown, rural, urban] = service_by_built_up(&rows).unwrap();
    assert!(close(rural, 0.5, 1e-12));
    assert!(close(urban, 2f64.sqrt(), 1e-12));
    assert!(close(unknown, 0.5 * (0.5 + 2f64.sqrt()), 1e-12));
    assert!(service_by_built_up(&rows[..2]).is_err());
}

#[test]
fn period_shares_are_the_places_medians_made_to_sum_to_one() {
    let calls = [
        [70.0, 20.0, 10.0],
        [60.0, 10.0, 30.0],
        [80.0, 15.0, 5.0],
        // A place without stop calls has no shares.
        [0.0, 0.0, 0.0],
    ];
    let shares = period_shares(&calls).unwrap();
    for (share, expected) in shares.iter().zip([0.70, 0.15, 0.10]) {
        assert!(close(*share, expected / 0.95, 1e-12), "{shares:?}");
    }
    assert!(period_shares(&calls[3..]).is_err());
}

#[test]
fn a_coach_line_direction_runs_the_median_of_the_timetables_coach_route_directions() {
    assert_eq!(coach_departures(&[25.0, 1.0, 5.0, 3.0]).unwrap(), 4.0);
    assert!(coach_departures(&[]).is_err());
}

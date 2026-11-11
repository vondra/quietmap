//! Synthetic counted roads whose laws are known: the fit recovers them, weighs countries alike and
//! admits the major-road law only where it predicts a held-out country's counts.

use super::*;
use crate::sources::road_model_rows::enters_fit;
use crate::sources::road_traffic::{main_prediction, major_group_prediction};

/// A deterministic spread of trip ends in [0, 1e6) (a linear congruential sequence).
fn spread(seed: &mut u64) -> f64 {
    *seed = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    (*seed >> 11) as f64 / (1u64 << 53) as f64 * 1e6
}

/// 540 tertiary rows of a country, twelve at each of 45 places whose trip ends within 5 km run
/// from 1,000 to 45,000, rural and urban, one-way or not in turn, the 1 and 2 km trip ends
/// unrelated to the traffic.
fn tertiary_rows(country: &[u8; 2], shift: f64) -> Vec<CountedRow> {
    (1..=45)
        .flat_map(|i| std::iter::repeat_n(i, 12))
        .map(|i| {
            let around = 1_000.0 * f64::from(i);
            let (built_up, oneway) = (1 + (i % 2) as u8, i % 3 == 0);
            // ln y = 1 (rural) or 2 (urban) + 0.5 ln min(trip ends, 41,000) - 0.3 one-way.
            let law = [1.0, 2.0][usize::from(built_up - 1)]
                + 0.5 * around.min(41_000.0).ln()
                + if oneway { -0.3 } else { 0.0 };
            CountedRow {
                country: *country,
                class: 4,
                built_up,
                oneway,
                lanes: 0,
                rural_network: false,
                length_m: 100.0,
                total: (law + shift).exp(),
                around: [
                    f64::from(46 - i) * 7.0,
                    (f64::from(i) * 37.0) % 500.0 + 1.0,
                    around,
                    9e5,
                ],
            }
        })
        .collect()
}

/// The 90th percentile of the rows' 5 km trip ends is 41,000 (the 41st place of 45 equally
/// weighed passes 40.5); the exact law there is recovered, and the 5 km radius predicts best.
#[test]
fn a_main_class_fit_recovers_its_law_at_the_radius_that_predicts_best() {
    let rows = tertiary_rows(b"CZ", 0.0);
    let (model, support) = main_model(4, &rows.iter().collect::<Vec<_>>()).unwrap();
    assert_eq!(
        model,
        MainModel {
            class: 4,
            radius_m: 5_000.0,
            most_around: 41_000.0,
            intercept: [1.0, 2.0],
            slope: 0.5,
            oneway: -0.3
        }
    );
    assert!(support["mean_absolute_db"]["5000"].as_f64().unwrap() < 1e-9);
    assert!(support["mean_absolute_db"]["1000"].as_f64().unwrap() > 0.1);
    // The table's reader gives each counted row its count back.
    for row in &rows {
        let predicted = main_prediction(&model, row.around[2], (row.built_up, row.oneway));
        assert!((predicted.unwrap() / row.total - 1.0).abs() < 1e-3);
    }
}

/// Ten times Czechia's rows in a country counting 1.0 more in logs weigh as much as Czechia's,
/// so the intercepts rise by half of it; a country of 499 rows, a fallback's count or a
/// continuation moves nothing.
#[test]
fn every_country_weighs_alike_in_the_fit() {
    let mut rows = tertiary_rows(b"CZ", 0.0);
    for _ in 0..10 {
        rows.extend(tertiary_rows(b"US", 1.0));
    }
    rows.extend(tertiary_rows(b"LU", 5.0).into_iter().take(499));
    let (model, _) = main_model(4, &rows.iter().collect::<Vec<_>>()).unwrap();
    assert_eq!(
        (model.intercept, model.slope, model.oneway),
        ([1.5, 2.5], 0.5, -0.3)
    );
    let continuation = crate::roads::CONTINUITY_SOURCE;
    assert!(enters_fit(4, (true, 1041, false), 2, (900.0, 50.0)));
    for (class, status, built_up) in [
        (4, (false, 1041, false), 2),
        (4, (true, continuation, false), 2),
        (4, (true, 1041, true), 2),
        (4, (true, 1041, false), 0),
        (5, (true, 1041, false), 2),
    ] {
        assert!(!enters_fit(class, status, built_up, (900.0, 50.0)));
    }
    assert!(!enters_fit(4, (true, 1041, false), 2, (0.0, 50.0)));
}

/// The 2026-form major law in one group and surroundings each.
fn major_law() -> RoadModels {
    RoadModels {
        main: Vec::new(),
        regional_main: Vec::new(),
        major: [
            ([3.0, 3.2], [0.05, 0.1, 0.3]),
            ([2.5, 2.7], [0.02, 0.2, 0.2]),
            ([2.0, 2.3], [0.1, 0.1, 0.15]),
            ([0.5, 0.6], [0.0, 0.3, 0.4]),
        ]
        .map(|(intercept, slopes)| MajorModel { intercept, slopes }),
        major_radii_m: [1_000.0, 5_000.0, 15_000.0],
        major_oneway: -0.2,
        major_ln_lanes: 0.5,
        major_lanes_unknown: 0.1,
        major_countries: Vec::new(),
    }
}

/// 600 major rows of a country through every group, surroundings, direction and lane count, at
/// spread trip ends; their traffic follows the law, or with `reflected` its mirror about each
/// group and surroundings' mean: traffic falling where the law rises.
fn major_rows(country: &[u8; 2], seed: u64, reflected: bool) -> Vec<CountedRow> {
    let law = major_law();
    let mut seed = seed;
    let rows: Vec<CountedRow> = (0..600)
        .map(|i| {
            let group = i % 4;
            let around = [spread(&mut seed), 0.0, spread(&mut seed), spread(&mut seed)];
            let (built_up, oneway, lanes) = (
                1 + ((i / 4) % 2) as u8,
                (i / 8) % 2 == 1,
                (i / 16 % 5) as u8,
            );
            let total = major_group_prediction(
                &law,
                group,
                [around[0], around[2], around[3]],
                (built_up, oneway, lanes),
            );
            CountedRow {
                country: *country,
                class: [0, 1, 2, 10][group],
                built_up,
                oneway,
                lanes,
                rural_network: false,
                length_m: 50.0 + f64::from(i as u32 % 7),
                total,
                around,
            }
        })
        .collect();
    if reflected { reflect(rows) } else { rows }
}

fn reflect(rows: Vec<CountedRow>) -> Vec<CountedRow> {
    let mut sums = [(0.0, 0.0); 8];
    for row in &rows {
        let cell = &mut sums[major_cell(row)];
        *cell = (cell.0 + row.total.ln(), cell.1 + 1.0);
    }
    rows.into_iter()
        .map(|row| {
            let (sum, count) = sums[major_cell(&row)];
            CountedRow {
                total: (2.0 * sum / count - row.total.ln()).exp(),
                ..row
            }
        })
        .collect()
}

/// Counts that follow the law in three countries give it back exactly; each is predicted
/// without error held out, so all three take it. A country whose roads carry less where the
/// others carry more is predicted better by the others' class constants and keeps its priors.
#[test]
fn the_major_law_is_recovered_and_replaces_the_priors_only_where_it_predicts_held_out_counts() {
    let mut rows = major_rows(b"CZ", 1, false);
    rows.extend(major_rows(b"US", 2, false));
    rows.extend(major_rows(b"JP", 3, false));
    let (exact, _) = major_models(&rows.iter().collect::<Vec<_>>()).unwrap();
    let law = major_law();
    assert_eq!(exact.major, law.major);
    assert_eq!(
        (
            exact.major_oneway,
            exact.major_ln_lanes,
            exact.major_lanes_unknown
        ),
        (
            law.major_oneway,
            law.major_ln_lanes,
            law.major_lanes_unknown
        )
    );
    assert_eq!(exact.major_countries, vec![*b"CZ", *b"JP", *b"US"]);
    rows.extend(major_rows(b"TH", 4, true));
    let (models, support) = major_models(&rows.iter().collect::<Vec<_>>()).unwrap();
    assert_eq!(models.major_countries, vec![*b"CZ", *b"JP", *b"US"]);
    let thailand = &support["held_out"]["TH"];
    assert!(thailand["held_out_model_db"].as_f64() > thailand["held_out_constants_db"].as_f64());
}

/// A rural road network's counted rows fit their own law and stay out of the pooled one.
#[test]
fn a_rural_road_network_fits_its_own_law_apart_from_the_pooled_one() {
    let mut rows = tertiary_rows(b"CZ", 0.0);
    rows.extend(tertiary_rows(b"TH", 2.0).into_iter().map(|row| CountedRow {
        rural_network: true,
        ..row
    }));
    rows.extend(major_rows(b"CZ", 1, false));
    rows.extend(major_rows(b"US", 2, false));
    let (models, _) = fit_models(&rows).unwrap();
    assert_eq!(models.main.len(), 1);
    assert_eq!(models.main[0].intercept, [1.0, 2.0]);
    assert_eq!(models.regional_main.len(), 1);
    assert_eq!(models.regional_main[0].country, *b"TH");
    assert_eq!(models.regional_main[0].models[0].intercept, [3.0, 4.0]);
}

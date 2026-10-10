//! The shares' tests: they add up to the loudness, split equal sources equally and give a rare
//! loud pass less than its energy.

use super::*;
use crate::distribution::Distribution;
use crate::loudness::{NO_FLIGHTS, curves, loudness};
use crate::percentiles::tests::{contributor, selection};
use crate::percentiles::{Weather, click_lines, distributions_of, flight_weather};
use physics::bands::{BANDS, energy};

/// A road's received A-weighted octave energies at 50 km/h (relative).
const ROAD_SHAPE: [f64; BANDS] = [0.002, 0.03, 0.09, 0.28, 0.42, 0.18, 0.03, 0.004];

fn road_curves() -> [Curve; PERIODS] {
    let mut road = LayerSelection::new(Layer::Road);
    road.energy = [energy(60.0); PERIODS];
    road.spectrum = [ROAD_SHAPE; PERIODS];
    curves(&[road], &NO_FLIGHTS)
}

/// A line of passes at `mean_db` with `lambda` emitters within its distance.
fn passes(weather: &Weather, mean_db: f64, lambda: f64) -> Line<'_> {
    let mean = [energy(mean_db); PERIODS];
    Line::flights(weather, (mean, mean.map(|value| value * lambda)))
}

fn weather_of(mean_db: f64) -> Weather {
    let mut weather = Weather::default();
    let mean = [energy(mean_db); PERIODS];
    weather.add(&mean, &[mean; 2]);
    weather
}

/// The loudness of `lines` over `steady` as the level distributions give it (Nden).
fn nden(lines: &[Line], steady: [f64; PERIODS], curves: &[Curve; PERIODS]) -> f64 {
    let distributions: [Distribution; PERIODS] = distributions_of(lines, steady);
    loudness(&distributions, curves).nden_sone
}

/// Steady sources and passes together: the shares add up to the loudness of the moments they
/// split (the floor added first here, last in the distributions: a bin's rounding apart).
#[test]
fn shares_add_up_to_the_loudness() {
    let curves = road_curves();
    let (near, far) = (weather_of(55.0), weather_of(50.0));
    let lines = [passes(&near, 55.0, 0.05), passes(&far, 50.0, 0.3)];
    let steady = [
        [energy(45.0); PERIODS],
        [energy(40.0); PERIODS],
        [0.0; PERIODS],
        [0.0; PERIODS],
    ];
    let shares = loudness_shares(&lines, &[2, 3], &steady, &curves);
    let summed: f64 = shares.iter().sum();
    let whole = nden(&lines, [energy(45.0) + energy(40.0); PERIODS], &curves);
    assert!((summed / whole - 1.0).abs() < 0.005, "{summed} {whole}");
    assert!(shares.iter().all(|&share| share > 0.0), "{shares:?}");
}

/// Two equal steady sources share every moment equally; a line alone makes all of it.
#[test]
fn equal_sources_share_equally_and_one_alone_takes_all() {
    let curves = road_curves();
    let steady = [[energy(50.0); PERIODS], [energy(50.0); PERIODS]];
    let shares = loudness_shares(&[], &[], &steady, &curves);
    assert!((shares[0] / shares[1] - 1.0).abs() < 1e-12, "{shares:?}");
    let weather = weather_of(55.0);
    let alone = [passes(&weather, 55.0, 0.05)];
    let shares = loudness_shares(&alone, &[0], &[[0.0; PERIODS]], &curves);
    let whole = nden(&alone, [0.0; PERIODS], &curves);
    assert!(
        (shares[0] / whole - 1.0).abs() < 1e-9,
        "{} {whole}",
        shares[0]
    );
}

/// A train an hour, loud while it passes, beside a steady road of the same energy: the train
/// makes far less of the loudness than of the energy, the road far more than half.
#[test]
fn a_rare_loud_pass_takes_less_than_its_energy() {
    let curves = road_curves();
    let weather = weather_of(55.0);
    let train = [passes(&weather, 55.0, 0.01)];
    let shares = loudness_shares(
        &train,
        &[0],
        &[[0.0; PERIODS], [energy(55.0); PERIODS]],
        &curves,
    );
    let train_part = shares[0] / (shares[0] + shares[1]);
    assert!(train_part > 0.0 && train_part < 0.3, "{train_part}");
}

/// Passes, rare and frequent, and church bells sounding 2 % of the day, with nothing steady: the
/// shares split exactly the moments the distributions build (the same lines in the same order),
/// up to the loudness of a bin merged across the weather states.
#[test]
fn shares_split_exactly_the_moments_of_the_distributions() {
    let curves = road_curves();
    let (near, far) = (weather_of(55.0), weather_of(50.0));
    let bells = contributor(7, Layer::Building, 50.0, 80.0);
    let fields = serde_json::json!({"events_per_day": [36.0, 4.0, 0.0], "duty": [0.02, 0.01, 0.0]});
    let lines = [
        passes(&near, 55.0, 0.05),
        passes(&far, 50.0, 0.3),
        Line::of(&bells, Some(&fields)).expect("an event line"),
    ];
    let shares = loudness_shares(&lines, &[0, 1, 2], &[[0.0; PERIODS]; 3], &curves);
    let summed: f64 = shares.iter().sum();
    let whole = nden(&lines, [0.0; PERIODS], &curves);
    assert!((summed / whole - 1.0).abs() < 1e-4, "{summed} {whole}");
}

/// The list's rows: a listed road with its traffic (a line), a listed steady site, a road the
/// list leaves out (the rest) and the flights (the aircraft row): each part once, adding up to 1.
#[test]
fn every_line_and_every_steady_part_has_one_row() {
    let curves = road_curves();
    let road = contributor(1, Layer::Road, 50.0, 40.0);
    let site = contributor(2, Layer::Industry, 45.0, 200.0);
    let unlisted = contributor(3, Layer::Road, 40.0, 300.0);
    let mut aircraft = LayerSelection::new(Layer::Aircraft);
    aircraft.energy = [energy(42.0); PERIODS];
    let selections = [
        selection(Layer::Road, vec![road.clone(), unlisted]),
        selection(Layer::Industry, vec![site.clone()]),
        aircraft,
    ];
    let traffic = |c: &Contributor| {
        (c.group_key == 1).then(|| {
            serde_json::json!({"aadt_light": 3000.0, "speed_kmh": 50.0, "road_class": "secondary"})
        })
    };
    let flights = ([energy(42.0); PERIODS], [energy(42.0) * 0.01; PERIODS]);
    let weather = flight_weather(&flights);
    let (lines, remainder) = click_lines(&selections, (&weather, flights), &traffic);
    let shares = row_shares(&selections, &[&road, &site], (&lines, remainder), &curves);
    let total: f64 = shares.listed.iter().sum::<f64>() + shares.aircraft + shares.rest;
    assert!((total - 1.0).abs() < 1e-12, "{shares:?}");
    assert!(shares.listed.iter().all(|&share| share > 0.0), "{shares:?}");
    assert!(shares.aircraft > 0.0 && shares.rest > 0.0, "{shares:?}");
    // The road is the loudest by far: the largest share.
    assert!(
        shares.listed[0] > shares.listed[1] && shares.listed[0] > shares.rest,
        "{shares:?}"
    );
}

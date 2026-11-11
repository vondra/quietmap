//! A synthetic two-country network with hand-computed factors.
use super::*;
use crate::sources::rail_traffic::train_scales;
use physics::bands::PERIODS;

const OBSERVED: u16 = 100;
const GUESSED: u16 = 0;
/// Timetabled rows within a tenth of their national train-km.
const SPREAD: Option<f64> = Some(0.1);
/// A row: its length (km), run trains and TEN-T tier.
type Row = (f64, Trains, Tier);

/// A row's run trains a day, all in the day period: (passenger, its source), (freight, its source).
fn trains(
    (passenger, passenger_source): (f64, u16),
    (freight, freight_source): (f64, u16),
) -> Trains {
    let mut day = [0.0; PERIODS];
    day[0] = passenger;
    let mut freight_day = [0.0; PERIODS];
    freight_day[0] = freight;
    Trains {
        passenger: day,
        freight: freight_day,
        missing_calendar_estimate: [[0.0; PERIODS]; 2],
        status: [0, 0],
        source: [passenger_source, freight_source],
    }
}

/// Country AA lies on the TEN-T map: 10 km of corridor, 20 km of network line, 40 km off it and
/// 5 km of counted freight; BB has no TEN-T line.
fn network() -> [(Vec<Row>, NationalTrainKm); 2] {
    let aa = vec![
        (
            10.0,
            trains((20.0, OBSERVED), (10.0, GUESSED)),
            Tier::Corridor,
        ),
        (
            20.0,
            trains((40.0, GUESSED), (10.0, GUESSED)),
            Tier::Network,
        ),
        (40.0, trains((10.0, GUESSED), (10.0, GUESSED)), Tier::Off),
        (5.0, trains((0.0, OBSERVED), (8.0, OBSERVED)), Tier::Off),
    ];
    let bb = vec![
        (30.0, trains((10.0, GUESSED), (4.0, GUESSED)), Tier::Off),
        (10.0, trains((30.0, OBSERVED), (0.0, GUESSED)), Tier::Off),
    ];
    [
        (
            aa,
            NationalTrainKm {
                passenger: Some((2024, 1000.0)),
                freight: Some((2024, 1000.0)),
            },
        ),
        (
            bb,
            NationalTrainKm {
                passenger: Some((2024, 900.0)),
                freight: Some((2024, 60.0)),
            },
        ),
    ]
}

fn mapped(rows: &[Row]) -> MappedTrainKm {
    let mut sum = MappedTrainKm::default();
    for (length_km, trains, tier) in rows {
        sum.add(*length_km, trains, *tier);
    }
    sum
}

/// The sources' own scaling of every row, summed: (passenger, freight) train-km a day.
fn scaled_train_km(rows: &[Row], value: &TrainScales) -> (f64, f64) {
    rows.iter()
        .fold((0.0, 0.0), |(passenger, freight), (km, trains, tier)| {
            let [freight_scale, passenger_scale] = train_scales(
                Some(value),
                RailType::Rail,
                [trains.source[1], trains.source[0]],
                *tier,
            );
            (
                passenger + trains.passenger.iter().sum::<f64>() * passenger_scale * km,
                freight + trains.freight.iter().sum::<f64>() * freight_scale * km,
            )
        })
}

/// AA: passenger observed 200, guessed 800 + 400; freight observed 40, guessed 400 off, 200 on the
/// network, 100 on the corridor. Freight (1000 - 40) / (0.5 * 400 + 2 * 200 + 4 * 100) = 0.96;
/// passengers (1000 - 200) / 1200. BB: freight 60 / 120 = 0.5 unweighted; passengers
/// (900 - 300) / 300 = 2 are never raised.
#[test]
fn two_countries_take_their_hand_computed_factors() {
    let [(aa, aa_national), (bb, bb_national)] = network();
    let aa_mapped = mapped(&aa);
    assert_eq!(aa_mapped.passenger, [200.0, 1200.0]);
    assert_eq!(aa_mapped.freight_observed, 40.0);
    assert_eq!(aa_mapped.freight_guessed, [400.0, 200.0, 100.0]);
    let aa_value = fitted(&aa_mapped, &aa_national, TEN_T_FREIGHT_WEIGHTS, SPREAD);
    assert!((aa_value.scales[0] - 0.96).abs() < 1e-12);
    assert!((aa_value.scales[1] - 800.0 / 1200.0).abs() < 1e-12);
    assert_eq!(aa_value.tier_weights, TEN_T_FREIGHT_WEIGHTS);
    let bb_value = fitted(&mapped(&bb), &bb_national, [1.0; 3], SPREAD);
    assert_eq!(bb_value.scales, [0.5, 1.0]);
    // Scaled as the sources scale them, AA runs its national train-km.
    let (passenger, freight) = scaled_train_km(&aa, &aa_value);
    assert!((passenger - 1000.0).abs() < 1e-9 && (freight - 1000.0).abs() < 1e-9);
}

/// Within the national total a corridor carries 8 times, and a network line 4 times, the guessed
/// freight of a line off the network with the same prior.
#[test]
fn the_ten_t_weights_move_freight_onto_the_corridors() {
    let [(aa, national), _] = network();
    let value = fitted(&mapped(&aa), &national, TEN_T_FREIGHT_WEIGHTS, SPREAD);
    let freight = |tier| train_scales(Some(&value), RailType::Rail, [GUESSED, GUESSED], tier)[0];
    assert!((freight(Tier::Corridor) / freight(Tier::Off) - 8.0).abs() < 1e-12);
    assert!((freight(Tier::Network) / freight(Tier::Off) - 4.0).abs() < 1e-12);
    // Without a published goods total the guessed 700 stay, only moved between tiers.
    let unpublished = NationalTrainKm {
        freight: None,
        ..national
    };
    let kept = fitted(&mapped(&aa), &unpublished, TEN_T_FREIGHT_WEIGHTS, SPREAD);
    assert!((scaled_train_km(&aa, &kept).1 - 740.0).abs() < 1e-9);
}

/// The passenger factor is continuous: timetables that alone reach the national total leave the
/// defaults standing, a residual within the timetables' spread lowers them in proportion, one
/// beyond it sets them; without timetables the national total does. Counted freight above the
/// goods total silences guessed freight.
#[test]
fn the_factors_are_bounded_and_the_passenger_factor_continuous() {
    let mapped = MappedTrainKm {
        passenger: [900.0, 400.0],
        freight_observed: 50.0,
        freight_guessed: [100.0, 0.0, 0.0],
    };
    let factors = |passenger: f64, spread| {
        let national = NationalTrainKm {
            passenger: Some((2024, passenger)),
            freight: Some((2024, 40.0)),
        };
        fitted(&mapped, &national, [1.0; 3], spread).scales
    };
    assert_eq!(factors(800.0, SPREAD), [0.0, 1.0]);
    assert_eq!(factors(900.0, SPREAD)[1], 1.0);
    // Residual 9 is a tenth of the spread's 90: 1 - 0.1 * (1 - 9 / 400).
    assert!((factors(909.0, SPREAD)[1] - (1.0 - 0.1 * (1.0 - 9.0 / 400.0))).abs() < 1e-12);
    assert!((factors(1000.0, SPREAD)[1] - 0.25).abs() < 1e-12);
    assert_eq!(factors(1000.0, None)[1], 1.0, "no measured spread");
    let untimetabled = MappedTrainKm {
        passenger: [0.0, 400.0],
        ..mapped
    };
    let national = NationalTrainKm {
        passenger: Some((2024, 100.0)),
        freight: None,
    };
    assert_eq!(
        fitted(&untimetabled, &national, [1.0; 3], None).scales[1],
        0.25
    );
}

/// The spread is the median deviation of the countries the timetables cover; a country run on
/// defaults or without a passenger total does not count.
#[test]
fn the_timetable_spread_is_measured_where_timetables_cover_the_country() {
    let country = |passenger: [f64; 2], national: Option<f64>| {
        (
            MappedTrainKm {
                passenger,
                ..MappedTrainKm::default()
            },
            NationalTrainKm {
                passenger: national.map(|train_km| (2024, train_km)),
                freight: None,
            },
        )
    };
    let countries = [
        country([100.0, 10.0], Some(110.0)),
        country([50.0, 5.0], Some(40.0)),
        country([10.0, 90.0], Some(50.0)),
        country([200.0, 0.0], Some(190.0)),
        country([300.0, 0.0], None),
    ];
    let spread = timetable_spread(
        countries
            .iter()
            .map(|(mapped, national)| (mapped, national)),
    );
    assert_eq!(spread, Some((100.0_f64 / 110.0 - 1.0).abs()));
    assert_eq!(
        timetable_spread(countries[2..3].iter().map(|(m, n)| (m, n))),
        None
    );
}

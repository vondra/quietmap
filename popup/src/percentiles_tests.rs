//! The time levels' tests: traffic by the hour, lines alone and together, the weather, events.

use super::*;
use crate::candidates::DisplayRef;
use crate::distribution::BIN_DB;
use crate::update::Contributor;
use physics::bands::energy;
use physics::percentile::{exceeded_level_db, relative_intensity};

/// The distributions of `selections` and `flights`, `fields` the contributors' displays.
fn distributions(
    selections: &[LayerSelection],
    flights: ([f64; PERIODS], [f64; PERIODS]),
    fields: &dyn Fn(&Contributor) -> Option<serde_json::Value>,
) -> [Distribution; PERIODS] {
    let weather = flight_weather(&flights);
    let (lines, steady) = click_lines(selections, (&weather, flights), fields);
    distributions_of(&lines, steady)
}

/// The time levels of `selections` with no flights and `fields` the contributors' displays.
fn levels(
    selections: &[LayerSelection],
    fields: &dyn Fn(&Contributor) -> Option<serde_json::Value>,
) -> Percentiles {
    Percentiles::of(&distributions(
        selections,
        ([0.0; PERIODS], [0.0; PERIODS]),
        fields,
    ))
}

pub(crate) fn contributor(key: u64, layer: Layer, leq_db: f64, distance_m: f64) -> Contributor {
    Contributor {
        group_key: key,
        layer,
        energy: [energy(leq_db); PERIODS],
        weather: {
            let mut weather = Weather::default();
            weather.add(&[energy(leq_db); PERIODS], &[[energy(leq_db); PERIODS]; 2]);
            weather
        },
        distance_m,
        display: DisplayRef {
            ring: 0,
            tile: 0,
            attribute: key as u32,
        },
        pieces: Vec::new(),
        lines: Vec::new(),
        heard: None,
        nden_sone: None,
        share: None,
    }
}

pub(crate) fn selection(layer: Layer, contributors: Vec<Contributor>) -> LayerSelection {
    let mut selection = LayerSelection::new(layer);
    for contributor in contributors {
        for p in 0..PERIODS {
            selection.energy[p] += contributor.energy[p];
        }
        selection
            .contributors
            .insert(contributor.group_key, contributor);
    }
    selection
}

/// 33 vehicles a day at 20 km/h, as the service road by the owner's hotel.
fn quiet_road(_: &Contributor) -> Option<serde_json::Value> {
    Some(
        serde_json::json!({"aadt_light": 31.0, "aadt_heavy": 1.0, "aadt_moto": 1.0,
        "speed_kmh": 20.0, "road_class": "service"}),
    )
}

/// A road runs at the day, evening and night shares its fields carry (its country's), and at
/// the fallback where sources built before them carry none.
#[test]
fn a_road_runs_at_the_period_shares_its_fields_carry() {
    let thai = serde_json::json!({"aadt_light": 800.0, "speed_kmh": 50.0,
        "road_class": "primary", "period_shares": [0.632, 0.182, 0.186]});
    let (per_hour, _, _) = traffic(Layer::Road, &thai, [1.0; PERIODS]).unwrap();
    assert!((per_hour[2] - 800.0 * 0.186 / PERIOD_HOURS[2]).abs() < 1e-9);
    let older = serde_json::json!({"aadt_light": 800.0, "speed_kmh": 50.0,
        "road_class": "primary"});
    let (per_hour, _, _) = traffic(Layer::Road, &older, [1.0; PERIODS]).unwrap();
    assert!((per_hour[2] - 800.0 * OTHER_PERIOD_SHARES[2] / PERIOD_HOURS[2]).abs() < 1e-9);
}

/// A local road alone at night is loudest in its morning hour, its flow then well over its
/// night mean; airport movements run in the periods their energy fell in, ground vehicles
/// counted.
#[test]
fn a_road_follows_its_hours_and_movements_their_periods() {
    let steady = Line {
        key: 1,
        energy: [1.0; PERIODS],
        weather: &Weather::default(),
        lambda: [1e6; PERIODS],
        profile: Some(2),
        duty: None,
    };
    let mut loudest = f64::NEG_INFINITY;
    let mut shares = 0.0;
    steady.own_levels(2, 0.0, &mut |level, share| {
        loudest = loudest.max(level);
        shares += share;
    });
    let peak = (0..8)
        .map(|slot| hour_factor(2, 2, slot))
        .fold(0.0, f64::max);
    assert!(
        peak > 2.0 && (loudest - 10.0 * peak.log10()).abs() < 1e-9,
        "{loudest}"
    );
    assert!((shares - 1.0).abs() < 1e-9);
    let cargo = serde_json::json!({"arrivals_per_day": 4.0, "departures_per_day": 4.0,
        "ground_vehicles_per_day": 2.0});
    let (per_hour, _, _) = traffic(Layer::Aircraft, &cargo, [0.0, 0.0, 1.0]).unwrap();
    assert_eq!(per_hour[0], 0.0);
    assert!((per_hour[2] - 10.0 / 8.0).abs() < 1e-12);
}

/// One sparse road alone: its levels are the quantiles of its line over the period's hours,
/// each hour at its share of the day's vehicles, far below its Leq most of the time.
#[test]
fn a_lone_sparse_road_has_its_lines_own_levels_over_the_hours() {
    let road = selection(Layer::Road, vec![contributor(7, Layer::Road, 40.0, 4.0)]);
    let levels = levels(&[road], &quiet_road);
    let lambda = 33.0 * OTHER_PERIOD_SHARES[0] / (PERIOD_HOURS[0] * 3_600.0) / (20.0 / 3.6) * 4.0;
    let mut pooled: Vec<f64> = (0..12)
        .flat_map(|slot| {
            let factor = hour_factor(2, 0, slot);
            (0..2_000).map(move |k| {
                energy(40.0)
                    * factor
                    * relative_intensity(lambda * factor, (k as f64 + 0.5) / 2_000.0)
            })
        })
        .collect();
    pooled.sort_by(f64::total_cmp);
    for (exceeded, level) in [
        (0.1, levels.l10[0]),
        (0.5, levels.l50[0]),
        (0.9, levels.l90[0]),
    ] {
        let at = ((1.0 - exceeded) * (pooled.len() - 1) as f64).round() as usize;
        let expected = 10.0 * pooled[at].log10();
        assert!(
            (level - expected).abs() < 0.5,
            "{exceeded}: {level} vs {expected}"
        );
    }
    assert!(levels.l50[0] < 25.0 && levels.l10[0] > levels.l50[0]);
}

/// A busy road's passes run together (lambda about 12), yet its 3 am carries a fifth of the
/// night's mean flow: its night L90 sits several dB under its Leq, where one rate for the
/// whole night kept it within 1 dB (Madrid's stations read L90 4.5 dB under the model's
/// beyond its level offset, evidence 2026-10-02).
#[test]
fn a_busy_roads_quiet_hours_lower_its_night_l90() {
    let road = selection(
        Layer::Road,
        vec![contributor(7, Layer::Road, 60.0, 2_000.0)],
    );
    let busy = |_: &Contributor| {
        Some(
            serde_json::json!({"aadt_light": 20_000.0, "speed_kmh": 50.0,
            "road_class": "secondary"}),
        )
    };
    let levels = levels(&[road], &busy);
    let constant_l90 = exceeded_level_db(60.0, 12.0, 0.9);
    assert!(constant_l90 > 59.0, "{constant_l90}");
    assert!(levels.l90[2] < 56.0, "{}", levels.l90[2]);
    assert!(levels.l10[2] > 60.5, "{}", levels.l10[2]);
}

/// A car every half hour 4 m away is heard as passes about twice an hour by day; a
/// motorway's 30,000 vehicles 300 m away run together into a steady sound; industry has no
/// passes.
#[test]
fn a_contributor_is_heard_as_its_passes_or_as_a_steady_sound() {
    let lane = contributor(7, Layer::Road, 40.0, 4.0);
    let heard_lane = heard(&lane, &quiet_road(&lane).unwrap()).unwrap();
    assert!((heard_lane.per_hour[0] - 33.0 * 0.70 / 12.0).abs() < 1e-9);
    assert!(!heard_lane.steady);
    let motorway = contributor(8, Layer::Road, 45.0, 300.0);
    let busy = serde_json::json!({"aadt_light": 30_000.0, "speed_kmh": 100.0,
        "road_class": "motorway"});
    let heard_motorway = heard(&motorway, &busy).unwrap();
    assert!((heard_motorway.per_hour[0] - 30_000.0 * 0.65 / 12.0).abs() < 1e-9);
    assert!(heard_motorway.steady);
    let industry = contributor(9, Layer::Industry, 45.0, 30.0);
    assert_eq!(heard(&industry, &busy), None);
}

/// Industry is steady; the same click gives the same levels whatever order the hash maps
/// hold the contributors in.
#[test]
fn steady_sources_hold_their_mean_and_a_click_repeats() {
    let industry = selection(
        Layer::Industry,
        vec![contributor(1, Layer::Industry, 45.0, 300.0)],
    );
    let industry_levels = levels(&[industry], &quiet_road);
    for level in [
        industry_levels.l10[0],
        industry_levels.l50[0],
        industry_levels.l90[0],
    ] {
        assert!((level - 45.0).abs() < 1e-9, "{level}");
    }
    let roads = |order: &[u64]| {
        let contributors = order
            .iter()
            .map(|&key| contributor(key, Layer::Road, 30.0 + key as f64, 5.0 * key as f64))
            .collect();
        levels(&[selection(Layer::Road, contributors)], &quiet_road)
    };
    let (first, second) = (roads(&[1, 2, 3, 4, 5]), roads(&[5, 3, 1, 4, 2]));
    assert_eq!(
        (first.l10, first.l50, first.l90),
        (second.l10, second.l50, second.l90)
    );
}

/// A site's two pieces on either side of the click, one always downwind (favourable, 10 dB
/// over calm) and the other never: its level holds at their sum, never all favourable or all
/// calm together (one share for the whole put L90 7 dB low and L10 3 dB high).
#[test]
fn pieces_on_either_side_keep_their_own_weather() {
    let mut site = contributor(1, Layer::Industry, 0.0, 300.0);
    let (calm, favourable) = (1.0, 10.0);
    site.weather = Weather::default();
    site.weather.add(
        &[favourable; PERIODS],
        &[[calm; PERIODS], [favourable; PERIODS]],
    );
    site.weather
        .add(&[calm; PERIODS], &[[calm; PERIODS], [favourable; PERIODS]]);
    site.energy = [favourable + calm; PERIODS];
    let levels = levels(&[selection(Layer::Industry, vec![site])], &|_| None);
    for p in 0..PERIODS {
        for level in [levels.l5[p], levels.l50[p], levels.l90[p]] {
            assert!(
                (level - 11f64.log10() * 10.0).abs() <= BIN_DB / 2.0,
                "{level}"
            );
        }
    }
}

/// A steady source alone keeps its weather: favourable (50 dB) half the time and calm (40 dB)
/// the other half, it is at each for half the period, not at their mean.
#[test]
fn a_source_alone_keeps_its_weather() {
    let mut plant = contributor(1, Layer::Industry, 0.0, 300.0);
    let (calm, favourable) = (energy(40.0), energy(50.0));
    let mean = 0.5 * (calm + favourable);
    plant.weather = Weather::default();
    plant
        .weather
        .add(&[mean; PERIODS], &[[calm; PERIODS], [favourable; PERIODS]]);
    plant.energy = [mean; PERIODS];
    let line = Line::of(&plant, None).unwrap();
    let mut levels: Vec<(f64, f64)> = Vec::new();
    line.own_levels(0, 0.0, &mut |level, share| levels.push((level, share)));
    let at = |db: f64| -> f64 {
        levels
            .iter()
            .filter(|(level, _)| (level - db).abs() < 1e-9)
            .map(|(_, share)| share)
            .sum()
    };
    assert!(
        (at(40.0) - 0.5).abs() < 1e-9 && (at(50.0) - 0.5).abs() < 1e-9,
        "{levels:?}"
    );
}

/// A source's favourable share moves its levels smoothly: 24.9 % and 25.1 % of the time
/// favourable (10 dB over calm) give nearly the same spread (ten states read at their middles
/// jumped there by a whole state).
#[test]
fn the_weather_moves_the_levels_smoothly() {
    let spread = |share: f64| {
        let mut site = contributor(1, Layer::Industry, 0.0, 300.0);
        let (calm, favourable) = (1.0, 10.0);
        let mean = share * favourable + (1.0 - share) * calm;
        site.weather = Weather::default();
        site.weather
            .add(&[mean; PERIODS], &[[calm; PERIODS], [favourable; PERIODS]]);
        site.energy = [mean; PERIODS];
        distributions(
            &[selection(Layer::Industry, vec![site])],
            ([0.0; PERIODS], [0.0; PERIODS]),
            &|_| None,
        )
    };
    let (below, above) = (spread(0.249), spread(0.251));
    let mean = |distribution: &Distribution| -> f64 {
        distribution
            .levels()
            .map(|(level, share)| share * 10f64.powf(level / 10.0))
            .sum()
    };
    assert!((mean(&above[0]) / mean(&below[0]) - 1.0).abs() < 0.01);
    for exceeded in [0.1, 0.5, 0.9] {
        let step = above[0].exceeded_db(exceeded) - below[0].exceeded_db(exceeded);
        assert!(step.abs() < 0.2, "{exceeded}: {step}");
    }
}

/// Church bells sounding 2 % of the day at a mean of 40 dB beside a steady 40 dB: the
/// percentiles keep the steady 40 (a steady source of the bells' energy would put L50 at 43)
/// but for the bells' own 2 %, and the bells are heard as their rings a day.
#[test]
fn events_sound_for_their_duty_and_are_silent_otherwise() {
    let bells_fields = |c: &Contributor| {
        (c.group_key == 2).then(
            || serde_json::json!({"events_per_day": [36.0, 0.0, 3.0], "duty": [0.02, 0.0, 0.01]}),
        )
    };
    let plant = contributor(1, Layer::Industry, 40.0, 50.0);
    let bells = contributor(2, Layer::Building, 40.0, 80.0);
    let levels = levels(
        &[
            selection(Layer::Industry, vec![plant]),
            selection(Layer::Building, vec![bells.clone()]),
        ],
        &bells_fields,
    );
    for level in [levels.l5[0], levels.l50[0], levels.l90[0]] {
        assert!((level - 40.0).abs() < 1e-9, "{level}");
    }
    let distribution = &distributions(
        &[
            selection(
                Layer::Industry,
                vec![contributor(1, Layer::Industry, 40.0, 50.0)],
            ),
            selection(Layer::Building, vec![bells.clone()]),
        ],
        ([0.0; PERIODS], [0.0; PERIODS]),
        &bells_fields,
    )[0];
    // Ringing, 40 dB over the duty's 2 %: 40 + 10 log10(1 + 50) for 2 % of the day.
    let ringing = 10.0 * 51f64.log10() + 40.0;
    assert!((distribution.exceeded_db(0.019) - ringing).abs() <= BIN_DB / 2.0);
    let fields = bells_fields(&bells).unwrap();
    let heard_bells = heard(&bells, &fields).unwrap();
    assert!(!heard_bells.steady && (heard_bells.per_hour[0] - 3.0).abs() < 1e-9);
}

//! The time levels' tests: traffic by the hour, lines alone and together, the weather, events.

use super::traffic::OTHER_PERIOD_SHARES;
use super::*;
use crate::candidates::DisplayRef;
use crate::update::Contributor;
use physics::bands::energy;
use physics::percentile::relative_intensity;

const BIN_DB: f64 = Resolution::CLICK.bin_db;

/// The distributions of `selections` without flights, `fields` the contributors' displays.
fn distributions(
    selections: &[LayerSelection],
    fields: &dyn Fn(&Group) -> Result<serde_json::Value, String>,
) -> [Distribution; PERIODS] {
    let passes = |group: &Group| fields(group).map(|fields| Passes::read(group.layer, &fields));
    let (lines, steady) = click_lines(selections, &[], &passes).unwrap();
    distributions_of(&lines, steady, Resolution::CLICK)
}

/// The time levels of `selections` with no flights and `fields` the contributors' displays.
fn levels(
    selections: &[LayerSelection],
    fields: &dyn Fn(&Group) -> Result<serde_json::Value, String>,
) -> Percentiles {
    Percentiles::of(&distributions(selections, fields))
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
        slant_distance_m: distance_m,
        display: DisplayRef {
            ring: 0,
            tile: 0,
            attribute: key as u32,
        },
        display_lden: energy(leq_db),
        pieces: Vec::new(),
        lines: Vec::new(),
        heard: None,
        nden_sone: None,
        share: None,
        path: Default::default(),
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

/// 33 vehicles a day at 20 km/h, as a quiet service road to a hotel.
fn quiet_road(_: &Group) -> Result<serde_json::Value, String> {
    Ok(
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
    let (per_hour, _, _) = Passes::read(Layer::Road, &thai)
        .traffic([1.0; PERIODS])
        .unwrap();
    assert!((per_hour[2] - 800.0 * 0.186 / PERIOD_HOURS[2]).abs() < 1e-9);
    let older = serde_json::json!({"aadt_light": 800.0, "speed_kmh": 50.0,
        "road_class": "primary"});
    let (per_hour, _, _) = Passes::read(Layer::Road, &older)
        .traffic([1.0; PERIODS])
        .unwrap();
    assert!((per_hour[2] - 800.0 * OTHER_PERIOD_SHARES[2] / PERIOD_HOURS[2]).abs() < 1e-9);
}

/// A local road alone at night is loudest in its morning hour, its flow then well over its
/// night mean; airport movements run in the periods their sound exposure fell in, ground vehicles
/// counted: at an even mean power all day, at one rate every hour (the mean power alone put
/// three times the day's rate into the evening's four hours).
#[test]
fn a_road_follows_its_hours_and_movements_their_periods() {
    let steady = Line {
        key: 1,
        energy: [1.0; PERIODS],
        means: [[1.0; WEATHER_STATES]; PERIODS],
        lambda: [1e6; PERIODS],
        profile: Some(2),
        duty: None,
        passing: None,
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
    let (per_hour, _, _) = Passes::read(Layer::Aircraft, &cargo)
        .traffic([0.0, 0.0, 1.0])
        .unwrap();
    assert_eq!(per_hour[0], 0.0);
    assert!((per_hour[2] - 10.0 / 8.0).abs() < 1e-12);
    let (per_hour, _, _) = Passes::read(Layer::Aircraft, &cargo)
        .traffic([1.0; PERIODS])
        .unwrap();
    for rate in per_hour {
        assert!((rate - 10.0 / 24.0).abs() < 1e-12, "{per_hour:?}");
    }
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
    let busy = |_: &Group| {
        Ok(
            serde_json::json!({"aadt_light": 20_000.0, "speed_kmh": 50.0,
            "road_class": "secondary"}),
        )
    };
    let levels = levels(&[road], &busy);
    let constant_l90 = 60.0 + 10.0 * relative_intensity(12.0, 0.1).log10();
    assert!(constant_l90 > 59.0, "{constant_l90}");
    assert!(levels.l90[2] < 56.0, "{}", levels.l90[2]);
    assert!(levels.l10[2] > 60.5, "{}", levels.l10[2]);
}

/// A car every half hour 4 m away is heard as passes about twice an hour by day; a
/// motorway's 30,000 vehicles 300 m away run together into a steady sound; industry has no
/// passes; a dense road's vehicles run together over its axis, 4 m below the receiver.
#[test]
fn a_contributor_is_heard_as_its_passes_or_as_a_steady_sound() {
    let lane = contributor(7, Layer::Road, 40.0, 4.0);
    let heard_lane = passes_at(&lane, &quiet_road(&lane.group()).unwrap()).unwrap();
    assert!((heard_lane.per_hour[0] - 33.0 * 0.70 / 12.0).abs() < 1e-9);
    assert!(!heard_lane.steady);
    let motorway = contributor(8, Layer::Road, 45.0, 300.0);
    let busy = serde_json::json!({"aadt_light": 30_000.0, "speed_kmh": 100.0,
        "road_class": "motorway"});
    let heard_motorway = passes_at(&motorway, &busy).unwrap();
    assert!((heard_motorway.per_hour[0] - 30_000.0 * 0.65 / 12.0).abs() < 1e-9);
    assert!(heard_motorway.steady);
    let industry = contributor(9, Layer::Industry, 45.0, 30.0);
    assert_eq!(passes_at(&industry, &busy), None);
    // Over a road's axis its vehicles pass 4 m from the receiver, not none: 12,833 an hour at
    // 50 km/h run together there (Kurze's lambda 1.03), though none is within the horizontal 0 m.
    let mut axis = contributor(10, Layer::Road, 70.0, 0.0);
    axis.slant_distance_m = 4.0;
    let dense = serde_json::json!({"aadt_light": 220_000.0, "speed_kmh": 50.0,
        "road_class": "primary"});
    assert!(passes_at(&axis, &dense).unwrap().steady);
    let line = Line::of(&axis.group(), &Passes::read(Layer::Road, &dense)).unwrap();
    assert!((line.lambda[0] - 220_000.0 * 0.70 / 12.0 / 3_600.0 / (50.0 / 3.6) * 4.0).abs() < 1e-9);
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
    let levels = levels(&[selection(Layer::Industry, vec![site])], &|_| {
        Ok(serde_json::Value::Null)
    });
    for p in 0..PERIODS {
        for level in [levels.l5[p], levels.l50[p], levels.l90[p]] {
            assert!(
                (level - 11f64.log10() * 10.0).abs() <= BIN_DB / 2.0,
                "{level}"
            );
        }
    }
}

/// Two pieces in one share bin, one louder downwind and one louder in calm air (tree crowns can
/// take more from the arc): the weather states still average to their mean (summed together the
/// states' difference cancelled, and the mean read 0.29 dB loud).
#[test]
fn pieces_louder_in_opposite_states_keep_their_mean() {
    let mut site = contributor(1, Layer::Industry, 0.0, 300.0);
    site.weather = Weather::default();
    let mut total = 0.0;
    for (calm, downwind, share) in [(1.0, 10.0, 0.41), (10.0, 1.0, 0.49)] {
        let mean = calm + share * (downwind - calm);
        site.weather
            .add(&[mean; PERIODS], &[[calm; PERIODS], [downwind; PERIODS]]);
        total += mean;
    }
    site.energy = [total; PERIODS];
    let line = Line::of(
        &site.group(),
        &Passes::read(site.layer, &serde_json::Value::Null),
    )
    .unwrap();
    let average = line.state_means(0).iter().sum::<f64>() / WEATHER_STATES as f64;
    assert!((average - total).abs() < 1e-9, "{average} vs {total}");
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
    let line = Line::of(
        &plant.group(),
        &Passes::read(plant.layer, &serde_json::Value::Null),
    )
    .unwrap();
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
        distributions(&[selection(Layer::Industry, vec![site])], &|_| {
            Ok(serde_json::Value::Null)
        })
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
    let bells_fields = |c: &Group| {
        Ok(if c.key == 2 {
            serde_json::json!({"events_per_day": [36.0, 0.0, 3.0], "duty": [0.02, 0.0, 0.01]})
        } else {
            serde_json::Value::Null
        })
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
        &bells_fields,
    )[0];
    // Ringing, 40 dB over the duty's 2 %: 40 + 10 log10(1 + 50) for 2 % of the day.
    let ringing = 10.0 * 51f64.log10() + 40.0;
    assert!((distribution.exceeded_db(0.019) - ringing).abs() <= BIN_DB / 2.0);
    let fields = bells_fields(&bells.group()).unwrap();
    let heard_bells = passes_at(&bells, &fields).unwrap();
    assert!(!heard_bells.steady && (heard_bells.per_hour[0] - 3.0).abs() < 1e-9);
}

/// The flights heard as passes: a class in progress 2 % of the day at a mean of 40 dB beside a
/// steady 40 dB sounds 2 % of the time at 40 + 10 lg 51 dB and leaves the rest at 40 (one Kurze
/// line of that energy put every moment above 40); the same energy in progress ten times as
/// long moves the level longer and less; a class of 80 passes in progress on average holds its
/// mean. A faint class beside a loud one is silent between its own passes: both in progress 1 %
/// of the day at 55 and 100 dB a pass, the level exceeded half and nine tenths of the time
/// stays the steady 30 dB (held at its mean beside the loud one, the faint one hummed at 35).
#[test]
fn flights_are_heard_while_they_pass() {
    use crate::aircraft::FlightPasses;
    let heard = |classes: &[(f64, f64)], steady_db: f64| {
        let mut flights = FlightPasses::default();
        for &(passing, leq_db) in classes {
            flights.add_pass(0, energy(leq_db), passing);
        }
        let passes = |group: &Group| Ok(Passes::read(group.layer, &serde_json::Value::Null));
        let plant = selection(
            Layer::Industry,
            vec![contributor(1, Layer::Industry, steady_db, 50.0)],
        );
        let (lines, steady) =
            click_lines(&[plant], &Line::flight_passes(&flights), &passes).unwrap();
        distributions_of(&lines, steady, Resolution::CLICK)[0].clone()
    };
    let day = |passing: f64, leq_db: f64| heard(&[(passing, leq_db)], 40.0);
    let mean_db = |distribution: &Distribution| {
        10.0 * (distribution.levels())
            .map(|(level, share)| share * energy(level))
            .sum::<f64>()
            .log10()
    };
    let rare = day(0.02, 40.0);
    let passing_db = 10.0 * 51f64.log10() + 40.0;
    assert!((rare.exceeded_db(0.015) - passing_db).abs() <= BIN_DB / 2.0);
    assert!((rare.exceeded_db(0.05) - 40.0).abs() < 1e-9);
    assert!((rare.exceeded_db(0.9) - 40.0).abs() < 1e-9);
    let often = day(0.2, 40.0);
    assert!((often.exceeded_db(0.1) - (10.0 * 6f64.log10() + 40.0)).abs() <= BIN_DB / 2.0);
    for (distribution, flights_db) in [(&rare, 40.0), (&often, 40.0)] {
        let whole = 10.0 * (energy(40.0) + energy(flights_db)).log10();
        assert!((mean_db(distribution) - whole).abs() < 1e-6);
    }
    let dense = day(80.0, 40.0);
    let whole = 10.0 * (2.0 * energy(40.0)).log10();
    assert!((dense.exceeded_db(0.01) - whole).abs() < 1e-9);
    assert!((dense.exceeded_db(0.99) - whole).abs() < 1e-9);
    let two = heard(&[(0.01, 35.0), (0.01, 80.0)], 30.0);
    for exceeded in [0.5, 0.9] {
        assert!((two.exceeded_db(exceeded) - 30.0).abs() < 1e-9);
    }
    assert!((two.exceeded_db(0.015) - 55.0).abs() <= BIN_DB);
}

/// Consecutive weather states whose every line stays within the spread take their mean (the
/// period's energy kept); a line apart by more keeps its states apart from there, in every layer.
#[test]
fn close_weather_states_merge_and_keep_their_energy() {
    let line = |means: [f64; WEATHER_STATES]| Line {
        key: 1,
        energy: [means.iter().sum::<f64>() / WEATHER_STATES as f64; PERIODS],
        means: [means; PERIODS],
        lambda: [1e6; PERIODS],
        profile: None,
        duty: None,
        passing: None,
    };
    let near = line([1.10, 1.05, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0]);
    let far = line([4.0, 4.0, 4.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0]);
    let mut layers = vec![
        (vec![near.clone()], [0.0; PERIODS]),
        (vec![far.clone()], [0.0; PERIODS]),
    ];
    merge_close_states(&mut layers, 1.2);
    let (merged_near, merged_far) = (&layers[0].0[0].means[0], &layers[1].0[0].means[0]);
    // The far line splits the states at 3; the near one's first three take their mean.
    assert_eq!(merged_far, &far.means[0]);
    assert_eq!(merged_near[0], merged_near[2]);
    assert!((merged_near[0] - (1.10 + 1.05 + 1.0) / 3.0).abs() < 1e-12);
    assert_eq!(&merged_near[3..], &near.means[0][3..]);
    for (lines, _) in &layers {
        let energy = lines[0].means[0].iter().sum::<f64>() / WEATHER_STATES as f64;
        assert!((energy - lines[0].energy[0]).abs() < 1e-12);
    }
    // Apart by more than the spread, no state merges.
    let mut apart = vec![(
        vec![line([
            2.0, 1.5, 1.0, 0.7, 0.5, 0.35, 0.25, 0.18, 0.12, 0.09,
        ])],
        [0.0; PERIODS],
    )];
    let before = apart[0].0[0].means;
    merge_close_states(&mut apart, 1.2);
    assert_eq!(apart[0].0[0].means, before);
}

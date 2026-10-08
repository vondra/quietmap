//! CNOSSOS-EU road traffic emission (Directive 2015/996 Annex II 2.2 with the coefficients of
//! Delegated Directive 2021/1226): rolling and propulsion noise per vehicle category and octave
//! band, summed into the sound power per metre of a traffic flow, with the road gradient's
//! propulsion term (2.2.4), the stop-and-go terms near traffic lights and roundabouts (2.2.5) and
//! the air temperature's rolling term (2.2.2). Battery-electric cars, an open category of the
//! method, roll like any car and drive without propulsion noise. There is no studded-tyre term.

use super::road_surface::RoadSurface;
use crate::bands::BANDS;

/// Reference speed of the rolling and propulsion laws (km/h).
pub const REFERENCE_SPEED_KMH: f64 = 70.0;
/// The laws are evaluated at no less than this speed (km/h), and are valid on all speed ranges
/// above it (2.2.2); the vehicle density uses the real speed.
const MINIMUM_LAW_SPEED_KMH: f64 = 20.0;

/// CNOSSOS-EU vehicle categories 1, 2, 3 and 4b.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VehicleCategory {
    Light,
    Medium,
    Heavy,
    Motorcycle,
}

struct Coefficients {
    rolling: Option<([f64; BANDS], [f64; BANDS])>,
    propulsion: ([f64; BANDS], [f64; BANDS]),
}

const LIGHT: Coefficients = Coefficients {
    rolling: Some((
        [83.1, 89.2, 87.7, 93.1, 100.1, 96.7, 86.8, 76.2],
        [30.0, 41.5, 38.9, 25.7, 32.5, 37.2, 39.0, 40.0],
    )),
    propulsion: (
        [97.9, 92.5, 90.7, 87.2, 84.7, 88.0, 84.4, 77.1],
        [-1.3, 7.2, 7.7, 8.0, 8.0, 8.0, 8.0, 8.0],
    ),
};
const MEDIUM: Coefficients = Coefficients {
    rolling: Some((
        [88.7, 93.2, 95.7, 100.9, 101.7, 95.1, 87.8, 83.6],
        [30.0, 35.8, 32.6, 23.8, 30.1, 36.2, 38.3, 40.1],
    )),
    propulsion: (
        [105.5, 100.2, 100.5, 98.7, 101.0, 97.8, 91.2, 85.0],
        [-1.9, 4.7, 6.4, 6.5, 6.5, 6.5, 6.5, 6.5],
    ),
};
const HEAVY: Coefficients = Coefficients {
    rolling: Some((
        [91.7, 96.2, 98.2, 104.9, 105.1, 98.5, 91.1, 85.6],
        [30.0, 33.5, 31.3, 25.4, 31.8, 37.1, 38.6, 40.6],
    )),
    propulsion: (
        [108.8, 104.2, 103.5, 102.9, 102.6, 98.5, 93.8, 87.5],
        [0.0, 3.0, 4.6, 5.0, 5.0, 5.0, 5.0, 5.0],
    ),
};
const MOTORCYCLE: Coefficients = Coefficients {
    rolling: None,
    propulsion: (
        [99.9, 101.9, 96.7, 94.4, 95.2, 94.7, 92.1, 88.6],
        [3.2, 5.9, 11.9, 11.6, 11.5, 12.6, 11.1, 12.0],
    ),
};

impl VehicleCategory {
    fn coefficients(self) -> &'static Coefficients {
        match self {
            VehicleCategory::Light => &LIGHT,
            VehicleCategory::Medium => &MEDIUM,
            VehicleCategory::Heavy => &HEAVY,
            VehicleCategory::Motorcycle => &MOTORCYCLE,
        }
    }
}

/// A flow of one category.
#[derive(Debug, Clone, Copy)]
pub struct CategoryFlow {
    pub vehicles_per_hour: f64,
    /// The category's own mean speed (the caller holds heavy vehicles to their limit).
    pub speed_kmh: f64,
    pub category: VehicleCategory,
    /// The road's slope in the flow's direction (%, positive uphill).
    pub slope_percent: f64,
    /// The nearest junction that stops and starts the flow, and the distance to it (m).
    pub junction: Option<(Junction, f64)>,
    /// Of a light vehicle flow, the share driving on batteries: their tyres roll as any car's,
    /// their drive adds no propulsion noise. Other categories ignore it.
    pub electric_share: f64,
}

/// The junctions of CNOSSOS-EU 2.2.5 (Table F-3: k = 1 and 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Junction {
    TrafficLights,
    Roundabout,
}

/// The junction corrections fade linearly to nothing over this distance from the junction (m).
pub const JUNCTION_REACH_M: f64 = 100.0;

/// CNOSSOS-EU 2.2.5 (Eqs. 2.2.17-2.2.18, Table F-3): the rolling and propulsion corrections (dB,
/// every band) of a category `distance_m` from a junction: vehicles braking and pulling away roll
/// quieter and drive their engines harder. The table has none for motorcycles; they follow the
/// cars' level at a junction (see `motorcycle_gain_db`).
pub fn junction_correction_db(
    category: VehicleCategory,
    junction: Option<(Junction, f64)>,
) -> (f64, f64) {
    let Some((kind, distance_m)) = junction else {
        return (0.0, 0.0);
    };
    let fade = junction_fade(Some((kind, distance_m)));
    let (rolling, propulsion) = match (category, kind) {
        (VehicleCategory::Light, Junction::TrafficLights) => (-4.5, 5.5),
        (VehicleCategory::Light, Junction::Roundabout) => (-4.4, 3.1),
        (VehicleCategory::Medium | VehicleCategory::Heavy, Junction::TrafficLights) => (-4.0, 9.0),
        (VehicleCategory::Medium | VehicleCategory::Heavy, Junction::Roundabout) => (-2.3, 6.7),
        (VehicleCategory::Motorcycle, _) => (0.0, 0.0),
    };
    (fade * rolling, fade * propulsion)
}

/// Slopes steeper than this count as this (2.2.13-2.2.15: Min(12 %; s)).
const SLOPE_LIMIT_PERCENT: f64 = 12.0;

/// CNOSSOS-EU 2.2.4 (Eqs. 2.2.13-2.2.15): the propulsion correction (dB, every band) of a
/// category climbing (`slope` > 0) or descending a road at `speed_kmh`; it holds the effect of the
/// slope on the speed too. Motorcycles have none.
pub fn gradient_correction_db(category: VehicleCategory, slope: f64, speed_kmh: f64) -> f64 {
    let (up, down) = (
        slope.min(SLOPE_LIMIT_PERCENT),
        (-slope).min(SLOPE_LIMIT_PERCENT),
    );
    match category {
        VehicleCategory::Light if slope < -6.0 => (down - 6.0) / 1.0,
        VehicleCategory::Light if slope > 2.0 => (up - 2.0) / 1.5 * speed_kmh / 100.0,
        VehicleCategory::Medium if slope < -4.0 => (down - 4.0) / 0.7 * (speed_kmh - 20.0) / 100.0,
        VehicleCategory::Medium if slope > 0.0 => up / 1.0 * speed_kmh / 100.0,
        VehicleCategory::Heavy if slope < -4.0 => (down - 4.0) / 0.5 * (speed_kmh - 10.0) / 100.0,
        VehicleCategory::Heavy if slope > 0.0 => up / 0.8 * speed_kmh / 100.0,
        _ => 0.0,
    }
}

/// The yearly average air temperature at which the road surface corrections hold (C), Eq. 2.2.10.
pub const REFERENCE_AIR_TEMPERATURE_C: f64 = 20.0;

/// CNOSSOS-EU 2.2.2 (Eq. 2.2.10): the rolling noise correction (dB, every band) of a category on
/// a road whose yearly average air temperature is `air_temperature_c`: K = 0.08 dB/C for light
/// vehicles and 0.04 for medium and heavy ones times (20 C - t), louder in colder climates.
/// Motorcycles roll no noise of their own.
pub fn temperature_correction_db(category: VehicleCategory, air_temperature_c: f64) -> f64 {
    let k = match category {
        VehicleCategory::Light => 0.08,
        VehicleCategory::Medium | VehicleCategory::Heavy => 0.04,
        VehicleCategory::Motorcycle => 0.0,
    };
    k * (REFERENCE_AIR_TEMPERATURE_C - air_temperature_c)
}

/// Motorcycles emit this much more than light vehicles at the same speed: the Japanese standard
/// model ASJ RTN-Model 2018 (Sakamoto, Acoust. Sci. & Tech. 41(3), 2020, Table 2.3, dense asphalt,
/// steady flow at 40-140 km/h: motorcycles and mopeds 49.6 + 30 lg V against light vehicles'
/// 45.8, the same speed law), from Japanese pass-by measurements; CNOSSOS-EU's category 4b puts
/// them 3.5 dB over cars at 20 km/h and 0.9-2.4 dB under them at 50-110 km/h.
pub const MOTORCYCLE_OVER_LIGHT_DB: f64 = 3.8;
/// The same where traffic stops and starts (ASJ RTN-Model 2018 Table 2.3, non-steady flow at
/// 10-60 km/h: 85.2 + 10 lg V against 82.3): motorcycles pull away like cars, 2.9 dB louder.
pub const MOTORCYCLE_OVER_LIGHT_NON_STEADY_DB: f64 = 2.9;

/// A-weighted sum (dB) of per-vehicle band energies.
fn a_weighted_db(bands: &[f64; BANDS]) -> f64 {
    10.0 * (0..BANDS)
        .map(|band| bands[band] * 10f64.powf(crate::bands::A_WEIGHTING_DB[band] / 10.0))
        .sum::<f64>()
        .log10()
}

/// One vehicle's band energies of a category at a law speed with a junction's rolling and
/// propulsion corrections (dB) and no other.
fn vehicle_at_junction(
    category: VehicleCategory,
    law_speed: f64,
    (rolling_db, propulsion_db): (f64, f64),
) -> [f64; BANDS] {
    let coefficients = category.coefficients();
    let (log_ratio, relative) = (
        (law_speed / REFERENCE_SPEED_KMH).log10(),
        (law_speed - REFERENCE_SPEED_KMH) / REFERENCE_SPEED_KMH,
    );
    std::array::from_fn(|band| {
        let (a_p, b_p) = coefficients.propulsion;
        let mut energy = 10f64.powf((a_p[band] + b_p[band] * relative + propulsion_db) / 10.0);
        if let Some((a_r, b_r)) = coefficients.rolling {
            energy += 10f64.powf((a_r[band] + b_r[band] * log_ratio + rolling_db) / 10.0);
        }
        energy
    })
}

/// How far into a junction's reach a flow is: 1 at the junction, 0 from `JUNCTION_REACH_M` on.
fn junction_fade(junction: Option<(Junction, f64)>) -> f64 {
    junction.map_or(0.0, |(_, distance_m)| {
        (1.0 - distance_m.abs() / JUNCTION_REACH_M).max(0.0)
    })
}

/// The A-weighted gain (dB) that takes a motorcycle of category 4b to its level over a light
/// vehicle at the same speed (ASJ RTN-Model 2018): a car's level with the junction's stop and
/// start terms (CNOSSOS-EU 2.2.5), plus 3.8 dB in steady flow, 2.9 dB at the junction.
fn motorcycle_gain_db(law_speed: f64, junction: Option<(Junction, f64)>) -> f64 {
    let car = vehicle_at_junction(
        VehicleCategory::Light,
        law_speed,
        junction_correction_db(VehicleCategory::Light, junction),
    );
    let own = vehicle_at_junction(VehicleCategory::Motorcycle, law_speed, (0.0, 0.0));
    let over = MOTORCYCLE_OVER_LIGHT_DB
        + (MOTORCYCLE_OVER_LIGHT_NON_STEADY_DB - MOTORCYCLE_OVER_LIGHT_DB)
            * junction_fade(junction);
    a_weighted_db(&car) + over - a_weighted_db(&own)
}

/// Sound power per metre (dB, Z-weighted) of a mix of flows on `surface` (2.2.6); `-inf` in every
/// band when silent. The air temperature (2.2.10) applies to rolling noise only (motorcycles have
/// none).
pub fn line_emission_db(
    flows: &[CategoryFlow],
    (surface, air_temperature_c): (&RoadSurface, f64),
) -> [f64; BANDS] {
    let mut energy = [0.0f64; BANDS];
    for flow in flows.iter().filter(|flow| flow.vehicles_per_hour > 0.0) {
        let speed = flow.speed_kmh;
        let coefficients = flow.category.coefficients();
        let law_speed = speed.max(MINIMUM_LAW_SPEED_KMH);
        let (log_ratio, relative) = (
            (law_speed / REFERENCE_SPEED_KMH).log10(),
            (law_speed - REFERENCE_SPEED_KMH) / REFERENCE_SPEED_KMH,
        );
        // Vehicles per metre: Q / (1000 v) with Q per hour and v in km/h.
        let density = flow.vehicles_per_hour / (1000.0 * speed);
        let gradient = gradient_correction_db(flow.category, flow.slope_percent, law_speed);
        let (junction_rolling, junction_propulsion) =
            junction_correction_db(flow.category, flow.junction);
        let rolling_correction =
            junction_rolling + temperature_correction_db(flow.category, air_temperature_c);
        let combustion = match flow.category {
            VehicleCategory::Light => 1.0 - flow.electric_share.clamp(0.0, 1.0),
            _ => 1.0,
        };
        // A motorcycle keeps category 4b's spectrum at the level ASJ measures over a car, pulling
        // away from a junction like one.
        let motorcycle_scale = if flow.category == VehicleCategory::Motorcycle {
            10f64.powf(motorcycle_gain_db(law_speed, flow.junction) / 10.0)
        } else {
            1.0
        };
        for band in 0..BANDS {
            let (surface_rolling, surface_propulsion) =
                surface.corrections(flow.category, speed, band);
            let (a_p, b_p) = coefficients.propulsion;
            let mut vehicle = combustion
                * 10f64.powf(
                    (a_p[band]
                        + b_p[band] * relative
                        + gradient
                        + junction_propulsion
                        + surface_propulsion)
                        / 10.0,
                );
            if let Some((a_r, b_r)) = coefficients.rolling {
                vehicle += 10f64.powf(
                    (a_r[band] + b_r[band] * log_ratio + rolling_correction + surface_rolling)
                        / 10.0,
                );
            }
            energy[band] += density * vehicle * motorcycle_scale;
        }
    }
    energy.map(|e| {
        if e > 0.0 {
            10.0 * e.log10()
        } else {
            f64::NEG_INFINITY
        }
    })
}

#[cfg(test)]
mod tests {
    use super::super::road_surface::REFERENCE_SURFACE;
    use super::*;
    use crate::bands::{a_weighted_energy, level_db};

    /// Eq. 2.2.10: Prague's 9.4 C adds 0.85 dB to a light vehicle's rolling noise and 0.42 dB to a
    /// heavy one's, Singapore's 26.7 C takes 0.54 and 0.27 dB off; a 50 km/h car flow's A-weighted
    /// level moves 0.8 dB in Prague (rolling dominates), motorcycles not at all.
    #[test]
    fn colder_places_roll_louder() {
        assert!((temperature_correction_db(VehicleCategory::Light, 9.4) - 0.848).abs() < 1e-9);
        assert!((temperature_correction_db(VehicleCategory::Heavy, 9.4) - 0.424).abs() < 1e-9);
        assert!((temperature_correction_db(VehicleCategory::Light, 26.7) + 0.536).abs() < 1e-9);
        assert_eq!(
            temperature_correction_db(VehicleCategory::Motorcycle, 0.0),
            0.0
        );
        let cars = flow(500.0, 50.0, VehicleCategory::Light);
        let level = |temperature: f64| {
            level_db(a_weighted_energy(&line_emission_db(
                &cars,
                (&REFERENCE_SURFACE, temperature),
            )))
        };
        let prague = level(9.4) - level(REFERENCE_AIR_TEMPERATURE_C);
        assert!((0.7..0.85).contains(&prague), "{prague}");
    }

    /// Battery-electric cars keep their rolling noise only: a 50 km/h car flow all electric reads
    /// 0.9 dB below a combustion one, at 30 km/h 2.2 dB; Norway's 27.6 % of its fleet takes
    /// 0.23 dB off at 50 km/h. Heavy vehicles ignore the share.
    #[test]
    fn electric_cars_roll_without_propulsion_noise() {
        let level = |speed: f64, electric_share: f64, category| {
            a_weighted(
                &[CategoryFlow {
                    electric_share,
                    ..flow(500.0, speed, category)[0]
                }],
                &REFERENCE_SURFACE,
            )
        };
        let light = VehicleCategory::Light;
        let at_50 = level(50.0, 0.0, light) - level(50.0, 1.0, light);
        let at_30 = level(30.0, 0.0, light) - level(30.0, 1.0, light);
        let norway = level(50.0, 0.0, light) - level(50.0, 0.276, light);
        assert!((at_50 - 0.9).abs() < 0.1, "{at_50}");
        assert!((at_30 - 2.24).abs() < 0.05, "{at_30}");
        assert!((norway - 0.23).abs() < 0.03, "{norway}");
        let heavy = VehicleCategory::Heavy;
        assert_eq!(level(50.0, 1.0, heavy), level(50.0, 0.0, heavy));
    }

    /// dev4's flat correction of rolling noise, on every category alike.
    fn flat(db: f64) -> RoadSurface {
        RoadSurface {
            alpha: [[db; BANDS]; 3],
            ..REFERENCE_SURFACE
        }
    }

    fn a_weighted(flows: &[CategoryFlow], surface: &RoadSurface) -> f64 {
        level_db(a_weighted_energy(&line_emission_db(
            flows,
            (surface, REFERENCE_AIR_TEMPERATURE_C),
        )))
    }

    fn flow(
        vehicles_per_hour: f64,
        speed_kmh: f64,
        category: VehicleCategory,
    ) -> [CategoryFlow; 1] {
        [CategoryFlow {
            vehicles_per_hour,
            speed_kmh,
            category,
            slope_percent: 0.0,
            junction: None,
            electric_share: 0.0,
        }]
    }

    /// dev4 K1 and K2 (79.11 and 80.07 dB(A)/m) and the 20 km/h lock (66.17 dB(A)/m).
    #[test]
    fn reference_cases_keep_their_levels() {
        let k1 = a_weighted(
            &flow(10_000.0 * 0.70 / 12.0, 50.0, VehicleCategory::Light),
            &REFERENCE_SURFACE,
        );
        assert!((k1 - 79.11).abs() < 0.15, "{k1}");
        let k2 = a_weighted(
            &flow(500.0 * 0.70 / 12.0, 80.0, VehicleCategory::Heavy),
            &flat(4.0),
        );
        assert!((k2 - 80.07).abs() < 0.15, "{k2}");
        let slow = a_weighted(
            &flow(100.0, 20.0, VehicleCategory::Light),
            &REFERENCE_SURFACE,
        );
        assert!((slow - 66.17).abs() < 0.15, "{slow}");
    }

    /// 2.2.4 on the audit's cases: 100 heavy vehicles an hour at 80 km/h up 6 % read 3.20 dB(A)/m
    /// above level; a two-way road at 80 km/h with 12 % heavy vehicles, half up and half down,
    /// 1.1 dB at 6 % and 3.4 dB at 10 %; motorcycles and gentle slopes nothing.
    #[test]
    fn slopes_load_the_engines() {
        let heavy = |slope| {
            a_weighted(
                &[CategoryFlow {
                    vehicles_per_hour: 100.0,
                    speed_kmh: 80.0,
                    category: VehicleCategory::Heavy,
                    slope_percent: slope,
                    junction: None,
                    electric_share: 0.0,
                }],
                &REFERENCE_SURFACE,
            )
        };
        assert!(
            (heavy(6.0) - heavy(0.0) - 3.20).abs() < 0.05,
            "{}",
            heavy(6.0) - heavy(0.0)
        );
        let two_way = |slope: f64| {
            let flows: Vec<CategoryFlow> = [slope, -slope]
                .iter()
                .flat_map(|&s| {
                    [
                        (VehicleCategory::Light, 440.0),
                        (VehicleCategory::Heavy, 60.0),
                    ]
                    .map(|(category, vehicles_per_hour)| CategoryFlow {
                        vehicles_per_hour,
                        speed_kmh: 80.0,
                        category,
                        slope_percent: s,
                        junction: None,
                        electric_share: 0.0,
                    })
                })
                .collect();
            a_weighted(&flows, &REFERENCE_SURFACE)
        };
        let (six, ten) = (two_way(6.0) - two_way(0.0), two_way(10.0) - two_way(0.0));
        assert!(
            (six - 1.1).abs() < 0.3 && (ten - 3.4).abs() < 0.4,
            "{six} {ten}"
        );
        assert_eq!(
            gradient_correction_db(VehicleCategory::Motorcycle, 10.0, 50.0),
            0.0
        );
        assert_eq!(
            gradient_correction_db(VehicleCategory::Light, 2.0, 50.0),
            0.0
        );
        assert_eq!(
            gradient_correction_db(VehicleCategory::Light, -6.0, 50.0),
            0.0
        );
        assert_eq!(
            gradient_correction_db(VehicleCategory::Heavy, -4.0, 80.0),
            0.0
        );
        assert_eq!(
            gradient_correction_db(VehicleCategory::Heavy, 20.0, 80.0),
            gradient_correction_db(VehicleCategory::Heavy, 12.0, 80.0)
        );
    }

    /// 2.2.5 on the audit's cases: at 50 km/h with 4 % medium and 4 % heavy vehicles a stop line
    /// reads 3.6 dB(A)/m above free flow and the first 100 m 1.35 dB on average; cars alone lose
    /// 0.5 dB there; heavy vehicles 50 m from a roundabout take (-1.15, +3.35).
    #[test]
    fn junctions_stop_and_start_the_flow() {
        let mix = |junction| {
            let flows = [
                (VehicleCategory::Light, 920.0),
                (VehicleCategory::Medium, 40.0),
                (VehicleCategory::Heavy, 40.0),
            ]
            .map(|(category, vehicles_per_hour)| CategoryFlow {
                vehicles_per_hour,
                speed_kmh: 50.0,
                category,
                slope_percent: 0.0,
                junction,
                electric_share: 0.0,
            });
            a_weighted(&flows, &REFERENCE_SURFACE)
        };
        let free = mix(None);
        let stop_line = mix(Some((Junction::TrafficLights, 0.0))) - free;
        assert!((stop_line - 3.6).abs() < 0.2, "{stop_line}");
        let mean_energy = (0..100)
            .map(|x| 10f64.powf(mix(Some((Junction::TrafficLights, f64::from(x) + 0.5))) / 10.0))
            .sum::<f64>()
            / 100.0;
        let approach = 10.0 * mean_energy.log10() - free;
        assert!((approach - 1.35).abs() < 0.15, "{approach}");
        let cars = |junction| {
            a_weighted(
                &[CategoryFlow {
                    vehicles_per_hour: 1_000.0,
                    speed_kmh: 50.0,
                    category: VehicleCategory::Light,
                    slope_percent: 0.0,
                    junction,
                    electric_share: 0.0,
                }],
                &REFERENCE_SURFACE,
            )
        };
        let car_change = cars(Some((Junction::TrafficLights, 0.0))) - cars(None);
        assert!((car_change + 0.5).abs() < 0.2, "{car_change}");
        let (rolling, propulsion) =
            junction_correction_db(VehicleCategory::Heavy, Some((Junction::Roundabout, 50.0)));
        assert!((rolling + 1.15).abs() < 1e-12 && (propulsion - 3.35).abs() < 1e-12);
        assert_eq!(
            junction_correction_db(VehicleCategory::Light, Some((Junction::Roundabout, 150.0))),
            (0.0, 0.0)
        );
    }

    /// The laws hold above 130 km/h too: cars at 140 km/h emit 0.79 dB(A)/m more than at 130
    /// (the rolling law outgrows the thinner flow), where a clamp at 130 left 0.32 dB less.
    #[test]
    fn fast_cars_follow_the_laws_past_one_hundred_and_thirty() {
        let at = |speed| {
            a_weighted(
                &flow(1_000.0, speed, VehicleCategory::Light),
                &REFERENCE_SURFACE,
            )
        };
        let rise = at(140.0) - at(130.0);
        assert!((rise - 0.79).abs() < 0.05, "{rise}");
    }

    /// At a traffic light a motorcycle pulls away 2.9 dB over a car doing the same (ASJ's
    /// non-steady flow), the car with CNOSSOS-EU's stop-and-start terms; 50 m on, halfway back to
    /// the steady 3.8 dB.
    #[test]
    fn a_motorcycle_pulls_away_over_a_car_by_the_japanese_measurements() {
        for (distance, over) in [(0.0, 2.9), (50.0, 3.35)] {
            let at = Some((Junction::TrafficLights, distance));
            let mix = |category| {
                let [one] = flow(100.0, 30.0, category);
                a_weighted(
                    &[CategoryFlow {
                        junction: at,
                        ..one
                    }],
                    &REFERENCE_SURFACE,
                )
            };
            let (car, moto) = (
                mix(VehicleCategory::Light),
                mix(VehicleCategory::Motorcycle),
            );
            assert!(
                (moto - car - over).abs() < 0.05,
                "{distance}: {moto} vs {car}"
            );
        }
    }

    /// A motorcycle sounds 3.8 dB over a car at the same speed (ASJ RTN-Model 2018), at 30, 50
    /// and 90 km/h alike.
    #[test]
    fn a_motorcycle_sounds_over_a_car_by_the_japanese_measurements() {
        for speed in [30.0, 50.0, 90.0] {
            let car = a_weighted(
                &flow(100.0, speed, VehicleCategory::Light),
                &REFERENCE_SURFACE,
            );
            let moto = a_weighted(
                &flow(100.0, speed, VehicleCategory::Motorcycle),
                &REFERENCE_SURFACE,
            );
            assert!(
                (moto - car - MOTORCYCLE_OVER_LIGHT_DB).abs() < 0.05,
                "{speed}: {moto} vs {car}"
            );
        }
    }

    /// Table F-4 of 2021/1226 at 50 km/h: a car on block pavers in herring-bone (NL10) +2.1 dB, on
    /// pavers not in herring-bone (NL11, setts) +5.7, a lorry on them +7.4; motorcycles keep their
    /// level, and a silent flow is silent on any surface.
    #[test]
    fn surfaces_follow_table_f4() {
        use super::super::road_surface::{HARD_ELEMENTS, HARD_ELEMENTS_HERRINGBONE};
        let over = |category, surface| {
            a_weighted(&flow(100.0, 50.0, category), surface)
                - a_weighted(&flow(100.0, 50.0, category), &REFERENCE_SURFACE)
        };
        let light = VehicleCategory::Light;
        assert!((over(light, &HARD_ELEMENTS_HERRINGBONE) - 2.1).abs() < 0.05);
        assert!((over(light, &HARD_ELEMENTS) - 5.7).abs() < 0.05);
        assert!((over(VehicleCategory::Heavy, &HARD_ELEMENTS) - 7.4).abs() < 0.05);
        assert_eq!(over(VehicleCategory::Motorcycle, &HARD_ELEMENTS), 0.0);
        assert_eq!(
            line_emission_db(&flow(0.0, 50.0, light), (&HARD_ELEMENTS, 20.0)),
            [f64::NEG_INFINITY; BANDS]
        );
    }
}

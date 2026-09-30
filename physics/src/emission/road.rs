//! CNOSSOS-EU road traffic emission (Directive 2015/996 Annex II 2.2 with the coefficients of
//! Delegated Directive 2021/1226): rolling and propulsion noise per vehicle category and octave
//! band, summed into the sound power per metre of a traffic flow, with the road gradient's
//! propulsion term (2.2.4). There are no temperature, acceleration or studded-tyre terms.

use crate::bands::BANDS;

/// Reference speed of the rolling and propulsion laws (km/h).
pub const REFERENCE_SPEED_KMH: f64 = 70.0;
/// Heavy vehicles never emit above this speed (km/h).
pub const HEAVY_SPEED_CAP_KMH: f64 = 80.0;
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
    pub speed_kmh: f64,
    pub category: VehicleCategory,
    /// The road's slope in the flow's direction (%, positive uphill).
    pub slope_percent: f64,
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

/// Sound power per metre (dB, Z-weighted) of a mix of flows; `-inf` in every band when silent.
/// The surface correction applies to rolling noise only (motorcycles have none).
pub fn line_emission_db(flows: &[CategoryFlow], surface_correction_db: f64) -> [f64; BANDS] {
    let mut energy = [0.0f64; BANDS];
    for flow in flows.iter().filter(|flow| flow.vehicles_per_hour > 0.0) {
        let speed = match flow.category {
            VehicleCategory::Heavy => flow.speed_kmh.min(HEAVY_SPEED_CAP_KMH),
            _ => flow.speed_kmh,
        };
        let coefficients = flow.category.coefficients();
        let law_speed = speed.max(MINIMUM_LAW_SPEED_KMH);
        let (log_ratio, relative) = (
            (law_speed / REFERENCE_SPEED_KMH).log10(),
            (law_speed - REFERENCE_SPEED_KMH) / REFERENCE_SPEED_KMH,
        );
        // Vehicles per metre: Q / (1000 v) with Q per hour and v in km/h.
        let density = flow.vehicles_per_hour / (1000.0 * speed);
        let gradient = gradient_correction_db(flow.category, flow.slope_percent, law_speed);
        for band in 0..BANDS {
            let (a_p, b_p) = coefficients.propulsion;
            let mut vehicle = 10f64.powf((a_p[band] + b_p[band] * relative + gradient) / 10.0);
            if let Some((a_r, b_r)) = coefficients.rolling {
                vehicle +=
                    10f64.powf((a_r[band] + b_r[band] * log_ratio + surface_correction_db) / 10.0);
            }
            energy[band] += density * vehicle;
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
    use super::*;
    use crate::bands::{a_weighted_energy, level_db};

    fn a_weighted(flows: &[CategoryFlow], surface: f64) -> f64 {
        level_db(a_weighted_energy(&line_emission_db(flows, surface)))
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
        }]
    }

    /// dev4 K1 and K2 (79.11 and 80.07 dB(A)/m) and the 20 km/h lock (66.17 dB(A)/m).
    #[test]
    fn reference_cases_keep_their_levels() {
        let k1 = a_weighted(
            &flow(10_000.0 * 0.70 / 12.0, 50.0, VehicleCategory::Light),
            0.0,
        );
        assert!((k1 - 79.11).abs() < 0.15, "{k1}");
        let k2 = a_weighted(
            &flow(500.0 * 0.70 / 12.0, 80.0, VehicleCategory::Heavy),
            4.0,
        );
        assert!((k2 - 80.07).abs() < 0.15, "{k2}");
        let slow = a_weighted(&flow(100.0, 20.0, VehicleCategory::Light), 0.0);
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
                }],
                0.0,
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
                    })
                })
                .collect();
            a_weighted(&flows, 0.0)
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

    /// The laws hold above 130 km/h too: cars at 140 km/h emit 0.79 dB(A)/m more than at 130
    /// (the rolling law outgrows the thinner flow), where a clamp at 130 left 0.32 dB less.
    #[test]
    fn fast_cars_follow_the_laws_past_one_hundred_and_thirty() {
        let at = |speed| a_weighted(&flow(1_000.0, speed, VehicleCategory::Light), 0.0);
        let rise = at(140.0) - at(130.0);
        assert!((rise - 0.79).abs() < 0.05, "{rise}");
    }

    #[test]
    fn surface_touches_rolling_only_and_heavy_speed_is_capped() {
        let moto = flow(100.0, 50.0, VehicleCategory::Motorcycle);
        assert_eq!(line_emission_db(&moto, 0.0), line_emission_db(&moto, 4.0));
        let fast = line_emission_db(&flow(100.0, 120.0, VehicleCategory::Heavy), 0.0);
        let capped = line_emission_db(&flow(100.0, 80.0, VehicleCategory::Heavy), 0.0);
        assert_eq!(fast, capped);
        assert_eq!(
            line_emission_db(&flow(0.0, 50.0, VehicleCategory::Light), 0.0),
            [f64::NEG_INFINITY; BANDS]
        );
    }
}

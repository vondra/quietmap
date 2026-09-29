//! Railway, tram and level-crossing horn emission: one whole-train band spectrum per category with
//! a rolling term growing as 30 lg(v / v_ref) and a constant traction term (dev4's tables), the
//! trains of a period spread over the line as CNOSSOS-EU's density N / (T 1000 v).
//!
//! Calibration (a model change against dev4): dev4's per-train tables read 7-12 dB above
//! independent CNOSSOS-EU per-vehicle computations (Appendix G) of typical consists and above the
//! EBA Laerm-Monitoring 2023 pass-bys (19 stations, +5.3 dB with 3.9 dB fewer trains; dev4 audit
//! 2026-09-24, `cnossos_calc.out`). Each category is shifted to its typical consist: passenger
//! trains to the EBA 2023 mean (26 axles, 176 m: regional EMU +7.2, loco and 8 coaches +5.3 dB),
//! high-speed trains converging on the 16-car set (+0.7 dB at 300 km/h), freight in the EU to
//! composite-brake wagons (105 axles, 511 m: +11.0 dB), freight elsewhere to long cast-iron and
//! composite trains (+2.5 to +3.9 dB), trams to a 30 m tram (+8.7 dB at 25 km/h, +12.3 at 40).

use crate::bands::BANDS;

/// Rolling noise grows as B lg(v / v_ref) (dev4's constant).
const ROLLING_SPEED_COEFFICIENT: f64 = 30.0;
/// Speeds below this are evaluated at it (as dev4).
const MINIMUM_SPEED_KMH: f64 = 20.0;

/// The rail type code of the prepared rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RailType {
    Rail,
    Tram,
    LightRail,
    NarrowGauge,
    Funicular,
    /// Heritage lines: no model yet, silent.
    Preserved,
    /// A level-crossing horn approach: soundings in the passenger counts.
    Horn,
}

impl RailType {
    pub fn from_code(code: u8) -> Self {
        match code {
            1 => RailType::Tram,
            2 => RailType::LightRail,
            3 => RailType::NarrowGauge,
            4 => RailType::Funicular,
            5 => RailType::Preserved,
            6 => RailType::Horn,
            _ => RailType::Rail,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            RailType::Rail => "rail",
            RailType::Tram => "tram",
            RailType::LightRail => "light_rail",
            RailType::NarrowGauge => "narrow_gauge",
            RailType::Funicular => "funicular",
            RailType::Preserved => "preserved",
            RailType::Horn => "horn",
        }
    }

    /// Speed where the line carries no posted limit (dev4: trams 25 km/h between stops).
    pub fn default_speed_kmh(self) -> f64 {
        match self {
            RailType::Tram => 25.0,
            RailType::LightRail => 60.0,
            RailType::NarrowGauge => 40.0,
            RailType::Funicular => 20.0,
            RailType::Rail => 80.0,
            RailType::Preserved => 0.0,
            RailType::Horn => 60.0,
        }
    }
}

/// Where freight runs: the EU network (EU27, CH, NO, GB) or elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreightRegion {
    Europe,
    World,
}

struct Train {
    rolling_db: [f64; BANDS],
    traction_db: [f64; BANDS],
    reference_speed_kmh: f64,
    maximum_speed_kmh: f64,
}

static FREIGHT: Train = Train {
    rolling_db: [110.0, 118.0, 126.0, 130.0, 131.0, 128.0, 120.0, 110.0],
    traction_db: [115.0, 113.0, 110.0, 105.0, 100.0, 95.0, 90.0, 85.0],
    reference_speed_kmh: 80.0,
    // EBA 2023 train-weighted mean freight pass-by speed on main lines.
    maximum_speed_kmh: 88.9,
};
static PASSENGER: Train = Train {
    rolling_db: [105.0, 112.0, 118.0, 122.0, 125.0, 122.0, 115.0, 105.0],
    traction_db: [100.0, 98.0, 95.0, 92.0, 88.0, 84.0, 78.0, 70.0],
    reference_speed_kmh: 100.0,
    maximum_speed_kmh: 300.0,
};
static TRAM: Train = Train {
    rolling_db: [98.0, 105.0, 110.0, 114.0, 117.0, 114.0, 107.0, 97.0],
    traction_db: [105.0, 103.0, 100.0, 97.0, 93.0, 89.0, 83.0, 75.0],
    reference_speed_kmh: 50.0,
    maximum_speed_kmh: 70.0,
};
static LIGHT_RAIL: Train = Train {
    rolling_db: [100.0, 107.0, 112.0, 116.0, 119.0, 116.0, 109.0, 99.0],
    traction_db: [108.0, 106.0, 103.0, 100.0, 96.0, 92.0, 86.0, 78.0],
    reference_speed_kmh: 80.0,
    maximum_speed_kmh: 120.0,
};

/// Horn spectrum (A-weighted sum 0 dB; Volpe 1993 horns, energy mean) and sound power calibrated
/// by dev4 to the FRA reference SEL of 107 dBA at 100 ft on a 402 m approach at 40 mph.
const HORN_SPECTRUM_DB: [f64; BANDS] = [-18.1, -28.5, -10.1, -2.6, -3.2, -8.4, -13.7, -21.8];
const HORN_SOUND_POWER_DBA: f64 = 140.72;

/// The train categories of the tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Category {
    Passenger,
    Freight,
    Tram,
    LightRail,
}

impl Category {
    fn train(self) -> &'static Train {
        match self {
            Category::Passenger => &PASSENGER,
            Category::Freight => &FREIGHT,
            Category::Tram => &TRAM,
            Category::LightRail => &LIGHT_RAIL,
        }
    }

    /// The calibration at `speed_kmh` (see the module doc).
    fn calibration_db(self, region: FreightRegion, speed_kmh: f64) -> f64 {
        match self {
            Category::Freight => match region {
                FreightRegion::Europe => -11.0,
                FreightRegion::World => -3.0,
            },
            Category::Tram => -8.7 - 0.24 * (speed_kmh.clamp(25.0, 40.0) - 25.0),
            // No independent consist computation: the regional EMU's excess.
            Category::LightRail => -7.0,
            // The EBA mean train up to 200 km/h, converging on the high-speed set at 300.
            Category::Passenger => -6.5 + 5.8 * ((speed_kmh - 200.0) / 100.0).clamp(0.0, 1.0),
        }
    }
}

fn train_bands(category: &Train, speed_kmh: f64, calibration_db: f64) -> [f64; BANDS] {
    let rolling_term =
        ROLLING_SPEED_COEFFICIENT * (speed_kmh / category.reference_speed_kmh).log10();
    std::array::from_fn(|band| {
        let rolling = 10f64.powf((category.rolling_db[band] + rolling_term) / 10.0);
        let traction = 10f64.powf(category.traction_db[band] / 10.0);
        10.0 * (rolling + traction).log10() + calibration_db
    })
}

/// Sound power per metre (dB, Z-weighted) of one period's trains: `passenger` and `freight` are
/// the trains in the period of `period_hours`; `-inf` when silent.
pub fn line_emission_db(
    rail_type: RailType,
    line_speed_kmh: f64,
    passenger: f64,
    freight: f64,
    period_hours: f64,
    region: FreightRegion,
) -> [f64; BANDS] {
    let mut energy = [0.0f64; BANDS];
    match rail_type {
        RailType::Preserved => {}
        RailType::Horn => {
            if passenger > 0.0 {
                let speed = line_speed_kmh.clamp(MINIMUM_SPEED_KMH, 200.0);
                let density = passenger / (period_hours * 1000.0 * speed);
                for band in 0..BANDS {
                    energy[band] = density
                        * 10f64.powf((HORN_SOUND_POWER_DBA + HORN_SPECTRUM_DB[band]) / 10.0);
                }
            }
        }
        _ => {
            let passenger_category = match rail_type {
                RailType::Tram => Category::Tram,
                RailType::LightRail | RailType::NarrowGauge => Category::LightRail,
                _ => Category::Passenger,
            };
            for (category, count) in [
                (passenger_category, passenger),
                (Category::Freight, freight),
            ] {
                if count <= 0.0 {
                    continue;
                }
                let train = category.train();
                let speed = line_speed_kmh.clamp(MINIMUM_SPEED_KMH, train.maximum_speed_kmh);
                let bands = train_bands(train, speed, category.calibration_db(region, speed));
                let density = count / (period_hours * 1000.0 * speed);
                for band in 0..BANDS {
                    energy[band] += density * 10f64.powf(bands[band] / 10.0);
                }
            }
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
    use crate::bands::{PERIOD_HOURS, a_weighted_energy, level_db};

    fn a_weighted(bands: &[f64; BANDS]) -> f64 {
        level_db(a_weighted_energy(bands))
    }

    /// The same trains in the 8 h night read 10 lg(12/8) = 1.76 dB above the 12 h day, and each
    /// category takes its own speed limit (freight at most 88.9 km/h).
    #[test]
    fn density_and_speed_follow_the_period_and_category() {
        let day = line_emission_db(
            RailType::Rail,
            120.0,
            10.0,
            0.0,
            PERIOD_HOURS[0],
            FreightRegion::Europe,
        );
        let night = line_emission_db(
            RailType::Rail,
            120.0,
            10.0,
            0.0,
            PERIOD_HOURS[2],
            FreightRegion::Europe,
        );
        assert!((a_weighted(&night) - a_weighted(&day) - 1.761).abs() < 1e-3);
        let freight_fast = line_emission_db(
            RailType::Rail,
            160.0,
            0.0,
            10.0,
            12.0,
            FreightRegion::Europe,
        );
        let freight_capped =
            line_emission_db(RailType::Rail, 88.9, 0.0, 10.0, 12.0, FreightRegion::Europe);
        assert_eq!(freight_fast, freight_capped);
    }

    /// The calibration lowers EU freight by 11 dB against the world fleet's 3 dB.
    #[test]
    fn eu_freight_is_eight_decibels_below_world_freight() {
        let eu = line_emission_db(RailType::Rail, 80.0, 0.0, 10.0, 12.0, FreightRegion::Europe);
        let world = line_emission_db(RailType::Rail, 80.0, 0.0, 10.0, 12.0, FreightRegion::World);
        assert!((a_weighted(&world) - a_weighted(&eu) - 8.0).abs() < 1e-9);
    }

    #[test]
    fn heritage_and_empty_periods_are_silent() {
        let silent = [f64::NEG_INFINITY; BANDS];
        assert_eq!(
            line_emission_db(
                RailType::Preserved,
                60.0,
                10.0,
                5.0,
                12.0,
                FreightRegion::Europe
            ),
            silent
        );
        assert_eq!(
            line_emission_db(RailType::Rail, 60.0, 0.0, 0.0, 12.0, FreightRegion::Europe),
            silent
        );
    }
}

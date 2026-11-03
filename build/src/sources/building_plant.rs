//! Outdoor plant runs by climate; the year tables supply household stocks and cooled floor shares.
use crate::climate::DegreeDays;
use crate::source_tables::{Plant, SourceTables};
use physics::emission::settlement::PlantUse;

// EN 14825 heating degree days and measured compressor hours; operating sound powers and hours.
const HEATING_DEGREE_DAYS_PER_RUNNING: f64 = 5_370.0;
const HEATING_RUNNING_MAX: f64 = 0.9;
const HEAT_PUMP_LW_DBA: f64 = 60.0;
const COOLING_HOURS_MAX: f64 = 6.0;
const COOLING_DEGREE_DAYS: f64 = 1_300.0;
const COOLING_SHAPE: [f64; 3] = [1.1, 1.5, 0.6];
const SPLIT_LW_DBA: f64 = 62.0;
const CENTRAL_LW_DBA: f64 = 68.0;

pub fn cooled_service_share(tables: &SourceTables, country_iso: u16) -> f64 {
    tables
        .service_cooling
        .get(country_iso.to_le_bytes())
        .copied()
        .unwrap_or(0.847)
}

fn country_plant(tables: &SourceTables, country_iso: u16) -> Plant {
    tables
        .plants
        .get(country_iso.to_le_bytes())
        .copied()
        .unwrap_or_default()
}

/// The outdoor units of one dwelling in `country_iso` (ISO 3166 alpha-2, little-endian) under
/// `climate`'s degree days: heating, cooling by split or window units, cooling by central units.
pub fn dwelling_plant(
    tables: &SourceTables,
    country_iso: u16,
    climate: DegreeDays,
) -> [PlantUse; 3] {
    let plant = country_plant(tables, country_iso);
    let heating = (climate.heating / HEATING_DEGREE_DAYS_PER_RUNNING).min(HEATING_RUNNING_MAX);
    let hours = plant.cooling_hours.unwrap_or_else(|| {
        COOLING_HOURS_MAX * (1.0 - (-climate.cooling / COOLING_DEGREE_DAYS).exp())
    });
    let cooling = COOLING_SHAPE.map(|shape| (hours / 24.0 * shape).min(1.0));
    [
        PlantUse {
            units_per_dwelling: plant.heating_units,
            running: [heating; 3],
            lw_dba: HEAT_PUMP_LW_DBA,
        },
        PlantUse {
            units_per_dwelling: plant.cooling_units * (1.0 - plant.central_share),
            running: cooling,
            lw_dba: SPLIT_LW_DBA,
        },
        PlantUse {
            units_per_dwelling: plant.cooling_units * plant.central_share,
            running: cooling,
            lw_dba: CENTRAL_LW_DBA,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use physics::emission::settlement::plant_sound_power;

    fn iso(code: &[u8; 2]) -> u16 {
        u16::from_le_bytes(*code)
    }

    /// A Prague home (2,640 heating degree days, 40 cooling): 6 % of heat pumps at 0.49 of the
    /// hours, 16 % of air conditioners an hour a month; 12 dB under dev4's 57 dB(A) by day.
    /// Bangkok's (3,830 cooling): 29 % of units 5.7 hours a day, louder in the evening.
    #[test]
    fn homes_carry_their_countrys_units_at_their_climates_hours() {
        let tables = SourceTables::test_fixture();
        let prague = plant_sound_power(
            &dwelling_plant(
                tables,
                iso(b"CZ"),
                DegreeDays {
                    heating: 2_640.0,
                    cooling: 40.0,
                },
            ),
            1.0,
        )
        .unwrap();
        assert!((prague.day_dba - 45.0).abs() < 0.1, "{prague:?}");
        assert!(prague.night_offset_db.abs() < 0.5);
        let bangkok = plant_sound_power(
            &dwelling_plant(
                tables,
                iso(b"TH"),
                DegreeDays {
                    heating: 0.0,
                    cooling: 3_830.0,
                },
            ),
            1.0,
        )
        .unwrap();
        assert!((bangkok.day_dba - 50.8).abs() < 0.1, "{bangkok:?}");
        assert!(bangkok.evening_offset_db > 1.0 && bangkok.night_offset_db < -2.0);
        // Dubai runs its units 12.8 hours a day; a Houston home's central condenser rules.
        let dubai = dwelling_plant(
            tables,
            iso(b"AE"),
            DegreeDays {
                heating: 0.0,
                cooling: 3_430.0,
            },
        );
        assert!((dubai[1].running[0] - 12.8 / 24.0 * 1.1).abs() < 1e-9);
        let houston = dwelling_plant(
            tables,
            iso(b"US"),
            DegreeDays {
                heating: 700.0,
                cooling: 2_100.0,
            },
        );
        assert!(houston[2].units_per_dwelling > 0.6 && houston[2].lw_dba == 68.0);
    }
}

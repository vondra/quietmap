//! Buildings as sources (dev4 settlement v3, not a standard): a fixed plant term plus a term per
//! square metre of gross floor area, `L_W = 10 lg(10^(fixed/10) + GFA 10^(per_m2/10))`, per
//! building class. The same area law serves the open-air leisure areas. Homes instead carry the
//! outdoor units their country's households own, each running its climate's share of the hours:
//! [`plant_sound_power`].

use super::spectrum::SoundPower;
use crate::bands::BANDS;

/// Sheds, roofs, huts, greenhouses and ruins: uninhabited, they emit nothing.
pub const SILENT: u8 = 10;
/// Detached and terraced houses: the apartments' heat-pump floor with a gentler night cut.
pub const HOUSE: u8 = 11;
/// Supermarkets and food shops: rooftop refrigeration runs through the night (dev4 audit B2).
pub const FOOD_RETAIL: u8 = 12;
/// Restaurants, cafes and bars: the kitchen extract (their guests are the people outside them).
pub const HOSPITALITY: u8 = 13;

/// Classes whose noise scales with the footprint, not the floor area: single-volume halls
/// (warehouse 2, church 5, barn 8) and ground-floor activity (food retail, hospitality).
pub fn is_footprint_scaled(class: u8) -> bool {
    matches!(class, 2 | 5 | 8 | FOOD_RETAIL | HOSPITALITY)
}

/// A class's area law: plant term, term per square metre, shape and period offsets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BuildingProfile {
    pub lw_fixed_dba: f64,
    pub lw_per_m2_dba: f64,
    pub spectrum_db: [f64; BANDS],
    pub evening_offset_db: f64,
    pub night_offset_db: f64,
}

const fn profile(
    fixed: f64,
    per_m2: f64,
    spectrum: [f64; BANDS],
    evening: f64,
    night: f64,
) -> BuildingProfile {
    BuildingProfile {
        lw_fixed_dba: fixed,
        lw_per_m2_dba: per_m2,
        spectrum_db: spectrum,
        evening_offset_db: evening,
        night_offset_db: night,
    }
}

const RESIDENTIAL_PLANT: [f64; BANDS] = [-1.0, -1.0, 0.0, 1.0, 1.0, 0.0, -3.0, -6.0];
const BUILDING_SERVICES: [f64; BANDS] = [-1.0, 0.0, 1.0, 1.0, 0.0, -1.0, -3.0, -6.0];
const HALL_BREAKOUT: [f64; BANDS] = [0.0, 1.0, 1.0, 0.0, -1.0, -2.0, -4.0, -7.0];

/// The profile of a building class; `None` for [`SILENT`]. Classes 0-9: residential (one air
/// source heat pump L_W 54-62, Daikin EN14825), commercial (AHU and chillers, Guyer), warehouse
/// or factory (roof and facade breakout), school (yard voices, 71.7 dB just outside in breaks),
/// hospital (24/7 chillers and gensets), place of worship (a public building's services), hotel, garage (vent fans),
/// farm (livestock fans), public; food retail re-anchored to one refrigeration unit (RWDI),
/// hospitality to a kitchen extract (Guyer; its VDI 3770 voices per m2 left to the people outside
/// the venues, `people`). Unknown classes are residential.
pub fn building_profile(class: u8) -> Option<BuildingProfile> {
    Some(match class {
        0 => profile(57.0, 25.0, RESIDENTIAL_PLANT, -5.0, -10.0),
        1 => profile(
            70.0,
            30.0,
            [-1.0, 1.0, 1.0, 1.0, 0.0, -1.0, -3.0, -6.0],
            -5.0,
            -10.0,
        ),
        2 => profile(58.0, 45.0, HALL_BREAKOUT, -3.0, -8.0),
        3 => profile(
            66.0,
            28.0,
            [-2.0, 0.0, 1.0, 2.0, 1.0, 0.0, -2.0, -5.0],
            -10.0,
            -25.0,
        ),
        4 => profile(72.0, 26.0, BUILDING_SERVICES, -3.0, -5.0),
        // A place of worship's own plant (its bells are events of their own, `bells`): a public
        // building's services, dev4's steady 72 dB(A) of "bells, fleet average" dropped.
        5 => profile(62.0, 25.0, BUILDING_SERVICES, -8.0, -20.0),
        6 => profile(
            58.0,
            22.0,
            [-1.0, 0.0, 0.0, 1.0, 1.0, -1.0, -3.0, -6.0],
            -2.0,
            -10.0,
        ),
        7 => profile(
            41.0,
            18.0,
            [-3.0, -1.0, 0.0, 1.0, 0.0, -1.0, -3.0, -6.0],
            -5.0,
            -15.0,
        ),
        8 => profile(
            56.0,
            20.0,
            [-1.0, 0.0, 1.0, 0.0, -1.0, -2.0, -4.0, -7.0],
            -5.0,
            -15.0,
        ),
        9 => profile(62.0, 25.0, BUILDING_SERVICES, -8.0, -20.0),
        SILENT => return None,
        HOUSE => profile(57.0, 22.0, RESIDENTIAL_PLANT, -5.0, -8.0),
        FOOD_RETAIL => profile(
            55.0,
            48.0,
            [1.0, 2.0, 1.0, 0.0, -1.0, -2.0, -4.0, -7.0],
            -2.0,
            -2.0,
        ),
        HOSPITALITY => profile(
            68.0,
            f64::NEG_INFINITY,
            [-1.0, 0.0, 1.0, 1.0, 1.0, 0.0, -3.0, -6.0],
            0.0,
            -5.0,
        ),
        _ => profile(57.0, 21.0, RESIDENTIAL_PLANT, -5.0, -10.0),
    })
}

/// Homes: the residential classes, whose plant is their dwellings' outdoor units.
pub fn is_home(class: u8) -> bool {
    matches!(class, 0 | HOUSE) || class > HOSPITALITY
}

/// One use of a home's outdoor units: units per dwelling (households owning one times units per
/// household), the share of the hours they run by day, evening and night, and their sound power
/// while running (dB(A)).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlantUse {
    pub units_per_dwelling: f64,
    pub running: [f64; 3],
    pub lw_dba: f64,
}

/// The expected sound power of `dwellings` homes' outdoor units, the energy over the year of each
/// use's running hours (an air-source heat pump heating, an air conditioner cooling); `None` when
/// nothing runs.
pub fn plant_sound_power(uses: &[PlantUse], dwellings: f64) -> Option<SoundPower> {
    let energy = |period: usize| {
        dwellings
            * uses
                .iter()
                .map(|u| u.units_per_dwelling * u.running[period] * 10f64.powf(u.lw_dba / 10.0))
                .sum::<f64>()
    };
    let [day, evening, night] = [energy(0), energy(1), energy(2)];
    if day <= 0.0 || evening <= 0.0 || night <= 0.0 {
        return None;
    }
    Some(SoundPower {
        day_dba: 10.0 * day.log10(),
        spectrum_db: RESIDENTIAL_PLANT,
        evening_offset_db: 10.0 * (evening / day).log10(),
        night_offset_db: 10.0 * (night / day).log10(),
    })
}

/// The shared area law (dB(A)): a fixed floor plus `lw_per_m2_dba` over `area_m2`.
pub fn area_law_lw_dba(lw_fixed_dba: f64, lw_per_m2_dba: f64, area_m2: f64) -> f64 {
    10.0 * (10f64.powf(lw_fixed_dba / 10.0) + area_m2 * 10f64.powf(lw_per_m2_dba / 10.0)).log10()
}

/// A building of `class` over `footprint_m2` with `floors` (0 when unknown or a ground activity):
/// the gross floor area counts one floor for footprint-scaled classes. `None` when silent.
pub fn building_sound_power(class: u8, footprint_m2: f64, floors: u8) -> Option<SoundPower> {
    let profile = building_profile(class)?;
    let counted_floors = if is_footprint_scaled(class) {
        1
    } else {
        floors.max(1)
    };
    let gross_floor_area_m2 = footprint_m2 * f64::from(counted_floors);
    Some(SoundPower {
        day_dba: area_law_lw_dba(
            profile.lw_fixed_dba,
            profile.lw_per_m2_dba,
            gross_floor_area_m2,
        ),
        spectrum_db: profile.spectrum_db,
        evening_offset_db: profile.evening_offset_db,
        night_offset_db: profile.night_offset_db,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(class: u8, footprint_m2: f64, floors: u8) -> f64 {
        building_sound_power(class, footprint_m2, floors)
            .unwrap()
            .day_dba
    }

    /// A 200 m2 three-floor apartment block: 10 lg(10^5.7 + 600 10^2.5) = 58.39 dB(A).
    #[test]
    fn residential_block_is_its_heat_pump_floor_plus_gentle_size_scaling() {
        assert!(
            (day(0, 200.0, 3) - 58.39).abs() < 0.1,
            "{}",
            day(0, 200.0, 3)
        );
    }

    #[test]
    fn sheds_are_silent_and_halls_scale_by_footprint() {
        assert_eq!(building_sound_power(SILENT, 50_000.0, 10), None);
        assert_eq!(day(2, 1_000.0, 5), day(2, 1_000.0, 1));
        assert!(day(0, 1_000.0, 5) > day(0, 1_000.0, 1));
        assert_eq!(
            day(0, 1_000.0, 0),
            day(0, 1_000.0, 1),
            "unknown floors count one"
        );
    }

    /// Units times running share times the running level, summed per period over the uses and
    /// the dwellings: a heat pump running 0.5 of the hours at 60 dB(A) in 6 % of 10 homes is
    /// 10 lg(10 x 0.06 x 0.5 x 10^6) = 54.8 dB(A).
    #[test]
    fn homes_emit_their_units_running_hours() {
        let heating = PlantUse {
            units_per_dwelling: 0.06,
            running: [0.5; 3],
            lw_dba: 60.0,
        };
        let plant = plant_sound_power(&[heating], 10.0).unwrap();
        assert!((plant.day_dba - 54.77).abs() < 0.01, "{}", plant.day_dba);
        assert_eq!((plant.evening_offset_db, plant.night_offset_db), (0.0, 0.0));
        let cooling = PlantUse {
            units_per_dwelling: 1.0,
            running: [0.22, 0.3, 0.12],
            lw_dba: 62.0,
        };
        let plant = plant_sound_power(&[cooling], 1.0).unwrap();
        assert!((plant.night_offset_db - 10.0 * (0.12f64 / 0.22).log10()).abs() < 1e-9);
        assert_eq!(plant_sound_power(&[], 1.0), None);
        assert!(is_home(0) && is_home(HOUSE) && is_home(20) && !is_home(1) && !is_home(SILENT));
    }

    /// Food retail refrigerates all night and spans at least 12 dB from a kiosk to a hypermarket;
    /// 24/7 plant classes cut less at night than day-only ones.
    #[test]
    fn operating_patterns_and_size_follow_the_calibration() {
        let food = building_profile(FOOD_RETAIL).unwrap();
        assert_eq!(food.night_offset_db, -2.0);
        assert!(day(FOOD_RETAIL, 1_000.0, 1) > day(1, 1_000.0, 1) + 5.0);
        assert!(day(FOOD_RETAIL, 80.0, 1) < 70.0);
        assert!(day(FOOD_RETAIL, 8_000.0, 1) - day(FOOD_RETAIL, 80.0, 1) >= 12.0);
        assert!(day(1, 500.0, 2) > day(0, 500.0, 2));
        let night = |class: u8| building_profile(class).unwrap().night_offset_db;
        assert!(night(4) > night(3) && night(2) > night(9));
        assert_eq!(night(1), -10.0);
    }
}

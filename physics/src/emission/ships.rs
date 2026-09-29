//! Ship traffic of an AIS vessel-density cell: the mean number of ships present in the cell times
//! one A-weighted sound power per acoustic class, radiated around the clock.

use super::spectrum::SoundPower;
use crate::bands::BANDS;

/// Mean hours in a month: hours present per month over it is the mean number of ships present
/// (EMODnet vessel density method 5.1).
const HOURS_PER_MONTH: f64 = 365.25 * 24.0 / 12.0;

/// The acoustic classes, in the order of the `ships.arrow` hour columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShipClass {
    /// Cargo, tanker, passenger, high-speed craft, military and unknown ships.
    Large,
    /// Tugs, service, dredging, fishing and other work boats.
    Work,
    /// Sailing and pleasure craft.
    Leisure,
}

pub const SHIP_CLASSES: [ShipClass; 3] = [ShipClass::Large, ShipClass::Work, ShipClass::Leisure];

impl ShipClass {
    /// A-weighted sound power of one ship, sailing or at berth. Large: Fredianelli et al. 2020
    /// (Sustainability 12:1740) pass-bys of 82.6-89.0 dB(A)/m over 150-250 m hulls and Bernardini
    /// et al. 2022 (IJERPH 19:10996, Table 7) moored ships 95-115 dB(A); work boats 83.5 dB(A)/m
    /// over 30 m and leisure craft 77.4 dB(A)/m over 12 m (Bernardini 2022, Table 11).
    pub const fn sound_power_dba(self) -> f64 {
        match self {
            ShipClass::Large => 108.0,
            ShipClass::Work => 98.0,
            ShipClass::Leisure => 88.0,
        }
    }

    /// Height of the dominant source above the water: funnels and vents of large ships
    /// (Bernardini 2022, 3.2), the engine casing of a work boat, a yacht's cockpit.
    pub const fn source_height_m(self) -> f64 {
        match self {
            ShipClass::Large => 15.0,
            ShipClass::Work => 5.0,
            ShipClass::Leisure => 3.0,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            ShipClass::Large => "large_ships",
            ShipClass::Work => "work_boats",
            ShipClass::Leisure => "leisure_craft",
        }
    }
}

/// Low-frequency dominated, falling about 4 dB per octave above 250 Hz (Fredianelli 2020
/// Figs 4-8, Bernardini 2022 3.5).
const SHIP_SPECTRUM: [f64; BANDS] = [0.0, 0.0, -3.0, -6.0, -9.0, -13.0, -18.0, -25.0];

/// A water cell's sound power from its mean vessel-hours per month by class, and the class
/// carrying most of the energy; `None` when no ship is present.
pub fn ship_cell_sound_power(hours_per_month: [f32; 3]) -> Option<(SoundPower, ShipClass)> {
    let mut total = 0.0;
    let mut loudest = (ShipClass::Large, 0.0);
    for class in SHIP_CLASSES {
        let hours = f64::from(hours_per_month[class as usize]);
        if hours.is_nan() || hours <= 0.0 {
            continue;
        }
        let energy = hours / HOURS_PER_MONTH * 10f64.powf(class.sound_power_dba() / 10.0);
        total += energy;
        if energy > loudest.1 {
            loudest = (class, energy);
        }
    }
    let sound = SoundPower {
        day_dba: 10.0 * total.log10(),
        spectrum_db: SHIP_SPECTRUM,
        evening_offset_db: 0.0,
        night_offset_db: 0.0,
    };
    (total > 0.0).then_some((sound, loudest.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ships_present_radiate_their_class_power_and_classes_add_in_energy() {
        let level = |hours: [f32; 3]| ship_cell_sound_power(hours).unwrap();
        let (always, class) = level([HOURS_PER_MONTH as f32, 0.0, 0.0]);
        assert!((always.day_dba - 108.0).abs() < 1e-3);
        assert_eq!(class, ShipClass::Large);
        // 60 h a month of large ships: 0.0821 ships present, 108 - 10.86 dB(A).
        assert!((level([60.0, 0.0, 0.0]).0.day_dba - 97.14).abs() < 0.02);
        // Ten work-boat hours carry the energy of one large-ship hour.
        let (mixed, class) = level([1.0, 10.0, 0.0]);
        assert!((mixed.day_dba - level([2.0, 0.0, 0.0]).0.day_dba).abs() < 1e-9);
        assert_eq!(class, ShipClass::Large);
        assert_eq!(level([0.0, 0.0, 30.0]).1, ShipClass::Leisure);
        assert_eq!(ship_cell_sound_power([0.0, 0.0, 0.0]), None);
        assert_eq!(ship_cell_sound_power([-1.0, 0.0, f32::NAN]), None);
    }
}

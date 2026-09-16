//! Ship traffic emission: the mean number of ships present in a water cell times one
//! A-weighted sound power per acoustic class, radiated around the clock.

use crate::types::NUM_BANDS;

/// Mean hours in a month; `hours_per_month / HOURS_PER_MONTH` is the mean number of ships
/// present in the cell (EMODnet vessel density method §5.1).
pub const HOURS_PER_MONTH: f64 = 365.25 * 24.0 / 12.0;

/// Ship cells reach as far as the heatmap painter can profile a ray
/// (`relevant_source_gpu::source_frame::MAXIMUM_PROFILE_RAY_M` = 11 872 m); the popup uses
/// the same cap so both agree. The busiest port cells (≈128 dB(A)) still hold ≈30 dB Lden at
/// the cap — an accepted truncation.
pub const SHIP_MAX_RADIUS_M: f64 = 11_800.0;

/// Half diagonal of the 1 km statistical cell: how far a sub-cell can sit from the row's
/// centre, so a reader's owner-square query pads `SHIP_MAX_RADIUS_M` by this much.
pub const SHIP_CELL_HALF_DIAGONAL_M: f64 = 707.2;

/// One acoustic class of ships, in the order of the `ships.arrow` hour columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShipClass {
    /// Cargo, tanker, passenger, high-speed craft, military and unknown ships.
    Large = 0,
    /// Tugs, service, dredging, fishing and other work boats.
    Work = 1,
    /// Sailing and pleasure craft.
    Leisure = 2,
}

pub const SHIP_CLASSES: [ShipClass; 3] = [ShipClass::Large, ShipClass::Work, ShipClass::Leisure];

impl ShipClass {
    /// A-weighted sound power of one ship, sailing or at berth.
    ///
    /// Large: Fredianelli et al. 2020 (Sustainability 12:1740) pass-by line-source levels of
    /// 82.6–89.0 dB(A) per metre over 150–250 m hulls give 105–113 dB(A); Bernardini et al.
    /// 2022 (IJERPH 19:10996, Table 7) moored ro-pax 109–113, ro-ro 107.5–109, container
    /// 95–97, cruise ships 95–115 dB(A) by size. Work: Bernardini 2022 Table 11 "medium
    /// vessels" 83.5 dB(A)/m over a 30 m hull. Leisure: "small vessels" 77.4 dB(A)/m over 12 m.
    pub const fn sound_power_dba(self) -> f64 {
        match self {
            Self::Large => 108.0,
            Self::Work => 98.0,
            Self::Leisure => 88.0,
        }
    }

    /// Height of the dominant source above the water: funnel and ventilation outlets of
    /// large ships (Bernardini 2022 §3.2), engine casing of work boats, cockpit of yachts.
    pub const fn source_height_m(self) -> f32 {
        match self {
            Self::Large => 15.0,
            Self::Work => 5.0,
            Self::Leisure => 3.0,
        }
    }

    /// The class stored as a point source's `source_type` byte (unknown bytes read as large).
    pub const fn from_index(index: u8) -> Self {
        match index {
            1 => Self::Work,
            2 => Self::Leisure,
            _ => Self::Large,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Large => "large_ships",
            Self::Work => "work_boats",
            Self::Leisure => "leisure_craft",
        }
    }
}

/// Relative octave spectrum 63 Hz … 8 kHz of ship noise: low-frequency dominated, falling
/// about 4 dB per octave above 250 Hz (Fredianelli 2020 Figs 4–8, Bernardini 2022 §3.5).
pub const SHIP_SPECTRUM: [f64; NUM_BANDS] = [0.0, 0.0, -3.0, -6.0, -9.0, -13.0, -18.0, -25.0];

/// A-weighted sound power of one water cell from its mean vessel-hours per month by class,
/// plus the class carrying the most energy. `None` when the cell is silent.
pub fn ship_cell_lw(hours_per_month: [f32; 3]) -> Option<(f64, ShipClass)> {
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
    (total > 0.0).then(|| (10.0 * total.log10(), loudest.0))
}

/// Emission bands normalized so their A-weighted total equals `lw`.
pub fn ship_emission_bands(lw: f64) -> [f64; NUM_BANDS] {
    super::spectrum::normalized_emission_bands(lw, &SHIP_SPECTRUM)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::propagation::iso9613::a_weighted_total;

    #[test]
    fn one_ship_always_present_radiates_its_class_power_and_classes_add_in_energy() {
        let (lw, class) = ship_cell_lw([HOURS_PER_MONTH as f32, 0.0, 0.0]).unwrap();
        assert!((lw - 108.0).abs() < 1e-3);
        assert_eq!(class, ShipClass::Large);
        // 60 h/month of large ships: N = 0.0821 → 108 − 10.86 dB.
        let (lane, _) = ship_cell_lw([60.0, 0.0, 0.0]).unwrap();
        assert!((lane - 97.14).abs() < 0.02, "{lane}");
        // Ten work boats hours equal one large ship hour in energy.
        let (mixed, class) = ship_cell_lw([1.0, 10.0, 0.0]).unwrap();
        let (large_only, _) = ship_cell_lw([2.0, 0.0, 0.0]).unwrap();
        assert!((mixed - large_only).abs() < 1e-9);
        assert_eq!(class, ShipClass::Large);
        assert_eq!(ship_cell_lw([0.0, 0.0, 0.0]), None);
        assert_eq!(ship_cell_lw([-1.0, 0.0, f32::NAN]), None);
        let bands = ship_emission_bands(97.14);
        assert!((a_weighted_total(&bands) - 97.14).abs() < 1e-6);
    }
}

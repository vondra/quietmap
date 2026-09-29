//! Industrial sites by dev4's area law (base level at 1 ha, ISO 8297 style, profiles by NACE code,
//! OSM subtype or source type), solar farms per MW and substations per MVA (IEC 551). The base
//! levels were calibrated against Czech strategic noise maps (SHM 2022) while the shapes still
//! added their A-weighted sum, which the area law restores (dev4's "spectral debt", C1 2026-07).

use super::DAY_ONLY_OFFSET_DB;
use super::spectrum::{SoundPower, a_weighted_level_db};
use crate::bands::BANDS;

/// An area-law profile: A-weighted level of a 1 ha site, spectrum shape, period offsets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IndustrialProfile {
    pub base_lw_dba: f64,
    pub spectrum_db: [f64; BANDS],
    pub evening_offset_db: f64,
    pub night_offset_db: f64,
}

const fn profile(base: f64, spectrum: [f64; BANDS], evening: f64, night: f64) -> IndustrialProfile {
    IndustrialProfile {
        base_lw_dba: base,
        spectrum_db: spectrum,
        evening_offset_db: evening,
        night_offset_db: night,
    }
}

// dev4's spectrum shapes (relative dB, 63 Hz .. 8 kHz), named by their main users. Sources: EU
// 2000/14/EC equipment limits, 3M Noise Navigator, FHWA RCNM (dev4 docs/about emission tables).
const OPEN_SITE: [f64; BANDS] = [-5.0, -3.0, -1.0, 0.0, 0.0, -1.0, -3.0, -6.0];
const EXTRACTION: [f64; BANDS] = [-3.0, -1.0, 0.0, 1.0, 0.0, -2.0, -5.0, -8.0];
const PROCESS: [f64; BANDS] = [-4.0, -2.0, 0.0, 1.0, 0.0, -1.0, -3.0, -6.0];
const WATER_TREATMENT: [f64; BANDS] = [-6.0, -3.0, -1.0, 0.0, 0.0, -1.0, -4.0, -7.0];
const THERMAL_POWER: [f64; BANDS] = [-2.0, 0.0, 1.0, 1.0, 0.0, -1.0, -3.0, -6.0];
const METALLURGY: [f64; BANDS] = [-2.0, -1.0, 0.0, 1.0, 1.0, 0.0, -2.0, -5.0];
const MACHINING: [f64; BANDS] = [-3.0, -1.0, 0.0, 1.0, 1.0, 0.0, -2.0, -5.0];
const MACHINERY: [f64; BANDS] = [-3.0, -1.0, 0.0, 1.0, 0.0, -1.0, -3.0, -6.0];

/// OSM source types of `industrial.arrow` with their own models or none: a rail yard (PLAN-z13
/// DROP: its 96 dB(A)/ha was guessed), a wind turbine, a wind-farm outline and an inactive site
/// (their turbines or nothing emit), a solar farm, a substation, a transformer (its rating joins
/// its substation). Types above the transformer are unknown and silent.
pub const SOURCE_QUARRY: u8 = 1;
pub const SOURCE_RAIL_YARD: u8 = 5;
pub const SOURCE_WIND_TURBINE: u8 = 10;
pub const SOURCE_WIND_OUTLINE: u8 = 11;
pub const SOURCE_INACTIVE: u8 = 12;
pub const SOURCE_SOLAR_FARM: u8 = 13;
pub const SOURCE_SUBSTATION: u8 = 14;
pub const SOURCE_TRANSFORMER: u8 = 15;

/// The coarsest profile, by OSM source type: generic site, quarry, farmyard, works, wastewater.
pub fn source_type_profile(source_type: u8) -> IndustrialProfile {
    match source_type {
        0 => profile(93.0, OPEN_SITE, -3.0, -10.0),
        SOURCE_QUARRY => profile(99.0, EXTRACTION, -5.0, -20.0),
        2 => profile(70.0, PROCESS, -5.0, -20.0),
        3 => profile(94.0, PROCESS, -3.0, -8.0),
        4 => profile(89.0, WATER_TREATMENT, 0.0, 0.0),
        _ => profile(92.0, OPEN_SITE, -3.0, -10.0),
    }
}

/// Synthetic NACE of registry-confirmed solar plants: they take the solar model, never a profile.
pub const SOLAR_NACE: u16 = 3599;

/// The most specific profile, by NACE code (4 digits, then the division); `None` when unknown.
/// Coal (05) runs around the clock, quarries (07, 08) by day; 06 and 62 are weak anchors.
pub fn nace_profile(nace: u16) -> Option<IndustrialProfile> {
    match nace {
        3511 => return Some(profile(97.0, THERMAL_POWER, -1.0, -2.0)),
        3512 => return Some(profile(90.0, PROCESS, 0.0, 0.0)),
        _ => {}
    }
    Some(match nace / 100 {
        5 => profile(99.0, EXTRACTION, 0.0, 0.0),
        6 => profile(92.0, PROCESS, -1.0, -2.0),
        7 | 8 => profile(99.0, EXTRACTION, -8.0, -20.0),
        19 => profile(96.0, THERMAL_POWER, -1.0, -2.0),
        23 => profile(100.0, EXTRACTION, -2.0, -4.0),
        24 => profile(100.0, METALLURGY, -2.0, -4.0),
        10 | 11 => profile(90.0, PROCESS, -5.0, -12.0),
        13..=15 => profile(88.0, PROCESS, -5.0, -15.0),
        16 | 17 => profile(93.0, MACHINING, -5.0, -15.0),
        20 => profile(94.0, PROCESS, -2.0, -4.0),
        22 => profile(90.0, PROCESS, -5.0, -10.0),
        25 => profile(93.0, MACHINING, -5.0, -10.0),
        27 | 28 => profile(90.0, PROCESS, -5.0, -12.0),
        29 | 30 => profile(93.0, MACHINERY, -5.0, -12.0),
        35 if nace != SOLAR_NACE => profile(97.0, THERMAL_POWER, -1.0, -2.0),
        37 => profile(89.0, WATER_TREATMENT, 0.0, 0.0),
        38 => profile(95.0, MACHINERY, -3.0, -8.0),
        1..=3 => profile(70.0, PROCESS, -5.0, -20.0),
        46 | 47 => profile(84.0, OPEN_SITE, -8.0, -20.0),
        52 => profile(86.0, OPEN_SITE, -3.0, -8.0),
        62 => profile(60.0, OPEN_SITE, -5.0, -20.0),
        _ => return None,
    })
}

/// The profile of an OSM site subtype (`industrial=*`, `product=*`); `None` for unknown (0).
/// A warehouse is a NACE 52 site: one fact, no quieter twin.
pub fn subtype_profile(subtype: u8) -> Option<IndustrialProfile> {
    Some(match subtype {
        1 => return nace_profile(5210),
        2 => profile(95.0, MACHINERY, -3.0, -6.0),
        3 => profile(99.0, EXTRACTION, -5.0, -20.0),
        4 => profile(90.0, PROCESS, -1.0, -3.0),
        5 => profile(100.0, EXTRACTION, -1.0, -3.0),
        6 => profile(100.0, METALLURGY, -1.0, -3.0),
        7 => profile(88.0, PROCESS, -3.0, -10.0),
        8 => profile(90.0, MACHINERY, -5.0, -15.0),
        9 => profile(93.0, MACHINERY, -3.0, -6.0),
        10 => profile(70.0, PROCESS, -5.0, -15.0),
        11 => profile(60.0, OPEN_SITE, -5.0, -20.0),
        12 => profile(92.0, MACHINERY, -3.0, -6.0),
        _ => return None,
    })
}

/// The area law's largest effective area (dev4 Fix B, 44k world polygons 2026-07): p99 of all
/// industry is 71 ha, of heavy industry 299 ha. Heavy divisions (mining 05/07/08, refining and
/// chemicals 19/20, minerals 23, metals 24) and their subtypes (3-6) radiate over their whole
/// footprint up to 300 ha; everything else stops at 50 ha, as emission-free yard grows.
pub const INDUSTRIAL_AREA_CAP_M2: f64 = 500_000.0;
pub const INDUSTRIAL_AREA_CAP_HEAVY_M2: f64 = 3_000_000.0;
/// The area law's smallest effective area.
const INDUSTRIAL_AREA_FLOOR_M2: f64 = 100.0;

pub fn sector_area_cap_m2(nace: Option<u16>, subtype: u8) -> f64 {
    let heavy_division =
        nace.is_some_and(|code| matches!(code / 100, 5 | 7 | 8 | 19 | 20 | 23 | 24));
    if heavy_division || matches!(subtype, 3..=6) {
        INDUSTRIAL_AREA_CAP_HEAVY_M2
    } else {
        INDUSTRIAL_AREA_CAP_M2
    }
}

/// The area law: the 1 ha level plus the shape's A-weighted sum plus 10 lg(effective area / 1 ha).
pub fn area_law_sound_power(profile: &IndustrialProfile, area_m2: f64, cap_m2: f64) -> SoundPower {
    let effective_m2 = area_m2.clamp(INDUSTRIAL_AREA_FLOOR_M2, cap_m2);
    SoundPower {
        day_dba: profile.base_lw_dba
            + a_weighted_level_db(&profile.spectrum_db)
            + 10.0 * (effective_m2 / 10_000.0).log10(),
        spectrum_db: profile.spectrum_db,
        evening_offset_db: profile.evening_offset_db,
        night_offset_db: profile.night_offset_db,
    }
}

/// Median capacity density of the 15,234 OSM solar farms tagging `plant:output:electricity`.
pub const SOLAR_MW_PER_HA_UNTAGGED: f64 = 0.55;
/// Full-power-equivalent inverter hours over the 12 h day, Central-European annual mean (an
/// assumption, not a measurement).
const SOLAR_DAY_DUTY_DB: f64 = -5.0;
/// A central inverter radiates 88 dB(A) per MW: Sungrow SG4950HV-MV, 95 dB(A) at 4.95 MW
/// (Lancefield Solar Farm noise assessment, Urbis 2022).
const SOLAR_LW_PER_MW_DBA: f64 = 88.0;
/// Inverter spectrum kept from dev4's former solar profile (Lancefield publishes only the total).
const SOLAR_SPECTRUM: [f64; BANDS] = [-8.0, -5.0, -2.0, 0.0, 0.0, -1.0, -3.0, -6.0];

/// A solar farm of `capacity_mw` (else its area at the untagged density), silent after dark.
pub fn solar_farm_sound_power(capacity_mw: Option<f64>, area_m2: f64) -> SoundPower {
    let mw = capacity_mw
        .filter(|mw| *mw > 0.0)
        .unwrap_or(area_m2 / 10_000.0 * SOLAR_MW_PER_HA_UNTAGGED);
    SoundPower {
        day_dba: SOLAR_LW_PER_MW_DBA + 10.0 * mw.max(1e-6).log10() + SOLAR_DAY_DUTY_DB,
        spectrum_db: SOLAR_SPECTRUM,
        evening_offset_db: DAY_ONLY_OFFSET_DB,
        night_offset_db: DAY_ONLY_OFFSET_DB,
    }
}

/// Substation classes from the `substation` tag, voltage and autotransformer evidence; 0 unknown.
pub const SUBSTATION_MAIN: u8 = 1;
pub const SUBSTATION_AUTO: u8 = 2;
pub const SUBSTATION_DISTRIBUTION: u8 = 3;
pub const SUBSTATION_MINOR: u8 = 4;

/// Class medians of OSM ratings (MVA): main 25, auto 160, distribution 2 (258,524 transformer
/// ratings), minor 0.4 (109 rated minor_distribution stations, mode 400 kVA); unknown as
/// distribution, the common case.
pub fn substation_class_mva(class: u8) -> f64 {
    match class {
        SUBSTATION_MAIN => 25.0,
        SUBSTATION_AUTO => 160.0,
        SUBSTATION_MINOR => 0.4,
        _ => 2.0,
    }
}

/// Transformer hum: the 100 Hz hum in the 125 Hz band over the 50 Hz fundamental, harmonics and
/// fans trailing off (an estimate: no published octave table was found).
const SUBSTATION_SPECTRUM: [f64; BANDS] = [-2.0, 0.0, -4.0, -8.0, -12.0, -16.0, -20.0, -26.0];

/// A substation of `mva` around the clock: IEC 551:1987 L_WA = 74 + 14 lg(MVA), 64 dB(A) at or
/// below 0.2 MVA (probably high for modern units: IEC 60076-10 is 8 dB lower).
pub fn substation_sound_power(mva: f64) -> SoundPower {
    SoundPower {
        day_dba: if mva <= 0.2 {
            64.0
        } else {
            74.0 + 14.0 * mva.log10()
        },
        spectrum_db: SUBSTATION_SPECTRUM,
        evening_offset_db: 0.0,
        night_offset_db: 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warehouses_coal_and_registry_divisions_take_their_profiles() {
        assert_eq!(subtype_profile(1), nace_profile(5210));
        assert_eq!(subtype_profile(1).unwrap().base_lw_dba, 86.0);
        let coal = nace_profile(510).unwrap();
        assert_eq!(
            (
                coal.base_lw_dba,
                coal.evening_offset_db,
                coal.night_offset_db
            ),
            (99.0, 0.0, 0.0)
        );
        let quarry = nace_profile(810).unwrap();
        assert_eq!(
            (quarry.evening_offset_db, quarry.night_offset_db),
            (-8.0, -20.0)
        );
        assert_eq!(nace_profile(700), nace_profile(810));
        assert_eq!(nace_profile(600).unwrap().night_offset_db, -2.0);
        assert_eq!(nace_profile(1920).unwrap().base_lw_dba, 96.0);
        assert_eq!(
            nace_profile(SOLAR_NACE),
            None,
            "solar never takes the thermal fallback"
        );
        assert_eq!(nace_profile(6200).unwrap().base_lw_dba, 60.0);
        assert_eq!(nace_profile(9900), None);
        assert_eq!(subtype_profile(0), None);
    }

    /// The w7-sources pilots: Vienna airport 24 MW 96.8, RING 2.112 MW 86.2, DE 4 MW 89.0; IEC 551
    /// spot values 100 MVA 102.0 and 1 MVA 74.0.
    #[test]
    fn solar_and_substation_pilots() {
        let solar = |mw: Option<f64>, area_m2: f64| solar_farm_sound_power(mw, area_m2).day_dba;
        assert!((solar(Some(24.0), 467_100.0) - 96.8).abs() < 0.05);
        assert!((solar(Some(2.112), 44_316.0) - 86.2).abs() < 0.05);
        assert!((solar(Some(4.0), 34_293.0) - 89.0).abs() < 0.05);
        assert!((solar(None, 20_000.0) - (88.0 + 10.0 * 1.1f64.log10() - 5.0)).abs() < 1e-9);
        let substation = |mva: f64| substation_sound_power(mva).day_dba;
        assert_eq!((substation(100.0), substation(1.0)), (102.0, 74.0));
        assert_eq!((substation(0.2), substation(0.1)), (64.0, 64.0));
        assert!(substation(0.3) > 64.0);
        let medians = [1, 2, 3, 4, 0].map(substation_class_mva);
        assert_eq!(medians, [25.0, 160.0, 2.0, 0.4, 2.0]);
    }

    #[test]
    fn heavy_sectors_radiate_over_300_ha_and_the_rest_over_50() {
        assert_eq!(
            sector_area_cap_m2(Some(2410), 0),
            INDUSTRIAL_AREA_CAP_HEAVY_M2
        );
        assert_eq!(sector_area_cap_m2(None, 6), INDUSTRIAL_AREA_CAP_HEAVY_M2);
        assert_eq!(
            sector_area_cap_m2(Some(810), 0),
            INDUSTRIAL_AREA_CAP_HEAVY_M2
        );
        assert_eq!(sector_area_cap_m2(Some(5210), 1), INDUSTRIAL_AREA_CAP_M2);
        assert_eq!(sector_area_cap_m2(Some(3511), 0), INDUSTRIAL_AREA_CAP_M2);
        assert_eq!(sector_area_cap_m2(None, 0), INDUSTRIAL_AREA_CAP_M2);
        // A 300 ha steelworks recovers 10 lg(300 / 50) = 7.78 dB over the flat cap; at 1 ha the
        // level is the base plus the shape's A-weighted sum.
        let steel = nace_profile(2410).unwrap();
        let level = |area: f64, cap: f64| area_law_sound_power(&steel, area, cap).day_dba;
        let recovered =
            level(3e6, INDUSTRIAL_AREA_CAP_HEAVY_M2) - level(3e6, INDUSTRIAL_AREA_CAP_M2);
        assert!((recovered - 10.0 * 6f64.log10()).abs() < 1e-9);
        let one_hectare = level(10_000.0, INDUSTRIAL_AREA_CAP_M2);
        assert!(
            (one_hectare - steel.base_lw_dba - a_weighted_level_db(&steel.spectrum_db)).abs()
                < 1e-9
        );
    }
}

//! Industrial noise emission (ISO 8297 + NACE profiles).
//!
//! Lw = baseLw + 10×log₁₀(area_m² / 10000)
//! Pre-discretized: Lw_per_point = Lw_total - 10×log₁₀(N_points)

use crate::types::NUM_BANDS;

/// Industrial emission profile.
pub struct IndustrialProfile {
    pub base_lw: f64,               // reference Lw at 10000 m² [dB]
    pub spectrum: [f64; NUM_BANDS], // relative dB per band
    pub evening_offset: f64,
    pub night_offset: f64,
}

/// Get profile by site_type.
/// base_lw values were authored against Czech SHM 2022 + CNOSSOS-EU Lw''
/// methodology (reviewed by GPT-5.4 + Gemini 3.1 Pro against ISO 8297 and
/// real EIS data) — but at a time when `industrial_emission_bands` carried a
/// hidden +4.9..+6.4 dB(A) spectrum surplus (audit 2026-06 I-03). Since the
/// B4+B6 normalization, base_lw IS the radiated dB(A) total, i.e. effective
/// emission dropped by that surplus; re-calibration against SHM is backlog
/// (C8a / area-density model I-04).
pub fn industrial_profile(site_type: u8) -> IndustrialProfile {
    match site_type {
        0 => IndustrialProfile {
            // generic industrial
            base_lw: 93.0,
            spectrum: [-5.0, -3.0, -1.0, 0.0, 0.0, -1.0, -3.0, -6.0],
            evening_offset: -3.0,
            night_offset: -10.0,
        },
        1 => IndustrialProfile {
            // quarry — crushing, loading, blasting
            base_lw: 99.0,
            spectrum: [-3.0, -1.0, 0.0, 1.0, 0.0, -2.0, -5.0, -8.0],
            evening_offset: -5.0,
            night_offset: -20.0,
        },
        2 => IndustrialProfile {
            // farmyard — animal husbandry, machinery, seasonal
            base_lw: 70.0,
            spectrum: [-4.0, -2.0, 0.0, 1.0, 0.0, -1.0, -3.0, -6.0],
            evening_offset: -5.0,
            night_offset: -20.0,
        },
        3 => IndustrialProfile {
            // works/factory
            base_lw: 94.0,
            spectrum: [-4.0, -2.0, 0.0, 1.0, 0.0, -1.0, -3.0, -6.0],
            evening_offset: -3.0,
            night_offset: -8.0,
        },
        4 => IndustrialProfile {
            // wastewater plant
            base_lw: 89.0,
            spectrum: [-6.0, -3.0, -1.0, 0.0, 0.0, -1.0, -4.0, -7.0],
            evening_offset: 0.0,
            night_offset: 0.0, // 24/7
        },
        10 => IndustrialProfile {
            // wind turbine (handled by wind.rs)
            base_lw: 0.0,
            spectrum: [0.0; NUM_BANDS],
            evening_offset: 0.0,
            night_offset: 0.0,
        },
        _ => IndustrialProfile {
            // default
            base_lw: 92.0,
            spectrum: [-5.0, -3.0, -1.0, 0.0, 0.0, -1.0, -3.0, -6.0],
            evening_offset: -3.0,
            night_offset: -10.0,
        },
    }
}

/// Get profile by NACE 4-digit sector code.
/// WHY: OSM source_type only gives 5 coarse categories. NACE codes from IRZ/E-PRTR/GEM
/// enable sector-specific profiles (metallurgy ≠ warehouse ≠ solar farm).
/// 4-digit resolution distinguishes solar (3599, quiet) from thermal power (3511, loud).
/// Values from docs/about/index.md emission tables (authored pre-normalization —
/// see the honesty note on `industrial_profile`).
/// Sources: EU 2000/14/EC equipment limits, 3M Noise Navigator, FHWA RCNM.
pub fn nace_profile(nace_4digit: u16) -> Option<IndustrialProfile> {
    // Try 4-digit match first (more specific), then fall back to 2-digit.
    // NOTE: NACE 3512 ("renewable") mixes solar, wind, and hydro in upstream source data.
    // Wind turbines are safe (source_type=10 early-returns before NACE is checked).
    // But hydro would wrongly get the solar profile, so we use a synthetic code 3599
    // for confirmed solar plants only. Enrichment scripts must write 3599 (=360000/100) for solar.
    match nace_4digit {
        // Solar farms — inverters only, ~45-55 dB Lw, zero at night.
        // Synthetic NACE 3599 (not real NACE) to avoid 3512 which mixes renewables.
        3599 => {
            return Some(IndustrialProfile {
                base_lw: 55.0,
                spectrum: [-8.0, -5.0, -2.0, 0.0, 0.0, -1.0, -3.0, -6.0],
                evening_offset: -3.0,
                night_offset: -50.0, // effectively silent at night
            });
        }
        // Thermal/nuclear power — turbines, cooling towers, transformers
        3511 => {
            return Some(IndustrialProfile {
                base_lw: 97.0,
                spectrum: [-2.0, 0.0, 1.0, 1.0, 0.0, -1.0, -3.0, -6.0],
                evening_offset: -1.0,
                night_offset: -2.0, // near 24/7
            });
        }
        // Hydro power — turbines, spillways
        3512 => {
            return Some(IndustrialProfile {
                base_lw: 90.0,
                spectrum: [-4.0, -2.0, 0.0, 1.0, 0.0, -1.0, -3.0, -6.0],
                evening_offset: 0.0,
                night_offset: 0.0, // 24/7 baseload
            });
        }
        _ => {}
    }
    // Fall back to 2-digit match
    let nace_2 = nace_4digit / 100;
    Some(match nace_2 {
        // Heavy industry — high base Lw
        // Calibrated against Czech SHM 2022 + Irish Cement EIS (120.8 dBA plant total).
        // Coal mining (NACE 05: hard coal 0510 / lignite 0520) and other
        // mining & quarrying (NACE 08) share an acoustic class: draglines,
        // crushers, haul trucks, conveyors, ventilation fans. Same loud
        // open-pit/underground profile (GEM Global Coal Mine Tracker enrichment).
        5 | 8 => IndustrialProfile {
            base_lw: 99.0,
            spectrum: [-3.0, -1.0, 0.0, 1.0, 0.0, -2.0, -5.0, -8.0],
            evening_offset: -8.0,
            night_offset: -20.0,
        },
        23 => IndustrialProfile {
            // Cement, glass, minerals — grinding, crushing
            base_lw: 100.0,
            spectrum: [-3.0, -1.0, 0.0, 1.0, 0.0, -2.0, -5.0, -8.0],
            evening_offset: -2.0,
            night_offset: -4.0, // often 24/7
        },
        24 => IndustrialProfile {
            // Metallurgy — smelting, forging
            base_lw: 100.0,
            spectrum: [-2.0, -1.0, 0.0, 1.0, 1.0, 0.0, -2.0, -5.0],
            evening_offset: -2.0,
            night_offset: -4.0,
        },
        // Medium industry
        10 | 11 => IndustrialProfile {
            // Food/beverage processing
            base_lw: 90.0,
            spectrum: [-4.0, -2.0, 0.0, 1.0, 0.0, -1.0, -3.0, -6.0],
            evening_offset: -5.0,
            night_offset: -12.0,
        },
        13..=15 => IndustrialProfile {
            // Textiles, leather
            base_lw: 88.0,
            spectrum: [-4.0, -2.0, 0.0, 1.0, 0.0, -1.0, -3.0, -6.0],
            evening_offset: -5.0,
            night_offset: -15.0,
        },
        16 | 17 => IndustrialProfile {
            // Wood, paper — saws, presses
            base_lw: 93.0,
            spectrum: [-3.0, -1.0, 0.0, 1.0, 1.0, 0.0, -2.0, -5.0],
            evening_offset: -5.0,
            night_offset: -15.0,
        },
        20 => IndustrialProfile {
            // Chemical industry
            base_lw: 94.0,
            spectrum: [-4.0, -2.0, 0.0, 1.0, 0.0, -1.0, -3.0, -6.0],
            evening_offset: -2.0,
            night_offset: -4.0,
        },
        22 => IndustrialProfile {
            // Rubber, plastics
            base_lw: 90.0,
            spectrum: [-4.0, -2.0, 0.0, 1.0, 0.0, -1.0, -3.0, -6.0],
            evening_offset: -5.0,
            night_offset: -10.0,
        },
        25 => IndustrialProfile {
            // Metal fabrication — welding, cutting
            base_lw: 93.0,
            spectrum: [-3.0, -1.0, 0.0, 1.0, 1.0, 0.0, -2.0, -5.0],
            evening_offset: -5.0,
            night_offset: -10.0,
        },
        27 | 28 => IndustrialProfile {
            // Electrical/mechanical equipment
            base_lw: 90.0,
            spectrum: [-4.0, -2.0, 0.0, 1.0, 0.0, -1.0, -3.0, -6.0],
            evening_offset: -5.0,
            night_offset: -12.0,
        },
        29 | 30 => IndustrialProfile {
            // Motor vehicles, transport equipment
            base_lw: 93.0,
            spectrum: [-3.0, -1.0, 0.0, 1.0, 0.0, -1.0, -3.0, -6.0],
            evening_offset: -5.0,
            night_offset: -12.0,
        },
        // Energy/utilities — generic NACE 35 (not matched by 4-digit above)
        35 => IndustrialProfile {
            // Power generation — turbines, transformers (fallback)
            base_lw: 97.0,
            spectrum: [-2.0, 0.0, 1.0, 1.0, 0.0, -1.0, -3.0, -6.0],
            evening_offset: -1.0,
            night_offset: -2.0, // near 24/7
        },
        37 => IndustrialProfile {
            // Wastewater treatment
            base_lw: 89.0,
            spectrum: [-6.0, -3.0, -1.0, 0.0, 0.0, -1.0, -4.0, -7.0],
            evening_offset: 0.0,
            night_offset: 0.0, // 24/7
        },
        38 => IndustrialProfile {
            // Waste/recycling — loaders, compactors
            base_lw: 95.0,
            spectrum: [-3.0, -1.0, 0.0, 1.0, 0.0, -1.0, -3.0, -6.0],
            evening_offset: -3.0,
            night_offset: -8.0,
        },
        // Light industry / services
        1..=3 => IndustrialProfile {
            // Agriculture
            base_lw: 70.0,
            spectrum: [-4.0, -2.0, 0.0, 1.0, 0.0, -1.0, -3.0, -6.0],
            evening_offset: -5.0,
            night_offset: -20.0,
        },
        46 | 47 => IndustrialProfile {
            // Wholesale/retail trade — logistics
            base_lw: 84.0,
            spectrum: [-5.0, -3.0, -1.0, 0.0, 0.0, -1.0, -3.0, -6.0],
            evening_offset: -8.0,
            night_offset: -20.0,
        },
        52 => IndustrialProfile {
            // Warehousing/logistics
            base_lw: 86.0,
            spectrum: [-5.0, -3.0, -1.0, 0.0, 0.0, -1.0, -3.0, -6.0],
            evening_offset: -3.0,
            night_offset: -8.0,
        },
        _ => return None, // unknown NACE → fall back to site_type profile
    })
}

/// Get profile by site_subtype from OSM tags (industrial=*, product=*).
/// Fallback between nace_profile (most specific) and industrial_profile (coarsest).
/// Subtype values set by osm-extract site_subtype_from_tags().
pub fn subtype_profile(subtype: u8) -> Option<IndustrialProfile> {
    let spec = [-3.0, -1.0, 0.0, 1.0, 0.0, -1.0, -3.0, -6.0]; // generic industrial
    match subtype {
        0 => None, // unknown — fall through to source_type
        1 => Some(IndustrialProfile {
            // warehouse/logistics — quiet
            base_lw: 75.0,
            spectrum: [-5.0, -3.0, -1.0, 0.0, 0.0, -1.0, -3.0, -6.0],
            evening_offset: -5.0,
            night_offset: -15.0,
        }),
        2 => Some(IndustrialProfile {
            // factory/works — generic loud
            base_lw: 95.0,
            spectrum: spec,
            evening_offset: -3.0,
            night_offset: -6.0,
        }),
        3 => Some(IndustrialProfile {
            // mine/quarry — very loud
            base_lw: 99.0,
            spectrum: [-3.0, -1.0, 0.0, 1.0, 0.0, -2.0, -5.0, -8.0],
            evening_offset: -5.0,
            night_offset: -20.0, // daytime only
        }),
        4 => Some(IndustrialProfile {
            // chemical/refinery
            base_lw: 90.0,
            spectrum: [-4.0, -2.0, 0.0, 1.0, 0.0, -1.0, -3.0, -6.0],
            evening_offset: -1.0,
            night_offset: -3.0, // 24/7
        }),
        5 => Some(IndustrialProfile {
            // cement/mineral — very loud
            base_lw: 100.0,
            spectrum: [-3.0, -1.0, 0.0, 1.0, 0.0, -2.0, -5.0, -8.0],
            evening_offset: -1.0,
            night_offset: -3.0, // 24/7
        }),
        6 => Some(IndustrialProfile {
            // metal/steel/smelter — very loud
            base_lw: 100.0,
            spectrum: [-2.0, -1.0, 0.0, 1.0, 1.0, 0.0, -2.0, -5.0],
            evening_offset: -1.0,
            night_offset: -3.0, // 24/7
        }),
        7 => Some(IndustrialProfile {
            // food/brewery — moderate
            base_lw: 88.0,
            spectrum: [-4.0, -2.0, 0.0, 1.0, 0.0, -1.0, -3.0, -6.0],
            evening_offset: -3.0,
            night_offset: -10.0,
        }),
        8 => Some(IndustrialProfile {
            // wood/sawmill — moderate-loud
            base_lw: 90.0,
            spectrum: spec,
            evening_offset: -5.0,
            night_offset: -15.0,
        }),
        9 => Some(IndustrialProfile {
            // waste/recycling
            base_lw: 93.0,
            spectrum: spec,
            evening_offset: -3.0,
            night_offset: -6.0,
        }),
        10 => Some(IndustrialProfile {
            // farm/agriculture — quiet
            base_lw: 70.0,
            spectrum: [-4.0, -2.0, 0.0, 1.0, 0.0, -1.0, -3.0, -6.0],
            evening_offset: -5.0,
            night_offset: -15.0,
        }),
        11 => Some(IndustrialProfile {
            // office/commercial — very quiet
            base_lw: 60.0,
            spectrum: [-5.0, -3.0, -1.0, 0.0, 0.0, -1.0, -3.0, -6.0],
            evening_offset: -5.0,
            night_offset: -20.0,
        }),
        12 => Some(IndustrialProfile {
            // port/shipyard
            base_lw: 92.0,
            spectrum: [-3.0, -1.0, 0.0, 1.0, 0.0, -1.0, -3.0, -6.0],
            evening_offset: -3.0,
            night_offset: -6.0,
        }),
        _ => None,
    }
}

/// Effective-area cap for the area-law (Fix B, Wave 2). Below the flat 50 ha
/// cap a warehouse estate is mostly empty yard, so more area ≠ more emission —
/// but a steelworks / cement works / refinery / quarry genuinely radiates
/// across its whole footprint, and the flat cap clamped a 538 ha works to 50 ha
/// (−10 dB). Values are data-derived from the world industrial footprint
/// distribution (2026-07 sample, 44 k polygons): ALL-industrial p99 = 71 ha,
/// HEAVY p99 = 299 ha, LOW-FILL p99 = 35 ha. Low-fill / everything-else keep
/// 50 ha (their p99 is below it — the cap barely bites, and the artifact guard
/// stays); the clearly-distributed heavy divisions get 300 ha (heavy p99),
/// which still clamps the multi-thousand-ha OSM-zone artifacts (heavy max was
/// 8 350 ha — a whole region, not one plant). C2 residual + the multi-country
/// guards may tune this.
pub const INDUSTRIAL_AREA_CAP_M2: f64 = 500_000.0;
pub const INDUSTRIAL_AREA_CAP_HEAVY_M2: f64 = 3_000_000.0;

/// Heavy = emission genuinely fills the footprint: mining/coal (05|08),
/// coke/refining + chemicals (19|20), cement/minerals (23), metallurgy (24),
/// and their OSM subtypes (quarry 3, chemical/refinery 4, cement 5, steel 6).
/// Power (35) stays capped — its emission is concentrated (turbine hall,
/// cooling towers), and a double-counted Temelín NPP polygon already exists
/// (Codex CRITICAL 7). Logistics/office keep the default cap by omission.
pub fn sector_area_cap_m2(nace_4digit: Option<u16>, site_subtype: u8) -> f64 {
    let heavy_div = nace_4digit
        .map(|n| n / 100)
        .is_some_and(|d| matches!(d, 5 | 8 | 19 | 20 | 23 | 24));
    let heavy_subtype = matches!(site_subtype, 3..=6);
    if heavy_div || heavy_subtype {
        INDUSTRIAL_AREA_CAP_HEAVY_M2
    } else {
        INDUSTRIAL_AREA_CAP_M2
    }
}

/// Compute industrial Lw from profile, site area, and the resolved sector's
/// effective-area cap (`sector_area_cap_m2`).
///
/// **C1 spectral-debt migration (2026-07).** Every `base_lw` above was
/// calibrated against CZ SHM 2022 UNDER the pre-2026-06 normalization, which
/// emitted `base_lw + a_weighted_total(spectrum)` — a hidden +4.9..+6.4 dB(A)
/// surplus (audit I-03). The 2026-06 fix (`spectrum::normalized_emission_bands`)
/// correctly made `base_lw` the honest A-weighted total, which dropped effective
/// emission by exactly that surplus and left the whole layer ~5-6 dB low.
/// Adding the SAME deterministic scalar back here restores the SHM-era
/// calibration under the corrected spectrum — no fit, and the normalization
/// (which is correct) is NOT reverted. The per-sector residual against
/// multi-country anchors is C2 (Wave 2). Wind (source_type 10) never reaches
/// this fn — it returns early in `prepare_industrial_points`.
pub fn industrial_lw(profile: &IndustrialProfile, area_m2: f64, max_effective_area_m2: f64) -> f64 {
    let effective = area_m2.clamp(100.0, max_effective_area_m2);
    let spectral_debt = crate::propagation::iso9613::a_weighted_total(&profile.spectrum);
    profile.base_lw + spectral_debt + 10.0 * (effective / 10000.0).log10()
}

/// Compute emission bands, normalized so `a_weighted_total(bands) == lw`.
pub fn industrial_emission_bands(profile: &IndustrialProfile, lw: f64) -> [f64; NUM_BANDS] {
    super::spectrum::normalized_emission_bands(lw, &profile.spectrum)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heavy_sectors_get_the_raised_cap_low_fill_keep_50ha() {
        // steel by NACE 24, by subtype 6; quarry/chemical/cement divisions.
        assert_eq!(
            sector_area_cap_m2(Some(2410), 0),
            INDUSTRIAL_AREA_CAP_HEAVY_M2
        );
        assert_eq!(sector_area_cap_m2(None, 6), INDUSTRIAL_AREA_CAP_HEAVY_M2);
        assert_eq!(
            sector_area_cap_m2(Some(810), 0),
            INDUSTRIAL_AREA_CAP_HEAVY_M2
        );
        assert_eq!(
            sector_area_cap_m2(Some(2011), 0),
            INDUSTRIAL_AREA_CAP_HEAVY_M2
        );
        // low-fill + power + generic keep the default 50 ha cap.
        assert_eq!(sector_area_cap_m2(Some(5210), 1), INDUSTRIAL_AREA_CAP_M2); // warehouse
        assert_eq!(sector_area_cap_m2(Some(3511), 0), INDUSTRIAL_AREA_CAP_M2); // power (concentrated)
        assert_eq!(sector_area_cap_m2(None, 0), INDUSTRIAL_AREA_CAP_M2); // generic
    }

    #[test]
    fn raised_cap_recovers_a_big_steelworks_low_fill_unchanged() {
        let steel = nace_profile(2410).unwrap();
        let debt = crate::propagation::iso9613::a_weighted_total(&steel.spectrum);
        // A 300 ha steelworks under the heavy cap vs the old flat 50 ha cap:
        // exactly +10*log10(300/50) = +7.78 dB of area growth recovered.
        let at_50ha = industrial_lw(&steel, 3_000_000.0, INDUSTRIAL_AREA_CAP_M2);
        let at_300ha = industrial_lw(&steel, 3_000_000.0, INDUSTRIAL_AREA_CAP_HEAVY_M2);
        assert!((at_300ha - at_50ha - 10.0 * (300.0f64 / 50.0).log10()).abs() < 1e-9);
        // C1 present: at the 1 ha reference the Lw is base_lw + spectral debt.
        assert!(
            (industrial_lw(&steel, 10_000.0, INDUSTRIAL_AREA_CAP_M2) - (steel.base_lw + debt))
                .abs()
                < 1e-9
        );
        // Low-fill (warehouse) is byte-identical either way below 50 ha.
        let wh = nace_profile(5210).unwrap();
        assert_eq!(
            industrial_lw(&wh, 400_000.0, sector_area_cap_m2(Some(5210), 1)),
            industrial_lw(&wh, 400_000.0, INDUSTRIAL_AREA_CAP_M2),
        );
    }
}

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
        5 => IndustrialProfile {
            // rail yard (railway=yard): open-air mechanical
            // work (switchers, coupling, retarders) between the enclosed
            // factory (94) and the blasting quarry (99); quarry spectrum as
            // the closest modelled open-air mechanical analogue. Yards run
            // around the clock (Giusti 2000: "a rail yard operates 24 hours
            // per day, 7 days per week", CAA Vol.28 No.4). Provisional until
            // the Schall 03 yard chapter is verified (w4-horns open question).
            base_lw: 96.0,
            spectrum: [-3.0, -1.0, 0.0, 1.0, 0.0, -2.0, -5.0, -8.0],
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
/// WHY: OSM source_type only gives coarse categories. NACE codes from IRZ/E-PRTR/GEM
/// enable sector-specific profiles (metallurgy ≠ warehouse ≠ refinery).
/// Values from docs/about/index.md emission tables (authored pre-normalization —
/// see the honesty note on `industrial_profile`).
/// Sources: EU 2000/14/EC equipment limits, 3M Noise Navigator, FHWA RCNM.
/// NOTE: NACE 3512 ("renewable") mixes solar, wind, and hydro in upstream source data.
/// Wind turbines are safe (source_type=10 early-returns before NACE is checked).
/// Confirmed solar plants carry the synthetic code 3599 (not real NACE) so hydro
/// never takes the solar model — and 3599 never reaches this function: the prep
/// path intercepts it first (per-MW solar branch in `prepare_industrial_points`).
pub fn nace_profile(nace_4digit: u16) -> Option<IndustrialProfile> {
    // Try 4-digit match first (more specific), then fall back to 2-digit.
    match nace_4digit {
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
        // Coal and lignite mining (NACE 05: hard coal 0510 / lignite 0520)
        // runs around the clock — bucket-wheel excavators, conveyors, spreaders
        // cannot stop (Garzweiler: six excavators 24 h/day, 365 days/year,
        // Rheinische Industriekultur) — so no evening/night cut, unlike the
        // day-oriented quarries of NACE 08 below. Same loud open-pit/
        // underground spectrum (GEM Global Coal Mine Tracker enrichment).
        5 => IndustrialProfile {
            base_lw: 99.0,
            spectrum: [-3.0, -1.0, 0.0, 1.0, 0.0, -2.0, -5.0, -8.0],
            evening_offset: 0.0,
            night_offset: 0.0, // 24/7
        },
        // Oil and gas extraction — pumpjacks, compressors, drilling rigs.
        // Continuous operation like a process plant. Stamped by the VE/CO
        // oil and gas feeds (NACE 0600); a weak anchor — no published Lw per
        // pad was found, so this mirrors the process-plant duty at a moderate
        // level until W1 says otherwise.
        6 => IndustrialProfile {
            base_lw: 92.0,
            spectrum: [-4.0, -2.0, 0.0, 1.0, 0.0, -1.0, -3.0, -6.0],
            evening_offset: -1.0,
            night_offset: -2.0, // near 24/7
        },
        7 | 8 => IndustrialProfile {
            // Metal-ore mining (07) and other mining & quarrying (08) —
            // draglines, crushers, haul trucks, ventilation fans. Same loud
            // spectrum as coal, day-oriented hours.
            base_lw: 99.0,
            spectrum: [-3.0, -1.0, 0.0, 1.0, 0.0, -2.0, -5.0, -8.0],
            evening_offset: -8.0,
            night_offset: -20.0,
        },
        // Coke and refined petroleum — furnaces, compressors, cooling, flares.
        // A continuous process acoustically close to a thermal power plant
        // (same spectrum as 3511). Stamped by E-PRTR 1(a)/1(b)/1(d)/1(f) and
        // the VE oil-plants feed; the Pernis guard needs this arm to exist.
        19 => IndustrialProfile {
            base_lw: 96.0,
            spectrum: [-2.0, 0.0, 1.0, 1.0, 0.0, -1.0, -3.0, -6.0],
            evening_offset: -1.0,
            night_offset: -2.0, // near 24/7
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
        // Energy/utilities — generic NACE 35 (not matched by 4-digit above).
        // Synthetic 3599 (solar) is fenced off: it must never take the thermal
        // fallback if a future caller bypasses the prep solar branch — None
        // falls back to the site_type profile instead.
        35 if nace_4digit != 3599 => IndustrialProfile {
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
        // Computer programming (62) — a defensive arm only: no feed stamps it
        // since the India colour feed was deleted, but a registry point here
        // is an office address, so it gets the office level, never silence
        // and never a factory.
        62 => IndustrialProfile {
            base_lw: 60.0,
            spectrum: [-5.0, -3.0, -1.0, 0.0, 0.0, -1.0, -3.0, -6.0],
            evening_offset: -5.0,
            night_offset: -20.0,
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
        // warehouse/logistics — the NACE 52 profile, not its own quieter one:
        // a tagged warehouse IS a NACE 52 site, and the old subtype value sat
        // 13.2 dB Lden below it (reality-fixes w7-sources census). One fact.
        1 => nace_profile(5210),
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

/// Heavy = emission genuinely fills the footprint: mining/coal (05|07|08),
/// coke/refining + chemicals (19|20), cement/minerals (23), metallurgy (24),
/// and their OSM subtypes (quarry 3, chemical/refinery 4, cement 5, steel 6).
/// Power (35) stays capped — its emission is concentrated (turbine hall,
/// cooling towers), and a double-counted Temelín NPP polygon already exists
/// (Codex CRITICAL 7). Logistics/office keep the default cap by omission.
pub fn sector_area_cap_m2(nace_4digit: Option<u16>, site_subtype: u8) -> f64 {
    let heavy_div = nace_4digit
        .map(|n| n / 100)
        .is_some_and(|d| matches!(d, 5 | 7 | 8 | 19 | 20 | 23 | 24));
    let heavy_subtype = matches!(site_subtype, 3..=6);
    if heavy_div || heavy_subtype {
        INDUSTRIAL_AREA_CAP_HEAVY_M2
    } else {
        INDUSTRIAL_AREA_CAP_M2
    }
}

/// OSM power classes written by the extractor (`industrial.arrow`
/// `source_type`; `square-store::osm_contract`, classes owned by
/// `osm-extract::classify::industrial_class`). 13/14 carry their own physics
/// below (per-MW / per-MVA, not the area law); 11/12/15 are silent — the
/// turbines inside the fence emit, not the fence, and a transformer's rating
/// joins its substation instead of emitting twice.
pub const SOURCE_WIND_OUTLINE: u8 = 11;
pub const SOURCE_INACTIVE: u8 = 12;
pub const SOURCE_SOLAR_FARM: u8 = 13;
pub const SOURCE_SUBSTATION: u8 = 14;
pub const SOURCE_TRANSFORMER: u8 = 15;

/// Synthetic NACE for registry-confirmed solar plants (not real NACE: 3512
/// mixes solar, wind and hydro). The prep path treats it as a solar farm.
pub const SOLAR_NACE: u16 = 3599;

/// Median capacity density of the 15,234 OSM solar farms carrying
/// `plant:output:electricity` — the MW fallback for untagged farms.
pub const SOLAR_MW_PER_HA_UNTAGGED: f64 = 0.55;

/// Day duty of a solar farm [dB]: full-power-equivalent inverter hours
/// averaged over the 12 h day period. An ASSUMPTION (weakest in this file —
/// Central-European annual mean), not a measurement.
pub const SOLAR_DAY_DUTY_DB: f64 = -5.0;

/// Solar-farm day Lw [dB(A)]: 88 dB(A)/MW + 10·lg(MW) + day duty. The anchor is
/// measured: a Sungrow SG4950HV-MV central inverter at 4.95 MW radiates 95
/// dB(A) (Lancefield Solar Farm NIA, Urbis 2022 — 88 + 10·lg(4.95) = 94.95),
/// daylight-only operation, MV switchgear negligible beside it. MW comes from
/// the row's `plant:output:electricity` tag (parsed by the reader) or the
/// polygon area × [`SOLAR_MW_PER_HA_UNTAGGED`]. Evening/night: silent
/// (inverters sleep; the summer 19–21 h spillover costs Lden ≈ 0.1 dB,
/// unmodelled).
pub fn solar_farm_lw(capacity_mw: Option<f64>, area_m2: f64) -> f64 {
    let mw = capacity_mw
        .filter(|mw| *mw > 0.0)
        .unwrap_or(area_m2 / 10_000.0 * SOLAR_MW_PER_HA_UNTAGGED);
    88.0 + 10.0 * mw.max(1e-6).log10() + SOLAR_DAY_DUTY_DB
}

/// Solar inverter spectrum (unweighted, rel): kept from the legacy 3599 area
/// profile — Lancefield publishes only the total, so no per-band evidence.
pub const SOLAR_SPECTRUM: [f64; NUM_BANDS] = [-8.0, -5.0, -2.0, 0.0, 0.0, -1.0, -3.0, -6.0];

/// Substation fallback classes, derived by the readers from `voltage` /
/// autotransformer evidence (`square-store::osm_evidence::substation_power`).
pub const SUBSTATION_MAIN: u8 = 1;
pub const SUBSTATION_AUTO: u8 = 2;
pub const SUBSTATION_DISTRIBUTION: u8 = 3;

/// Substation class medians [MVA] over 258,524 OSM transformer ratings:
/// main/transmission 25, auto 160, distribution 2. Unknown (0) takes the
/// distribution median — the overwhelmingly common case.
pub fn substation_class_mva(substation_class: u8) -> f64 {
    match substation_class {
        SUBSTATION_MAIN => 25.0,
        SUBSTATION_AUTO => 160.0,
        _ => 2.0,
    }
}

/// Substation Lw [dB(A)], 24/7: IEC 551:1987 LWA = 74 + 14·lg(MVA), 64 dB
/// below 0.2 MVA (via the Arup Strutt empirical help page). Probably HIGH for
/// modern low-noise units — IEC 60076-10:2001 Standard Maximum is 66 +
/// 14·lg(MVA), 8 dB lower — but 551 is the specified anchor until W1 weighs
/// in; CIGRE Electra 310 (2020) is the newer reference to obtain.
pub fn substation_lw(mva: f64) -> f64 {
    if mva <= 0.2 { 64.0 } else { 74.0 + 14.0 * mva.log10() }
}

/// Transformer hum spectrum (unweighted, rel): an ESTIMATE — 50 Hz grid puts
/// the 100 Hz hum in the 125 Hz band with the 50 Hz fundamental in 63 Hz,
/// harmonics and cooling fans trailing off. No published octave table was
/// available (EN 50588-1 / CIGRE Electra 310 would supply one).
pub const SUBSTATION_SPECTRUM: [f64; NUM_BANDS] =
    [-2.0, 0.0, -4.0, -8.0, -12.0, -16.0, -20.0, -26.0];

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
    fn warehouse_subtype_is_the_nace52_profile() {
        // A tagged warehouse IS a NACE 52 site: one fact, no quieter twin.
        let sub = subtype_profile(1).unwrap();
        let nace = nace_profile(5210).unwrap();
        assert_eq!(sub.base_lw, nace.base_lw);
        assert_eq!(sub.spectrum, nace.spectrum);
        assert_eq!(sub.evening_offset, nace.evening_offset);
        assert_eq!(sub.night_offset, nace.night_offset);
        assert_eq!(sub.base_lw, 86.0);
    }

    #[test]
    fn coal_runs_day_and_night_quarries_do_not() {
        // NACE 05 (hard coal 0510 / lignite 0520): 24/7 open-pit/underground
        // operation; NACE 08 quarries keep their day-oriented hours.
        let coal = nace_profile(510).unwrap();
        assert_eq!(coal.evening_offset, 0.0);
        assert_eq!(coal.night_offset, 0.0);
        assert_eq!(coal.base_lw, 99.0);
        let quarry = nace_profile(810).unwrap();
        assert_eq!(quarry.evening_offset, -8.0);
        assert_eq!(quarry.night_offset, -20.0);
    }

    #[test]
    fn new_registry_divisions_have_profiles() {
        // Stamped by the E-PRTR sub-activity map and the national feeds —
        // each must resolve, never fall back to a generic factory.
        let oil = nace_profile(600).unwrap();
        assert_eq!((oil.base_lw, oil.night_offset), (92.0, -2.0));
        let metal_mine = nace_profile(700).unwrap();
        let quarry = nace_profile(810).unwrap();
        assert_eq!(metal_mine.base_lw, quarry.base_lw);
        assert_eq!(metal_mine.night_offset, quarry.night_offset);
        let refinery = nace_profile(1920).unwrap();
        assert_eq!((refinery.base_lw, refinery.night_offset), (96.0, -2.0));
        // Synthetic solar never takes the thermal fallback, even direct.
        assert!(nace_profile(3599).is_none());
        // Defensive office arm for the deleted India white category.
        assert_eq!(nace_profile(6200).unwrap().base_lw, 60.0);
    }

    #[test]
    fn solar_and_substation_pilots() {
        // w7-sources evidence pilots (Lw day / 24 h):
        // Vienna airport 24 MW → 96.8; RING 2.112 MW → 86.2; DE 4 MW → 89.0.
        assert!((solar_farm_lw(Some(24.0), 467_100.0) - 96.8).abs() < 0.05);
        assert!((solar_farm_lw(Some(2.112), 44_316.0) - 86.2).abs() < 0.05);
        assert!((solar_farm_lw(Some(4.0), 34_293.0) - 89.0).abs() < 0.05);
        // Untagged farm: area × 0.55 MW/ha.
        assert!(
            (solar_farm_lw(None, 20_000.0) - (88.0 + 10.0 * 1.1f64.log10() - 5.0)).abs()
                < 1e-9
        );
        // IEC 551 spot checks: 100 MVA → 102.0, 1 MVA → 74.0 exactly; the
        // evidence pilots (Řeporyje 107.9, Praha východ 96.6, kiosk 71.2) sit
        // on this curve at ~264/~41/~0.63 MVA of transformer ratings.
        assert_eq!(substation_lw(100.0), 102.0);
        assert_eq!(substation_lw(1.0), 74.0);
        assert_eq!(substation_lw(0.2), 64.0);
        assert_eq!(substation_lw(0.1), 64.0);
        assert!(substation_lw(0.3) > 64.0);
        assert_eq!(
            (
                substation_class_mva(1),
                substation_class_mva(2),
                substation_class_mva(3),
                substation_class_mva(0)
            ),
            (25.0, 160.0, 2.0, 2.0)
        );
    }

    #[test]
    fn rail_yard_profile_runs_around_the_clock() {
        let yard = industrial_profile(5);
        assert_eq!(yard.base_lw, 96.0);
        assert_eq!(yard.evening_offset, 0.0);
        assert_eq!(yard.night_offset, 0.0);
        assert_eq!(yard.spectrum, industrial_profile(1).spectrum);
    }

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

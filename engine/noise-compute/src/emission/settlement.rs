//! Settlement (building) noise emission — custom model (NOT standardized).
//!
//! Two-component Lw model:
//!   Lw = 10×log₁₀(10^(Lw_fixed/10) + GFA × 10^(Lw_per_m²/10))
//! where GFA = area_m² × floors
//!
//! Buildings pre-discretized at import (centroid or facade points).
//! This module computes emission for a single point source.
//!
//! Constants are radiated dB(A):
//! `a_weighted_total(building_emission_bands(p, lw)) == lw`. Per-class
//! provenance is inline below. Band normalization happens once in
//! `spectrum::normalized_emission_bands`; do not add compensating offsets here.
//!
//! Phase 2 (2026-06-12, re-extract) added four classes the coarse phase-1 u8
//! could not carry: [`SILENT`] (sheds/roofs/huts — the ~18 M phantom
//! residential emitters, now Lw 0), [`HOUSE`] (split from apartments with the
//! same heat-pump floor and a gentler night cut), [`FOOD_RETAIL`]
//! (24/7 refrigeration, night −2 not −20), and [`HOSPITALITY`] (kitchen extract
//! plus evening voices). Classes 0–9 stay byte-stable; the spill `building_type()`
//! enumerates the new u8s, the finalize POI join reclassifies `building=yes`.

use crate::types::NUM_BANDS;

/// Settlement `building_type` class ids written by `osm-extract::spill`.
/// 0–9 are the phase-1 set (residential-apartments default → 0); 10–13 are the
/// phase-2 additions. A class id never moves once shipped (Convention-B
/// per-file contract): the arrow stores the raw u8, so re-numbering would
/// silently re-profile every cached building.
pub const SILENT: u8 = 10;
/// Residential house (detached/terrace/bungalow) — heat-pump floor, split from
/// apartments (class 0) so the gentler night cut (−8, not −10) applies.
/// Daytime garden maintenance is not part of this source.
pub const HOUSE: u8 = 11;
/// Food retail (supermarket / convenience / food shop) — rooftop refrigeration
/// condensers run 24/7, so the night cut is only −2 dB (audit B2, the single
/// worst phase-1 finding).
pub const FOOD_RETAIL: u8 = 12;
/// Hospitality (restaurant / café / pub / bar / fast_food) — continuous kitchen
/// canopy-extract fan + evening patron voices.
pub const HOSPITALITY: u8 = 13;

/// FOOTPRINT-scaled classes: noise scales with the ground footprint, NOT
/// `GFA = footprint × floors`. Two reasons a class lands here:
///   • single-story tall HALL — warehouse/factory (2), church nave (5), farm
///     barn (8): one acoustic volume, and `height/3` would mint phantom floors.
///   • GROUND-FLOOR ACTIVITY — the retail box ([`FOOD_RETAIL`]) and hospitality
///     ([`HOSPITALITY`]): the POI join types the WHOLE building (a café in a
///     6-floor block becomes hospitality with floors=6), but the kitchen / patron
///     / refrigeration noise sits on the ground floor — it must NOT ×floors.
/// Genuine multi-story occupancy (residential, office, hotel, hospital, school)
/// keeps GFA — there each floor adds dwellings / plant / occupants.
pub fn is_shed_type(building_type: u8) -> bool {
    matches!(building_type, 2 | 5 | 8 | FOOD_RETAIL | HOSPITALITY)
}

/// Building emission profile.
pub struct BuildingProfile {
    pub lw_fixed: f64,              // point sources (loading dock, HVAC unit) [dB]
    pub lw_per_m2: f64,             // distributed (facade breakout, HVAC per area) [dB/m²]
    pub spectrum: [f64; NUM_BANDS], // relative dB per band
    pub evening_offset: f64,        // dB (typically -3 to -10)
    pub night_offset: f64,          // dB (typically -2 to -25)
}

/// Get emission profile by building type.
///
/// Spectra: "LF" plant/HVAC peaks 63–250 Hz (diffracts well → carries far;
/// real building-services plant is LF-dominant, not the old
/// 500 Hz–2 kHz voice band); voices peak mid; bells/whistles HF.
pub fn building_profile(building_type: u8) -> BuildingProfile {
    match building_type {
        0 => BuildingProfile {
            // residential (houses AND apartment blocks until phase 2 splits them)
            // Anchor: 1 ASHP outdoor unit Lw 54–62 (Daikin EN14825; EU 813/2013
            // cap 65) running day+night → whole-house fixed term ≈ 57; night cut
            // −10 not −15 because heat pumps run hardest on winter nights.
            // settlement v3 (gg2): per_m² 21→25 — a big AC/HP-heavy block scales
            // up GENTLY (20k m² → Lw ~68), NOT the full ~35 size-law (that gives
            // 78, unrealistic where district/gas heating has no outdoor plant).
            lw_fixed: 57.0,
            lw_per_m2: 25.0,
            spectrum: [-1.0, -1.0, 0.0, 1.0, 1.0, 0.0, -3.0, -6.0],
            evening_offset: -5.0,
            night_offset: -10.0,
        },
        1 => BuildingProfile {
            // commercial/retail/office (coarse class — includes food retail
            // until phase 2). Anchor: AHU Lw 85–90, chillers 89–97 SPL@0.9m
            // (Guyer) → generic-mix plant ≈ Lw 70 fixed + strong area term.
            // Night −10 (NOT −20: part of the class is 24/7 refrigeration —
            // the worst single audit finding, B2; true food-retail −2 lands
            // with the phase-2 subtype).
            lw_fixed: 70.0,
            lw_per_m2: 30.0,
            spectrum: [-1.0, 1.0, 1.0, 1.0, 0.0, -1.0, -3.0, -6.0],
            evening_offset: -5.0,
            night_offset: -10.0,
        },
        2 => BuildingProfile {
            // warehouse / factory / manufacture building — roof+facade breakout of
            // the indoor process. settlement v3 (gg2): per_m² 21→45 fixes the
            // ~30 dB undershoot of a STANDALONE factory (was a quiet flat 58) — a
            // 1000 m² hall now ~75, 200 m² workshop ~68, 5000 m² shed ~82.
            // FOOTPRINT-scaled (is_shed_type) so a tall hall isn't height/3 floors.
            // ONE conservative light-industrial profile for factory AND warehouse
            // (a heavy forge undershoots ~5 dB — accepted, Occam). 24/7 DCs → −8.
            lw_fixed: 58.0,
            lw_per_m2: 45.0,
            spectrum: [0.0, 1.0, 1.0, 0.0, -1.0, -2.0, -4.0, -7.0],
            evening_offset: -3.0,
            night_offset: -8.0,
        },
        3 => BuildingProfile {
            // school — yard shouting dominates (just-outside-yard 71.7 dB
            // during breaks, ~190 d/yr → 57 Leq annual at the fence).
            // Term-time day-weekday source; deep evening/night cuts stay.
            lw_fixed: 66.0,
            lw_per_m2: 28.0,
            spectrum: [-2.0, 0.0, 1.0, 2.0, 1.0, 0.0, -2.0, -5.0],
            evening_offset: -10.0,
            night_offset: -25.0,
        },
        4 => BuildingProfile {
            // hospital — 24/7 chillers/cooling towers (Lw to 103 for big
            // plants), gensets, A&E. Plan §A: campus plant ≈ Lw 72–80.
            lw_fixed: 72.0,
            lw_per_m2: 26.0,
            spectrum: [-1.0, 0.0, 1.0, 1.0, 0.0, -1.0, -3.0, -6.0],
            evening_offset: -3.0,
            night_offset: -5.0, // 24/7 operation
        },
        5 => BuildingProfile {
            // church/worship — bells: ~110–125 dB at tower, annualized −20.8
            // (12 min/day) → ≈ Lw 89 IF the tower rings; many OSM churches
            // don't → fleet-average 72 fixed, PROP-MEAS; confirm bell share and
            // Lw by measurement.
            lw_fixed: 72.0,
            lw_per_m2: 26.0,
            spectrum: [-3.0, -2.0, -1.0, 0.0, 1.0, 1.0, 0.0, -2.0],
            evening_offset: -5.0,
            night_offset: -20.0,
        },
        6 => BuildingProfile {
            // hotel — HVAC + kitchen extract (Lw 75–85 per unit, modest
            // whole-building mix → 58), 24/7 but quieter night.
            lw_fixed: 58.0,
            lw_per_m2: 22.0,
            spectrum: [-1.0, 0.0, 0.0, 1.0, 1.0, -1.0, -3.0, -6.0],
            evening_offset: -2.0,
            night_offset: -10.0,
        },
        7 => BuildingProfile {
            // garage/parking structure — vent fans; keep quiet (per-movement
            // parking noise belongs to the Parkplatzlärm lane, not here).
            lw_fixed: 41.0,
            lw_per_m2: 18.0,
            spectrum: [-3.0, -1.0, 0.0, 1.0, 0.0, -1.0, -3.0, -6.0],
            evening_offset: -5.0,
            night_offset: -15.0,
        },
        8 => BuildingProfile {
            // farm building — livestock vent fans 30–80 dB(A) source,
            // summer-heavy (annualized into the constant), machinery.
            lw_fixed: 56.0,
            lw_per_m2: 20.0,
            spectrum: [-1.0, 0.0, 1.0, 0.0, -1.0, -2.0, -4.0, -7.0],
            evening_offset: -5.0,
            night_offset: -15.0,
        },
        9 => BuildingProfile {
            // public/civic — office-grade HVAC, day-centric.
            lw_fixed: 62.0,
            lw_per_m2: 25.0,
            spectrum: [-1.0, 0.0, 1.0, 1.0, 0.0, -1.0, -3.0, -6.0],
            evening_offset: -8.0,
            night_offset: -20.0,
        },
        SILENT => BuildingProfile {
            // sheds/roofs/huts/greenhouses/ruins (~18 M objects, enum §C′):
            // uninhabited / unheated / infra → emit NOTHING. lw_fixed below the
            // prepare_building_points `lw < 10` gate so building_lw stays under
            // it for any GFA; never reaches the scatter. Bands kept finite so
            // building_emission_bands(profile, lw) can't NaN if ever called.
            lw_fixed: f64::MIN,
            lw_per_m2: f64::MIN,
            spectrum: [0.0; NUM_BANDS],
            evening_offset: 0.0,
            night_offset: 0.0,
        },
        HOUSE => BuildingProfile {
            // residential house (detached/terrace) — the SAME heat-pump fixed
            // floor as apartments (class 0; Lw 57, Daikin EN14825) but a smaller
            // GFA term (single-family, no shared AHU). Plan §C house offsets are
            // evening −5 / night −8 (heat pumps run hardest on winter nights;
            // "was −15"). The §A garden-maintenance overlay (mower LWA 96, day
            // only) is a SEPARATE event layer the plan defers — NOT baked into
            // the continuous house level here (keeping nominal = the heat-pump
            // floor, so the offsets stay plan-faithful).
            lw_fixed: 57.0,
            // per_m² 18→22 (settlement v3): gentle size scaling, smaller than
            // apartments (single-family, no shared AHU); a big house ≈ HP floor.
            lw_per_m2: 22.0,
            spectrum: [-1.0, -1.0, 0.0, 1.0, 1.0, 0.0, -3.0, -6.0],
            evening_offset: -5.0,
            night_offset: -8.0,
        },
        FOOD_RETAIL => BuildingProfile {
            // food retail — rooftop refrigeration condensers + car-park/trolley
            // activity, run 24/7 → night −2 (NOT −20: the single worst phase-1
            // finding, audit B2). settlement v3 (gg2): RE-ANCHORED to ONE
            // refrigeration unit (45–65 Lw, N ∝ sales area) — NOT the truck-reefer
            // Lw 102 conflation that made every shop a flat 88. fix 88→55,
            // per_m² 32→48 → SIZE scales: večerka 80 m² ~67, supermarket 1000 ~78,
            // hypermarket 5000 ~85. FOOTPRINT-scaled (is_shed_type). LF-dominant
            // (condenser fans). Sources: Parkplatzlärmstudie (activity) + RWDI.
            lw_fixed: 55.0,
            lw_per_m2: 48.0,
            spectrum: [1.0, 2.0, 1.0, 0.0, -1.0, -2.0, -4.0, -7.0],
            evening_offset: -2.0,
            night_offset: -2.0,
        },
        HOSPITALITY => BuildingProfile {
            // restaurant/café/pub — canopy kitchen-extract fan (Lw 68–75 Guyer)
            // + evening patron voices (VDI 3770 / LfU Biergarten, per seating
            // area). settlement v3 (gg2): fix 75→68, per_m² 26→50 → SIZE scales:
            // café 50 m² ~71, restaurant 250 ~75 (v2 anchor), large 1000 ~80.
            // evening +0 (patron peak), mid (voices) + LF (extract).
            lw_fixed: 68.0,
            lw_per_m2: 50.0,
            spectrum: [-1.0, 0.0, 1.0, 1.0, 1.0, 0.0, -3.0, -6.0],
            evening_offset: 0.0,
            night_offset: -5.0,
        },
        _ => BuildingProfile {
            // default (residential) — also the `building=yes` 79 % + the
            // silent tail (sheds/roofs/huts) until the phase-2 re-extract.
            lw_fixed: 57.0,
            lw_per_m2: 21.0,
            spectrum: [-1.0, -1.0, 0.0, 1.0, 1.0, 0.0, -3.0, -6.0],
            evening_offset: -5.0,
            night_offset: -10.0,
        },
    }
}

/// The shared AREA-LAW — the ONE formula behind every settlement AND leisure
/// source (one source of truth, [`crate::emission::leisure`] uses it too):
///   `Lw = 10·log10(10^(fix/10) + area·10^(per_m²/10))`
/// `fix` is the minimum plant floor (a tiny building/terrace still hums); the
/// `per_m²` term dominates once `area` grows → size scales at +10 dB / decade.
pub fn area_lw(lw_fixed: f64, lw_per_m2: f64, area_m2: f64) -> f64 {
    let e_fixed = 10f64.powf(lw_fixed / 10.0);
    let e_dist = area_m2 * 10f64.powf(lw_per_m2 / 10.0);
    10.0 * (e_fixed + e_dist).log10()
}

/// Building Lw — the shared [`area_lw`] over gross floor area (footprint × floors).
pub fn building_lw(profile: &BuildingProfile, area_m2: f64, floors: u8) -> f64 {
    area_lw(
        profile.lw_fixed,
        profile.lw_per_m2,
        area_m2 * floors.max(1) as f64,
    )
}

/// Compute emission bands for a building (day period), normalized so
/// `a_weighted_total(bands) == lw`.
pub fn building_emission_bands(profile: &BuildingProfile, lw: f64) -> [f64; NUM_BANDS] {
    super::spectrum::normalized_emission_bands(lw, &profile.spectrum)
}

/// Max distance at which building is audible (inverse of free-field propagation).
/// Lp(d) = Lw - 20·log₁₀(d) - 11 = 0 → d = 10^((Lw-11)/20).
pub fn building_max_dist(lw: f64) -> f64 {
    crate::propagation::geo::point_source_audibility_radius(lw, 2_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::propagation::iso9613::a_weighted_total;

    #[test]
    fn test_residential_lw() {
        let p = building_profile(0);
        let lw = building_lw(&p, 200.0, 3);
        // GFA = 600 m²: 10·log₁₀(10^5.7 + 600×10^2.5) ≈ 58.4 — settlement v3 gentle
        // size scaling (per_m² 25); the heat-pump fixed term 57 still anchors small.
        assert!((lw - 58.39).abs() < 0.1, "residential: {:.2}", lw);
    }

    /// v2 invariant: constants are honest radiated dB(A) — for EVERY emitting
    /// class the normalized bands must A-sum back to the input lw exactly (the
    /// C7 normalization contract, now without any AW_* compensation). SILENT is
    /// excluded — it is deliberately below the audibility gate (lw → −∞).
    #[test]
    fn radiated_dba_equals_lw_for_all_classes() {
        for t in 0..=HOSPITALITY {
            if t == SILENT {
                continue;
            }
            let p = building_profile(t);
            let lw = building_lw(&p, 500.0, 2);
            let aw = a_weighted_total(&building_emission_bands(&p, lw));
            assert!(
                (aw - lw).abs() < 1e-6,
                "class {t}: radiated {aw:.6} != lw {lw:.6}"
            );
        }
    }

    /// SILENT (sheds/roofs/huts) must produce NO emission: its Lw falls under
    /// the `prepare_building_points` `lw < 10` audibility gate for any GFA, so
    /// the scatter never sees it. Guards the ~18 M phantom residential emitters
    /// fix (enum §C′).
    #[test]
    fn silent_class_emits_nothing() {
        let p = building_profile(SILENT);
        // Even a huge footprint stays below the gate.
        let lw = building_lw(&p, 50_000.0, 10);
        assert!(lw < 10.0, "SILENT must be sub-audible, got lw={lw}");
    }

    /// Food retail night must NOT collapse (24/7 refrigeration): its night
    /// offset is the gentlest of any class (−2), unlike the day-only school
    /// (−25). The phase-1 worst-finding (audit B2) fix.
    #[test]
    fn food_retail_runs_at_night() {
        assert_eq!(building_profile(FOOD_RETAIL).night_offset, -2.0);
        assert!(building_profile(FOOD_RETAIL).night_offset > building_profile(3).night_offset);
        // Food retail is materially louder than generic commercial (class 1).
        let fr = building_lw(&building_profile(FOOD_RETAIL), 1000.0, 1);
        let co = building_lw(&building_profile(1), 1000.0, 1);
        assert!(fr > co + 5.0, "food-retail {fr:.1} vs commercial {co:.1}");
    }

    /// settlement v3: SIZE must matter — FOOD_RETAIL spans ≥12 dB across a 100×
    /// footprint (the old flat fix 88 gave 0.2 dB: every shop = a hypermarket).
    #[test]
    fn food_retail_scales_with_size() {
        let p = building_profile(FOOD_RETAIL);
        let small = building_lw(&p, 80.0, 1);
        let large = building_lw(&p, 8000.0, 1);
        assert!(
            small < 70.0,
            "small shop {small:.1} should be well below the old 88"
        );
        assert!(
            large - small >= 12.0,
            "size span {:.1} dB too flat",
            large - small
        );
    }

    #[test]
    fn test_commercial_louder() {
        let rp = building_profile(0); // residential
        let cp = building_profile(1); // commercial
        let r_lw = building_lw(&rp, 500.0, 2);
        let c_lw = building_lw(&cp, 500.0, 2);
        assert!(
            c_lw > r_lw,
            "commercial ({:.1}) should be louder than residential ({:.1})",
            c_lw,
            r_lw
        );
    }

    /// Night ordering encodes the v2 calibration's core finding: 24/7 plant
    /// classes (hospital −5, warehouse −8) cut far less than day-only classes
    /// (school −25); commercial is no longer "shut at night" (−10, was −20).
    #[test]
    fn night_offsets_follow_operating_patterns() {
        assert!(building_profile(4).night_offset > building_profile(3).night_offset);
        assert!(building_profile(2).night_offset > building_profile(9).night_offset);
        assert_eq!(building_profile(1).night_offset, -10.0);
    }

    #[test]
    fn test_max_dist() {
        // Very quiet: d < R11 pixel pitch → invisible on heatmap but not rejected.
        let d_quiet = building_max_dist(20.0);
        assert!(d_quiet > 0.0 && d_quiet < 5.0, "quiet d={:.2}", d_quiet);
        let d = building_max_dist(50.0);
        assert!(d > 50.0 && d < 500.0, "d={:.0}", d);
        assert_eq!(building_max_dist(90.0), 2000.0); // capped
    }

    /// On-demand whole-model overview (not a gate): prints every building +
    /// leisure class's resulting Lw at a small + large footprint, so the model
    /// reads at a glance. `cargo test emission_review_table -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn emission_review_table() {
        use crate::emission::leisure;
        let bnames: [(u8, &str); 14] = [
            (0, "residential/apt"),
            (1, "commercial/office"),
            (2, "warehouse/factory"),
            (3, "school"),
            (4, "hospital"),
            (5, "church"),
            (6, "hotel"),
            (7, "garage"),
            (8, "farm"),
            (9, "public/civic"),
            (SILENT, "SILENT shed"),
            (HOUSE, "house"),
            (FOOD_RETAIL, "food retail"),
            (HOSPITALITY, "hospitality"),
        ];
        println!("\nBUILDING  Lw = 10log10(10^(fix/10) + GFA*10^(perm2/10))   GFA = area*floors (footprint if shed)");
        println!(
            "{:<20}{:>5}{:>7}{:>10}{:>11}{:>11}",
            "class", "fix", "perm2", "scale", "Lw@200m2", "Lw@3000m2"
        );
        for (t, name) in bnames {
            if t == SILENT {
                println!("{name:<20}   (emits nothing)");
                continue;
            }
            let p = building_profile(t);
            let shed = is_shed_type(t);
            let fl = if shed { 1 } else { 3 };
            let scale = if shed { "footprint" } else { "GFA(x3fl)" };
            let small = building_lw(&p, 200.0, fl);
            let large = building_lw(&p, 3000.0, fl);
            println!(
                "{:<20}{:>5.0}{:>7.0}{:>10}{:>11.1}{:>11.1}",
                name, p.lw_fixed, p.lw_per_m2, scale, small, large
            );
        }
        let lnames: [(u8, &str); 8] = [
            (leisure::PITCH, "pitch"),
            (leisure::PADEL, "padel"),
            (leisure::TENNIS, "tennis"),
            (leisure::BASKETBALL, "basketball"),
            (leisure::PLAYGROUND, "playground"),
            (leisure::POOL, "pool"),
            (leisure::OUTDOOR_SEATING, "outdoor seating"),
            (leisure::STADIUM, "stadium"),
        ];
        println!("\nLEISURE  SAME area-law (now UNIFIED): Lw = 10log10(10^(fix/10) + area*10^(perm2/10))");
        println!(
            "{:<20}{:>5}{:>7}{:>9}{:>11}",
            "class", "fix", "perm2", "ref_m2", "Lw@ref"
        );
        for (s, name) in lnames {
            let p = leisure::leisure_profile(s);
            let at_ref = leisure::leisure_lw(&p, p.ref_area_m2);
            println!(
                "{:<20}{:>5.0}{:>7.0}{:>9.0}{:>11.1}",
                name, p.lw_fixed, p.lw_per_m2, p.ref_area_m2, at_ref
            );
        }
        println!();
    }
}

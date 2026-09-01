//! Popup presentation helpers — display filtering and top-N selection.

use crate::types::Contributor;

/// Levels below this are not shown in the popup — neither as a contributor
/// row nor as the "Other sources" leftover, which would otherwise read as a
/// meaningless negative dB (Viitasaari: total 18.2 dB, leftover -2.5 dB).
const DISPLAY_FLOOR_DB: f64 = 0.0;

pub fn is_displayable(contributor: &Contributor) -> bool {
    contributor.periods.lden_db >= DISPLAY_FLOOR_DB
}

/// The "Other sources" bucket as the popup shows it: NEG_INFINITY (null on the
/// wire) below the display floor. Applied only at the wire boundary, because
/// the aircraft merge (`source-reader::aircraft_v6`) still energy-sums the raw
/// bucket with the re-finalized tail.
pub fn other_sources_for_display(other_lden_db: f64) -> f64 {
    if other_lden_db >= DISPLAY_FLOOR_DB {
        other_lden_db
    } else {
        f64::NEG_INFINITY
    }
}

pub fn display_count(contributors: &[Contributor]) -> usize {
    contributors.iter().filter(|c| is_displayable(c)).count()
}

/// Result of popup contributor finalization: the top-N displayable list,
/// plus the energy sum (dB) of everything else (dropped by threshold or
/// truncated past top_n). `other_lden_db` = NEG_INFINITY when there are
/// no leftovers to report.
pub struct FinalizedContributors {
    pub shown: Vec<Contributor>,
    pub other_lden_db: f64,
}

pub fn finalize_popup_contributors(
    mut contributors: Vec<Contributor>,
    top_n: usize,
) -> FinalizedContributors {
    contributors.sort_by(|a, b| {
        b.periods
            .lden_db
            .partial_cmp(&a.periods.lden_db)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut shown: Vec<Contributor> = Vec::with_capacity(top_n.min(contributors.len()));
    let mut other_energy = 0.0f64;
    for c in contributors {
        let keep = is_displayable(&c) && shown.len() < top_n;
        if keep {
            shown.push(c);
        } else {
            let lden = c.periods.lden_db;
            if lden.is_finite() {
                other_energy += 10f64.powf(lden / 10.0);
            }
        }
    }
    // log10(0) is already NEG_INFINITY, the "nothing to report" value.
    FinalizedContributors {
        shown,
        other_lden_db: 10.0 * other_energy.log10(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        Contributor, LayerKind, NoisePeriods, PropagationBaseline, ScreeningBreakdown,
        TerrainBreakdown, VegetationBreakdown,
    };

    fn contributor(source_type: LayerKind, lden_db: f64) -> Contributor {
        Contributor {
            source_type,
            osm_id: None,
            name: String::new(),
            subtype: String::new(),
            distance_m: 0.0,
            periods: NoisePeriods {
                ld_db: lden_db,
                le_db: lden_db,
                ln_db: lden_db,
                lden_db,
            },
            periods_free: NoisePeriods {
                ld_db: lden_db,
                le_db: lden_db,
                ln_db: lden_db,
                lden_db,
            },
            emission_db: 0.0,
            baseline: PropagationBaseline::default(),
            terrain: TerrainBreakdown::default(),
            screening: ScreeningBreakdown::default(),
            vegetation: VegetationBreakdown::default(),
            terrain_impact_db: 0.0,
            screening_impact_db: 0.0,
            vegetation_impact_db: 0.0,
            atmospheric_impact_db: 0.0,
            ground_impact_db: 0.0,
            received_bands: [0.0; crate::types::NUM_BANDS],
            geometry: None,
            metadata: None,
        }
    }

    #[test]
    fn sorts_by_lden_drops_negative_and_truncates() {
        let r = finalize_popup_contributors(
            vec![
                contributor(LayerKind::Road, 11.0),
                contributor(LayerKind::Road, 12.0),
                contributor(LayerKind::Railway, 10.0),
                contributor(LayerKind::Industrial, 9.0),
                contributor(LayerKind::Building, -1.0),
            ],
            2,
        );
        // Top 2 are Road 12 and Road 11.
        assert_eq!(r.shown.len(), 2);
        assert_eq!(r.shown[0].periods.lden_db, 12.0);
        assert_eq!(r.shown[1].periods.lden_db, 11.0);
        // Other: Railway 10 + Industrial 9 (dropped by top_n) + Building -1 (below threshold).
        let expect = 10.0
            * (10f64.powf(10.0 / 10.0) + 10f64.powf(9.0 / 10.0) + 10f64.powf(-1.0 / 10.0)).log10();
        assert!((r.other_lden_db - expect).abs() < 1e-9);
    }

    #[test]
    fn other_lden_neg_inf_when_all_shown() {
        let r = finalize_popup_contributors(
            vec![
                contributor(LayerKind::Road, 10.0),
                contributor(LayerKind::Railway, 5.0),
            ],
            30,
        );
        assert_eq!(r.shown.len(), 2);
        assert_eq!(r.other_lden_db, f64::NEG_INFINITY);
    }

    #[test]
    fn other_lden_sums_only_below_threshold() {
        let r = finalize_popup_contributors(
            vec![
                contributor(LayerKind::Road, -5.0),
                contributor(LayerKind::Road, -10.0),
                contributor(LayerKind::Railway, -8.0),
            ],
            30,
        );
        // All three below threshold → none shown, all aggregated: the raw bucket
        // keeps its -2.4 dB so a later aircraft merge can still add to it, and
        // only the display projection hides it.
        assert_eq!(r.shown.len(), 0);
        let expect = 10.0
            * (10f64.powf(-5.0 / 10.0) + 10f64.powf(-10.0 / 10.0) + 10f64.powf(-8.0 / 10.0))
                .log10();
        assert!((r.other_lden_db - expect).abs() < 1e-9);
        assert_eq!(
            other_sources_for_display(r.other_lden_db),
            f64::NEG_INFINITY
        );
        assert_eq!(other_sources_for_display(0.0), 0.0);
    }
}

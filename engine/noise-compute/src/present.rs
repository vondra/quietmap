//! Popup presentation helpers — display filtering, top-N selection, and the
//! indoor projection of an enclosed receiver's level breakdown.

use crate::envelope::indoor_level_db;
use crate::types::{Contributor, NoisePeriods, NoiseResult, SourceMetadata};

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

/// Publish an enclosed receiver's whole level breakdown as the indoor estimate.
///
/// Owner decision 2026-09-02: when the query point lies inside a building,
/// every level row the popup shows is the indoor value — the same quantity the
/// painted tile stores per layer — so the headline number and the rows under it
/// can no longer disagree by the 20-35 dB envelope step. The audit that found
/// the gap measured it on 5 of 294 sampled z13 pixels, every one of them an
/// enclosed pixel.
///
/// The projection covers every received Lden LEVEL the popup publishes: the
/// totals, the per-layer source rows, the contributor rows with the aircraft
/// period triples they display, and the "Other sources" leftover. It deliberately
/// stops there. Per-effect attenuations (`*_impact_db`) are differences and are
/// the same indoors; emission, the per-band spectra, the per-segment traces and
/// the aircraft peak-event (Lmax) statistics describe the outdoor
/// source-to-wall path or a different metric, and bending them by one broadband
/// constant would misname what they measure. The popup names that split in the
/// line it prints above the rows.
///
/// `envelope_delta_db` is `None` outdoors, and then this is a no-op — the one
/// place the two paths differ, so an outdoor popup provably keeps its facade
/// levels.
pub fn project_result_to_indoor_display(result: &mut NoiseResult, envelope_delta_db: Option<f64>) {
    let Some(delta_db) = envelope_delta_db else {
        return;
    };
    let to_indoor = |periods: &mut NoisePeriods| {
        periods.ld_db = indoor_level_db(periods.ld_db, delta_db);
        periods.le_db = indoor_level_db(periods.le_db, delta_db);
        periods.ln_db = indoor_level_db(periods.ln_db, delta_db);
        periods.lden_db = indoor_level_db(periods.lden_db, delta_db);
    };
    to_indoor(&mut result.total);
    to_indoor(&mut result.total_free);
    for source in &mut result.sources {
        to_indoor(&mut source.periods);
        to_indoor(&mut source.periods_free);
    }
    for contributor in &mut result.contributors {
        to_indoor(&mut contributor.periods);
        to_indoor(&mut contributor.periods_free);
        // An aircraft row publishes its own period triples, and the popup shows
        // the airborne one as that row's Lden breakdown; the ground-ops triples
        // are the same quantity for the same row, so the payload cannot leave
        // them 20-35 dB apart from `periods` above. Peak-event levels in the
        // same panel are a different metric and stay outdoors.
        if let Some(SourceMetadata::Aircraft(aircraft)) = contributor.metadata.as_mut() {
            if let Some(airborne) = aircraft.airborne.as_mut() {
                to_indoor(&mut airborne.periods);
            }
            if let Some(ground_ops) = aircraft.ground_ops.as_mut() {
                to_indoor(&mut ground_ops.periods);
                to_indoor(&mut ground_ops.periods_free);
                to_indoor(&mut ground_ops.runway_roll.periods);
                to_indoor(&mut ground_ops.taxi.periods);
                to_indoor(&mut ground_ops.apron_movement.periods);
            }
        }
    }
    result.other_sources_lden = indoor_level_db(result.other_sources_lden, delta_db);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        AircraftAirborneDetail, AircraftMetadata, Contributor, LayerKind, NoisePeriods,
        PropagationBaseline, ScreeningBreakdown, SourceResult, TerrainBreakdown,
        VegetationBreakdown,
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

    fn periods(lden_db: f64) -> NoisePeriods {
        NoisePeriods {
            ld_db: lden_db - 1.0,
            le_db: lden_db - 2.0,
            ln_db: lden_db - 3.0,
            lden_db,
        }
    }

    /// An aircraft contributor carrying the period triple the popup renders as
    /// that row's own Lden breakdown.
    fn aircraft_contributor(lden_db: f64) -> Contributor {
        let mut c = contributor(LayerKind::Aircraft, lden_db);
        c.metadata = Some(SourceMetadata::Aircraft(Box::new(AircraftMetadata {
            variant: "airborne".to_string(),
            airborne: Some(AircraftAirborneDetail {
                periods: periods(lden_db),
                // A peak-event level, not an Lden: it must survive untouched.
                lmax_peak: Some(lden_db + 30.0),
                ..AircraftAirborneDetail::default()
            }),
            ..AircraftMetadata::default()
        })));
        c
    }

    fn source(source_type: LayerKind, lden_db: f64) -> SourceResult {
        SourceResult {
            source_type,
            periods: periods(lden_db),
            periods_free: periods(lden_db + 4.0),
            segment_count: 1,
            displayed_count: 1,
        }
    }

    /// One bug class: an enclosed receiver published facade levels in its rows
    /// while the painted tile stored the indoor value, a 20-35 dB gap.
    #[test]
    fn indoor_projection_moves_every_level_row_and_leaves_nothing_at_the_facade() {
        // Facade numbers measured at tile 13/4415/2784 pixel (402, 256),
        // lat 49.823781 lng 14.053102 (unclassified 3 m footprint, delta 20 dB).
        let delta_db = 20.0;
        let mut result = NoiseResult::empty();
        result.total = periods(59.160_156_549_059_55);
        result.total_free = periods(61.0);
        result.sources = vec![
            source(LayerKind::Road, 56.076_499_469_356_06),
            source(LayerKind::Railway, 39.209_011_302_189_87),
            // Below the envelope step: floors at 0 dB, as the painter clamps.
            source(LayerKind::Industrial, 4.546_426_204_508_708),
            // A silent layer stays silent rather than becoming 0 dB.
            source(LayerKind::Building, f64::NEG_INFINITY),
        ];
        let mut road = contributor(LayerKind::Road, 55.8);
        road.emission_db = 75.0;
        road.screening_impact_db = -5.0;
        road.received_bands = [40.0; crate::types::NUM_BANDS];
        result.contributors = vec![road, aircraft_contributor(36.3)];
        result.other_sources_lden = 24.179;

        let facade = result.clone();
        project_result_to_indoor_display(&mut result, Some(delta_db));

        assert_eq!(result.total.lden_db, facade.total.lden_db - delta_db);
        assert_eq!(result.total.ld_db, facade.total.ld_db - delta_db);
        assert_eq!(result.total.le_db, facade.total.le_db - delta_db);
        assert_eq!(result.total.ln_db, facade.total.ln_db - delta_db);
        assert_eq!(
            result.total_free.lden_db,
            facade.total_free.lden_db - delta_db
        );
        assert_eq!(result.sources[0].periods.lden_db, 36.076_499_469_356_06);
        assert_eq!(
            result.sources[0].periods_free.lden_db,
            40.076_499_469_356_06
        );
        assert_eq!(result.sources[1].periods.lden_db, 19.209_011_302_189_87);
        assert_eq!(result.sources[2].periods.lden_db, 0.0);
        assert_eq!(result.sources[3].periods.lden_db, f64::NEG_INFINITY);
        assert_eq!(result.contributors[0].periods.lden_db, 35.8);
        assert_eq!(result.contributors[0].periods_free.lden_db, 35.8);
        // The aircraft row's own breakdown moves with the row it explains,
        // otherwise its "Final Lden" contradicts the badge above it.
        let Some(SourceMetadata::Aircraft(aircraft)) = &result.contributors[1].metadata else {
            panic!("aircraft metadata lost");
        };
        let airborne = aircraft.airborne.as_ref().expect("airborne detail");
        let Some(SourceMetadata::Aircraft(facade_aircraft)) = &facade.contributors[1].metadata
        else {
            panic!("aircraft metadata lost");
        };
        let facade_airborne = facade_aircraft.airborne.as_ref().expect("airborne detail");
        assert_eq!(airborne.periods.lden_db, 36.3 - delta_db);
        assert_eq!(airborne.periods.ln_db, 36.3 - 3.0 - delta_db);
        assert_eq!(result.other_sources_lden, 24.179 - delta_db);
        // Nothing audible is left quoting the facade — the failure the audit
        // caught was rows that never moved at all.
        for (before, after) in facade.sources.iter().zip(&result.sources) {
            if before.periods.lden_db.is_finite() {
                assert!(
                    after.periods.lden_db < before.periods.lden_db,
                    "{:?} row stayed at its facade level",
                    before.source_type
                );
            }
        }
        // The z13 etalon tile 4415/2784 stores road byte 72 and rail byte 38 at
        // this pixel — 36.0 dB and 19.0 dB after the HM3 `byte / 2.0` dequantise.
        // Both rows now land inside that 0.5 dB tile quantum.
        assert!((result.sources[0].periods.lden_db - 36.0).abs() < 0.5);
        assert!((result.sources[1].periods.lden_db - 19.0).abs() < 0.5);
        // And the other half of the boundary: what describes the outdoor path,
        // or another metric, is left exactly where it was.
        let road = &result.contributors[0];
        assert_eq!(road.emission_db, facade.contributors[0].emission_db);
        assert_eq!(
            road.screening_impact_db,
            facade.contributors[0].screening_impact_db
        );
        assert_eq!(road.received_bands, facade.contributors[0].received_bands);
        assert_eq!(airborne.lmax_peak, facade_airborne.lmax_peak);
        assert_eq!(result.segments.len(), facade.segments.len());
    }

    /// The outdoor half of the same bug class: with no envelope the popup
    /// must publish the facade levels untouched.
    #[test]
    fn outdoor_result_keeps_every_facade_level() {
        let mut result = NoiseResult::empty();
        result.total = periods(47.919);
        result.sources = vec![source(LayerKind::Road, 40.448)];
        result.contributors = vec![contributor(LayerKind::Railway, 45.056)];
        result.other_sources_lden = 21.227;
        let facade = result.clone();

        project_result_to_indoor_display(&mut result, None);

        assert_eq!(result.total.lden_db, facade.total.lden_db);
        assert_eq!(result.total.ln_db, facade.total.ln_db);
        assert_eq!(
            result.sources[0].periods.lden_db,
            facade.sources[0].periods.lden_db
        );
        assert_eq!(
            result.sources[0].periods_free.lden_db,
            facade.sources[0].periods_free.lden_db
        );
        assert_eq!(
            result.contributors[0].periods.lden_db,
            facade.contributors[0].periods.lden_db
        );
        assert_eq!(result.other_sources_lden, facade.other_sources_lden);
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

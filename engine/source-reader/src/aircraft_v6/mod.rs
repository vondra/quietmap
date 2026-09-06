//! Decode current aircraft popup artifacts and merge observed noise into the result.

mod airborne_view;
pub mod airport_summary_view;
mod airport_traffic_view;
mod columns;
mod cruise_view;

use arrow::record_batch::RecordBatch;
use noise_compute::compute::aircraft_v6::{
    airport_traffic as compute_airport_traffic, compute_aircraft_v6,
};
use noise_compute::types::{
    LayerKind, NoisePeriods, NoiseResult, RasterSampler, Receiver, SourceMetadata, SourceResult,
    TraceCollector,
};

use airborne_view::AirborneRowAccum;
use airport_summary_view::AirportSummaryAccum;
use airport_traffic_view::AirportTrafficRowAccum;
use cruise_view::CruiseRowAccum;
use std::collections::HashMap;

/// Build the per-popup `osm_id` → `ref` lookup from the
/// `airport_lines.arrow` batches. One entry per unique osm_id (one OSM
/// way can have many microsegments — all share the same `ref`). Rows
/// without a `ref` tag are skipped, so `HashMap::get` returns `None`
/// for them and the SegmentTrace falls back to the generic label.
fn build_osm_ref_lookup(batches: &[RecordBatch]) -> HashMap<u64, String> {
    use arrow::array::Array;
    let mut out: HashMap<u64, String> = HashMap::new();
    for batch in batches {
        let (Some(osm_id), Some(ref_col)) = (
            columns::col_i64(batch, "osm_id"),
            columns::col_str(batch, "ref"),
        ) else {
            continue;
        };
        for i in 0..batch.num_rows() {
            if ref_col.is_null(i) {
                continue;
            }
            // `trim()` rejects whitespace-only refs that would otherwise
            // surface as " " labels in the popup (OSM is community-edited;
            // see e.g. `ref=" "` accidental data entries).
            let r = ref_col.value(i).trim();
            if r.is_empty() {
                continue;
            }
            // Synth osm_ids (bit 63 set) live in
            // `synth_airport_lines.arrow`, not here, so the i64 → u64
            // cast is bit-identical for every row in this file.
            out.entry(osm_id.value(i) as u64)
                .or_insert_with(|| r.to_string());
        }
    }
    out
}

/// Every loaded aircraft batch must agree on both class windows; otherwise
/// a mixed release can amplify full-year GA energy by the airline divisor.
/// An empty receiver has no rows to weight.
fn build_class_weights(
    airborne_batches: &[RecordBatch],
    airport_traffic_batches: &[RecordBatch],
    n_days: u16,
) -> Result<noise_compute::emission::aircraft::ClassWeights, String> {
    use noise_compute::emission::aircraft::{ClassWeights, SAMPLE_DAYS_BY_CLASS_KEY};
    if airborne_batches.is_empty() && airport_traffic_batches.is_empty() {
        return Ok(ClassWeights::uniform());
    }
    let mut stamp: Option<String> = None;
    for batch in airborne_batches
        .iter()
        .chain(airport_traffic_batches.iter())
    {
        let v = batch.schema_ref().metadata().get(SAMPLE_DAYS_BY_CLASS_KEY);
        match (v, &stamp) {
            (Some(v), None) => stamp = Some(v.clone()),
            (Some(v), Some(seen)) if v != seen => {
                return Err(format!(
                    "{SAMPLE_DAYS_BY_CLASS_KEY} disagrees across loaded aircraft arrows \
                     ({seen:?} vs {v:?}) — mixed/stale shards; re-extract / re-merge"
                ));
            }
            (Some(_), Some(_)) => {}
            // Current writers always carry the required normalization stamp.
            (None, _) => return ClassWeights::parse(None, n_days),
        }
    }
    ClassWeights::parse(stamp.as_deref(), n_days)
}

/// Add observed aircraft noise after the non-aircraft point computation.
/// Traffic requires complete cell-local summaries of the global movement unions.
#[allow(clippy::too_many_arguments)]
pub fn add_v6_aircraft_to_result(
    result: &mut NoiseResult,
    traces: &mut TraceCollector,
    receiver: &Receiver,
    airborne_batches: &[RecordBatch],
    cruise_batches: &[RecordBatch],
    airport_traffic_batches: &[RecordBatch],
    airport_lines_batches: &[RecordBatch],
    airport_summary: &AirportSummaryAccum,
    rasters: &dyn RasterSampler,
    // Vector obstacles feed airborne building diffraction and ground-ops
    // screening. Cruise remains structurally exempt.
    obstacles: &noise_compute::propagation::obstacle_index::ObstacleSet,
    n_days: u16,
    // Per-kind top-K cap for airborne sub-segment traces — passed to
    // compute_aircraft_v6 so the bounded min-heap in airborne::scatter
    // is sized correctly. query_noise_impl sets this to
    // SEGMENT_TOP_K_PER_KIND (150) on the normal path or
    // SEGMENT_TOP_K_PER_KIND_FULL (1000) on the "Show all" path.
    trace_cap: usize,
) -> Result<(), String> {
    assert_airborne_contract("airborne.arrow", airborne_batches)?;
    assert_cruise_contract("cruise.arrow", cruise_batches)?;
    assert_airport_traffic_contract("airport_traffic.arrow", airport_traffic_batches)?;
    // Contracts guard geometry; the shared window stamp guards normalization.
    let class_weights = build_class_weights(airborne_batches, airport_traffic_batches, n_days)?;
    let airborne_rows = AirborneRowAccum::new(airborne_batches)?;
    let cruise_rows = CruiseRowAccum::new(cruise_batches)?;
    let traffic_rows = AirportTrafficRowAccum::new(airport_traffic_batches)?;
    airport_summary.require_traffic(airport_traffic_batches)?;

    let airborne_views = airborne_rows.views();
    let cruise_view_slices = cruise_rows.views();
    let cruise_views = cruise_view_slices.as_row_views();
    let traffic_views = traffic_rows.views();

    let total_rows = airborne_views.len() + cruise_views.len() + traffic_views.len();
    if total_rows == 0 {
        return Ok(());
    }

    // Airborne screens against one receiver horizon; cruise is exempt.
    let horizon = if airborne_views.is_empty() {
        None
    } else {
        Some(noise_compute::emission::aircraft::ReceiverHorizon::build(
            |lat, lon| rasters.elevation(lat, lon),
            receiver.lat,
            receiver.lon,
            receiver.altitude_m(),
        ))
    };
    let mut crossing_scratch =
        noise_compute::propagation::obstacle_index::CrossingScratch::default();
    let receiver_is_enclosed =
        crate::structure_store::point_inside_enclosed(obstacles, receiver.lat, receiver.lon)
            .is_some();
    let building_horizon = (!airborne_views.is_empty() && !receiver_is_enclosed)
        .then(|| {
            noise_compute::emission::aircraft::BuildingHorizon::build(
                obstacles,
                rasters,
                receiver.lat,
                receiver.lon,
                receiver.altitude_m(),
                &mut crossing_scratch,
            )
        })
        .filter(|horizon| !horizon.is_empty());

    let (mut air_periods, mut air_contribs, band_data) = compute_aircraft_v6(
        receiver,
        &airborne_views,
        &cruise_views,
        rasters,
        horizon.as_ref(),
        building_horizon.as_ref(),
        n_days,
        &class_weights,
        trace_cap,
        Some(traces),
        result.timings.as_mut(),
    );

    // airport_traffic → Doc 29 line-source contributors; fold their
    // per-period Lden into `air_periods` so the top-of-popup Aircraft
    // total includes ground-ops energy (not just its contributor row).
    let timing_on = std::env::var("POPUP_TIMING").as_deref() == Ok("1");
    let t_traffic_start = std::time::Instant::now();
    let mut n_traffic_rows: usize = 0;
    if !traffic_views.is_empty() {
        n_traffic_rows = traffic_views.len();
        // OSM `ref` tags (e.g. runway "06/24") let SegmentTrace render
        // "LKPR RWY 06/24" instead of generic "LKPR runway-roll". Rows
        // without a matching OSM ref fall through to the generic label.
        let osm_ref_lookup = build_osm_ref_lookup(airport_lines_batches);
        let traffic_contribs = compute_airport_traffic::run(
            receiver,
            &traffic_views,
            n_days,
            &class_weights,
            rasters,
            obstacles,
            &osm_ref_lookup,
            Some(airport_summary.lookup()),
            Some(traces),
        );
        if !traffic_contribs.is_empty() {
            let mut all: Vec<NoisePeriods> = Vec::with_capacity(1 + traffic_contribs.len());
            all.push(air_periods);
            for c in &traffic_contribs {
                all.push(c.periods.clone());
            }
            air_periods = noise_compute::periods::sum_periods(&all);
        }
        air_contribs.extend(traffic_contribs);
    }
    let t_traffic = t_traffic_start.elapsed();
    if timing_on {
        eprintln!(
            "popup-stage airport_traffic={:.0}ms (n_traffic_rows={})",
            t_traffic.as_secs_f64() * 1000.0,
            n_traffic_rows,
        );
    }
    if let Some(t) = result.timings.as_mut() {
        t.aircraft_ground_ms = t_traffic.as_secs_f64() * 1000.0;
    }

    if !air_periods.lden_db.is_finite() && air_contribs.is_empty() {
        return Ok(());
    }

    // Upstream Confidence::assess ran with has_aircraft=false (no
    // visibility into popup aircraft arrows there); now that we have
    // rows, bump the score and drop the stale "no ADS-B data" note.
    result.confidence.overall = (result.confidence.overall + 0.15).min(1.0);
    result
        .confidence
        .notes
        .retain(|n| !n.starts_with("Aircraft:"));

    // Compute aircraft `periods_free` from the contributors before we hand
    // them off. Airborne popup contributors retain pre-screen Doc 29 energy;
    // ground ops still reports its received sum because its separate kernel
    // has no free-field variant yet.
    let aircraft_periods_free = noise_compute::periods::sum_periods(
        &air_contribs
            .iter()
            .map(|c| c.periods_free.clone())
            .collect::<Vec<_>>(),
    );

    result.contributors.extend(air_contribs);
    if air_periods.lden_db.is_finite() {
        let displayed_count = result
            .contributors
            .iter()
            .filter(|c| matches!(c.metadata.as_ref(), Some(SourceMetadata::Aircraft(_))))
            .count();
        result.sources.push(SourceResult {
            source_type: LayerKind::Aircraft,
            periods: air_periods,
            periods_free: aircraft_periods_free,
            segment_count: total_rows,
            displayed_count,
        });
    }
    // Fresh per-popup compute; non-aircraft pass never touches this.
    result.aircraft_detail = Some(band_data);
    result.total = sum_periods_linear(&result.sources);
    // Recompute `total_free` after aircraft merge — the noise-compute pass
    // set it from non-aircraft sources only; we now have the full set.
    result.total_free = noise_compute::periods::sum_periods(
        &result
            .sources
            .iter()
            .map(|s| s.periods_free.clone())
            .collect::<Vec<_>>(),
    );

    // Re-finalize over the merged contributor set so aircraft compete
    // for top-N slots (non-aircraft pass already committed its top-30
    // + `other_sources_lden`; appending aircraft rows would leave a
    // padded list and a stale tail bucket).
    let other_lden_existing = result.other_sources_lden;
    let merged = std::mem::take(&mut result.contributors);
    let finalized = noise_compute::present::finalize_popup_contributors(merged, 30);
    result.contributors = finalized.shown;
    result.other_sources_lden = combine_other_lden(other_lden_existing, finalized.other_lden_db);
    Ok(())
}

/// Linear-energy sum of two `other_sources_lden` numbers. Either side
/// can be `NEG_INFINITY` (no leftovers), in which case the other side
/// passes through unchanged.
fn combine_other_lden(a: f64, b: f64) -> f64 {
    let to_lin = |v: f64| {
        if v.is_finite() {
            10f64.powf(v / 10.0)
        } else {
            0.0
        }
    };
    let total = to_lin(a) + to_lin(b);
    if total > 0.0 {
        10.0 * total.log10()
    } else {
        f64::NEG_INFINITY
    }
}

/// Energy-sum the per-source periods after the aircraft contribution
/// has been pushed onto `sources`. The non-aircraft pass already filled
/// `result.total` for road/rail/building/industrial, but that total
/// predates the aircraft push so we recompute from scratch here.
fn sum_periods_linear(sources: &[SourceResult]) -> NoisePeriods {
    let to_lin = |db: f64| -> f64 {
        if db.is_finite() {
            (db * std::f64::consts::LN_10 * 0.1).exp()
        } else {
            0.0
        }
    };
    let to_db = |lin: f64| -> f64 {
        if lin > 0.0 {
            10.0 * lin.log10()
        } else {
            f64::NEG_INFINITY
        }
    };
    let (mut day, mut eve, mut night) = (0.0f64, 0.0f64, 0.0f64);
    for s in sources {
        day += to_lin(s.periods.ld_db);
        eve += to_lin(s.periods.le_db);
        night += to_lin(s.periods.ln_db);
    }
    if day + eve + night <= 0.0 {
        return NoisePeriods::silence();
    }
    noise_compute::periods::periods(to_db(day), to_db(eve), to_db(night))
}

use square_store::aircraft_contract::{
    AIRBORNE_CONTRACT, AIRPORT_TRAFFIC_CONTRACT, CRUISE_CONTRACT, SCHEMA_VERSION,
};

fn assert_metadata_value(
    label: &str,
    batches: &[RecordBatch],
    key: &str,
    expected: &str,
    recovery: &str,
) -> Result<(), String> {
    for (idx, batch) in batches.iter().enumerate() {
        let actual = batch.schema_ref().metadata().get(key).map(String::as_str);
        if actual == Some(expected) {
            continue;
        }
        return Err(format!(
            "{label}[batch {idx}] {key} mismatch (expected {expected}, got {actual:?}) — {recovery}"
        ));
    }
    Ok(())
}

/// A receiver can load many squares; every batch must belong to this release.
pub(super) fn assert_schema_version(label: &str, batches: &[RecordBatch]) -> Result<(), String> {
    assert_metadata_value(
        label,
        batches,
        "schema_version",
        SCHEMA_VERSION,
        "re-extract aircraft pipeline",
    )
}

/// Guard the `airport_traffic.arrow` dimensional contract. Mirrors
/// [`assert_schema_version`] but checks the orthogonal
/// `airport_traffic_contract` metadata key, which encodes the
/// quantity stored in `band_energy_lin` (see
/// [`AIRPORT_TRAFFIC_CONTRACT`]).
pub(super) fn assert_airport_traffic_contract(
    label: &str,
    batches: &[RecordBatch],
) -> Result<(), String> {
    // Enforce schema_version too: metadata corruption could leave only
    // one of the two stamps intact.
    assert_schema_version(label, batches)?;
    assert_metadata_value(
        label,
        batches,
        "airport_traffic_contract",
        AIRPORT_TRAFFIC_CONTRACT,
        "re-extract aircraft pipeline",
    )
}

/// Geometry stamps distinguish z30/Int16 data from incompatible float artifacts.
pub(super) fn assert_airborne_contract(label: &str, batches: &[RecordBatch]) -> Result<(), String> {
    assert_schema_version(label, batches)?;
    assert_metadata_value(
        label,
        batches,
        "airborne_contract",
        AIRBORNE_CONTRACT,
        "re-extract aircraft pipeline",
    )
}

/// Guard the z9 `cruise.arrow` spatial contract. The z9 producer stores
/// explicit `lon`/`lat`; dev1 contracts store a legacy cell id instead.
pub(super) fn assert_cruise_contract(label: &str, batches: &[RecordBatch]) -> Result<(), String> {
    assert_schema_version(label, batches)?;
    assert_metadata_value(
        label,
        batches,
        "cruise_contract",
        CRUISE_CONTRACT,
        "rebuild the cruise z9 data",
    )
}

#[cfg(test)]
mod airport_summary_tests;
#[cfg(test)]
mod producer_roundtrip_tests;
#[cfg(test)]
mod periodic_selection_tests;

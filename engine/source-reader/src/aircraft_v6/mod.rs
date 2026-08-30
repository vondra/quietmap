//! Run `compute_aircraft_v6` over the popup arrows and merge its output
//! into the `NoiseResult` returned by `compute_at_point_with_traces`.
//!
//! Lifetime story: `RecordBatch` arrays are `Arc<dyn Array>`-backed, so
//! source-reader's hex store can clone the batches cheaply and drop its
//! `RwLock`. `AirborneRowAccum<'a>` (Opt C) is now zero-copy — it borrows
//! `&[f32]` / `&str` directly out of the live arrow buffers tied to
//! the caller's `&[RecordBatch]` Vec. `CruiseRowAccum` and
//! `AirportTrafficRowAccum` still snapshot columns into owned
//! `Vec<T>` / `String` (Tier 3 to convert; small batches make the
//! savings less load-bearing for those layers).

mod airborne_view;
pub mod airport_summary_view;
mod airport_traffic_view;
mod columns;
mod cruise_view;

use std::path::Path;

use arrow::record_batch::RecordBatch;
use noise_compute::compute::aircraft_v6::{
    airport_traffic as compute_airport_traffic, compute_aircraft_v6,
};
use noise_compute::types::{
    LayerKind, NoisePeriods, NoiseResult, RasterSampler, Receiver, SourceMetadata, SourceResult,
    TraceCollector,
};

use airborne_view::AirborneRowAccum;
use airport_summary_view::load_airport_summary_cached;
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

/// Build the GA full-year hybrid per-class weight LUT from the
/// `sample_days_by_class` metadata stamped on the airborne and
/// airport_traffic arrows. Mirrors the
/// heatmap's `worklist::resolve_class_weights` strictness: every loaded
/// airborne/airport_traffic batch MUST carry the stamp and they must all
/// AGREE — a missing stamp or a disagreement across the grid_disk(1)
/// hexes (mixed/stale shards) FAILS LOUD (owner directive 2026-06-12; no
/// uniform fallback), since the first-stamp-wins shortcut could silently
/// apply one window's LUT to rows from another and re-introduce the GA
/// +14.8 dB phantom. Because the vector embeds both windows, this also
/// catches the case where `n_days` (the divisor) differs across hexes.
/// When there are no aircraft batches at all (rural receiver), returns
/// the uniform LUT — there is nothing to weight.
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
            // A batch passed the v4/v9 contract gate but lacks the stamp:
            // impossible for current writers, so treat as a loud error.
            (None, _) => return ClassWeights::parse(None, n_days),
        }
    }
    ClassWeights::parse(stamp.as_deref(), n_days)
}

/// Run `compute_aircraft_v6` over the popup arrows and merge its output
/// into an existing `NoiseResult`. Caller is expected to have invoked
/// `compute_at_point_with_traces` first.
///
/// `airport_summary_path` points at the global `airport_summary.arrow`
/// sidecar (typically `<prepared>/aircraft/airport_summary.arrow`).
/// When the file is absent OR an airport is missing from it, popup
/// arr/dep/observed counts return zero; there is no fallback to per-row
/// sums, which over-count by 4-8×.
///
/// Returns `Err(String)` when any of the popup arrows fails its schema
/// check (`v15` for airborne/cruise, `airport_traffic_v8` for the
/// ground-ops arrow), so the popup HTTP path can map the failure to a
/// structured 500 response with an operator-actionable message.
// 13 args: the popup aircraft entry accretes one param per physics input.
// Bundle into a context struct when another input is added.
#[allow(clippy::too_many_arguments)]
pub fn add_v6_aircraft_to_result(
    result: &mut NoiseResult,
    traces: &mut TraceCollector,
    receiver: &Receiver,
    airborne_batches: &[RecordBatch],
    cruise_batches: &[RecordBatch],
    airport_traffic_batches: &[RecordBatch],
    airport_lines_batches: &[RecordBatch],
    airport_summary_path: Option<&Path>,
    rasters: &dyn RasterSampler,
    barriers: &[noise_compute::types::Barrier],
    // Vector obstacles (geodata-v2): threads into ground-ops screening
    // (`compute_airport_traffic::run`) so airport ground shares the popup's
    // physics; airborne/cruise are structurally exempt (no ground rays).
    obstacles: Option<&noise_compute::propagation::obstacle_index::ObstacleSet>,
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
    // GA full-year hybrid per-class weight LUT — built from the
    // `sample_days_by_class` metadata the airborne / airport_traffic
    // arrows stamp. FAILS LOUD when rows
    // exist but the stamp is missing (owner directive 2026-06-12 — no
    // compat shim): the arrows predate the hybrid contract and must be
    // re-extracted. The contract bumps (airborne v4 / airport_traffic v9)
    // already reject such files above; this is the in-kernel safety net.
    let class_weights = build_class_weights(airborne_batches, airport_traffic_batches, n_days)?;
    let airborne_rows = AirborneRowAccum::new(airborne_batches);
    let cruise_rows = CruiseRowAccum::new(cruise_batches);
    let traffic_rows = AirportTrafficRowAccum::new(airport_traffic_batches);
    // Only the `!traffic_views.is_empty()` branch below ever consults the
    // summary lookup, so when this popup's 7-hex ring holds no airport_traffic
    // rows the whole-world ~11 MB airport_summary.arrow is never used. Gate the
    // load on `has_traffic_rows` so an empty-desert / no-airport click skips the
    // open + Arrow parse + ~50k-row HashMap build entirely. Before this gate
    // EVERY popup (incl. rural clicks with zero nearby aircraft) paid the full
    // load one line before the `total_rows == 0` bail-out below — and, uncached,
    // re-paid it on every request, which is a real CPU wall at popup scale.
    //
    // When traffic rows DO exist the sidecar is MANDATORY: a missing sidecar
    // or unwired path is a fatal pipeline
    // state (Stage 2C reduce did not run, or operator forgot to copy it), raised
    // as a loud `eprintln!` + `Err` so the popup HTTP path returns 500 instead of
    // silently showing zero arr/dep counts (indistinguishable from "no ADS-B data").
    let has_traffic_rows = !airport_traffic_batches.is_empty();
    let airport_summary_accum = if !has_traffic_rows {
        None
    } else {
        let p = airport_summary_path.ok_or_else(|| {
            eprintln!(
                "ERROR: airport_traffic.arrow rows present but airport_summary_path is None \
                 — caller must wire the sidecar location whenever traffic rows are loaded."
            );
            "airport_summary_path = None with airport_traffic.arrow rows present \
             — wire `<prepared>/aircraft/airport_summary.arrow`"
                .to_string()
        })?;
        let loaded = load_airport_summary_cached(p)?;
        if loaded.is_none() {
            eprintln!(
                "ERROR: airport_traffic.arrow rows present but airport_summary.arrow \
                 missing at {} — Stage 2C reduce phase did not run, or scope changed \
                 between extract and popup. Re-extract or copy the sidecar.",
                p.display()
            );
            return Err(format!(
                "airport_summary.arrow missing at {} (required when airport_traffic.arrow present) \
                 — re-run Stage 2C reduce phase",
                p.display()
            ));
        }
        loaded
    };

    let airborne_views = airborne_rows.views();
    let cruise_view_slices = cruise_rows.views();
    let cruise_views = cruise_view_slices.as_row_views();
    let traffic_views = traffic_rows.views();

    let total_rows = airborne_views.len() + cruise_views.len() + traffic_views.len();
    if total_rows == 0 {
        return Ok(());
    }

    // C2 terrain screening (P0, default OFF): one receiver horizon per
    // popup query, built from the DEM terrain surface (DSM-biased:
    // GLO-30 includes canopy/buildings) via the same tile-cached
    // bilinear sampler the rest of the popup uses. ~1.5 k samples
    // ≈ 0.1-0.3 ms. Env-gated like POPUP_TIMING until P1 measurement /
    // P2 default-flip; unset ⇒ `None` ⇒ byte-identical popup output.
    let horizon = if std::env::var("QM_AIRBORNE_HORIZON").as_deref() == Ok("1") {
        Some(noise_compute::emission::aircraft::ReceiverHorizon::build(
            |lat, lon| rasters.elevation(lat, lon),
            receiver.lat,
            receiver.lon,
            receiver.altitude_m(),
        ))
    } else {
        None
    };

    let (mut air_periods, mut air_contribs, band_data) = compute_aircraft_v6(
        receiver,
        airborne_views,
        &cruise_views,
        rasters,
        horizon.as_ref(),
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
        // Borrow of the process-cached map — no per-query rebuild.
        let airport_summary_lookup = airport_summary_accum.as_ref().map(|a| a.lookup());
        let traffic_contribs = compute_airport_traffic::run(
            receiver,
            &traffic_views,
            n_days,
            &class_weights,
            rasters,
            barriers,
            obstacles,
            &osm_ref_lookup,
            airport_summary_lookup,
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

    // Compute aircraft `periods_free` from the contributors before we
    // hand them off. Airborne and cruise kernels apply no terrain /
    // screening (Doc 29 free-field NPD), so each contributor's
    // `periods_free == periods`. Ground ops sets the same — see TODO on
    // `SourceResult.periods_free` doc in noise-compute/src/types/propagation.rs
    // to pull real free-field periods out of the airport_traffic kernel
    // variants. This remains an explicit approximation until then.
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

/// Stamp written by every aircraft-extract Arrow file. Inline copy
/// (not a build-dep) keeps arrow IPC / parquet / anyhow out of the
/// popup runtime; must move in lock-step with `aircraft-extract::SCHEMA_VERSION`.
/// v15 adds mandatory endpoint terrain elevations to airborne sub-segments
/// and Stage 1. Earlier files cannot supply them and must be rejected.
pub(super) const EXPECTED_SCHEMA_VERSION: &str = "v15";

/// The `airport_traffic.arrow` semantic contract. `schema_version`
/// only guards column types/order; this guards what those columns
/// mean today: `band_energy_lin` = raw Σ over n_days of Z-weighted
/// energy at 25 m perpendicular (v6 convention; consumer divides via
/// `period_leq(_, n_days_f, _)`); per-row scalar `unique_*_count` +
/// per-microseg UNION `microseg_unique_*` replace the v4 `flight_ids`
/// list. Airport-level UNION across R4s lives in the separate
/// `airport_summary.arrow` sidecar.
/// v9 (GA full-year hybrid): the microseg UNIONs split into non-GA +
/// `microseg_unique_ga_*`, and the file stamps `sample_days_by_class`
/// so the consumer weights GA energy at `1/ga_n_days`. A v8 reader would
/// double-weight GA at 1/12 — the bump refuses it.
pub(super) const EXPECTED_AIRPORT_TRAFFIC_CONTRACT: &str = "airport_traffic_v9";

/// `airborne.arrow` sub-segment column-shape contract. v2 (K3) keeps
/// only `terrain_start_elev_m` / `terrain_end_elev_m`; v1 stored five
/// elevs (start / q1 / mid / q3 / end). Popup reader hard-fails on a
/// v1 file because the 13-col offset shifts every read past
/// `flags` — silent decoding would alias `terrain_q1_elev_m` slice
/// over what v2 treats as `terrain_end_elev_m`.
/// v4 (GA full-year hybrid): no column change, but the file carries the
/// `sample_days_by_class` vector the consumer REQUIRES to weight GA rows
/// at `1/ga_n_days`. A v3 reader ignores it and ships the +14.8 dB phantom.
pub(super) const EXPECTED_AIRBORNE_CONTRACT: &str = "airborne_v4";

/// Expected `cruise_contract` metadata. v16 drops the tautological
/// `flags` column (always IS_DEPARTURE per Doc 29 §A.3.2). Older
/// cruise.arrow files lack columns the popup expects; silent skip
/// would zero out cruise contributions at every receiver.
pub(super) const EXPECTED_CRUISE_CONTRACT: &str = "cruise_v17";

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

/// Verify `schema_version` on every batch in the slice — the caller
/// merges batches across R4 cells, so one hex's current batches and a
/// sibling's stale batches can land in the same slice. Wrong-schema
/// readers silently drop rows via `col_list(...)` → `continue`; this
/// is the loud safety net.
pub(super) fn assert_schema_version(label: &str, batches: &[RecordBatch]) -> Result<(), String> {
    assert_metadata_value(
        label,
        batches,
        "schema_version",
        EXPECTED_SCHEMA_VERSION,
        "re-extract aircraft pipeline",
    )
}

/// Guard the `airport_traffic.arrow` dimensional contract. Mirrors
/// [`assert_schema_version`] but checks the orthogonal
/// `airport_traffic_contract` metadata key, which encodes the
/// quantity stored in `band_energy_lin` (see
/// [`EXPECTED_AIRPORT_TRAFFIC_CONTRACT`]).
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
        EXPECTED_AIRPORT_TRAFFIC_CONTRACT,
        "re-extract aircraft pipeline",
    )
}

/// Guard the `airborne.arrow` sub-segment column-shape contract.
/// Mirrors [`assert_airport_traffic_contract`] but checks the
/// `airborne_contract` metadata key set by Stage 2A. Pre-K3 (v1) files
/// have three extra terrain columns the v2 popup reader would silently
/// alias over `terrain_end_elev_m`, producing wrong Filter D cuts on
/// every airborne sub-segment.
pub(super) fn assert_airborne_contract(label: &str, batches: &[RecordBatch]) -> Result<(), String> {
    assert_schema_version(label, batches)?;
    assert_metadata_value(
        label,
        batches,
        "airborne_contract",
        EXPECTED_AIRBORNE_CONTRACT,
        "re-extract aircraft pipeline",
    )
}

/// Guard the `cruise.arrow` spatial-resolution contract. Pre-v15 files
/// stored an `r8_hex` column; the popup/heatmap readers silently skip
/// batches whose `r7_hex` column is missing, hiding the version skew.
pub(super) fn assert_cruise_contract(label: &str, batches: &[RecordBatch]) -> Result<(), String> {
    assert_schema_version(label, batches)?;
    assert_metadata_value(
        label,
        batches,
        "cruise_contract",
        EXPECTED_CRUISE_CONTRACT,
        "re-extract cruise stage 2B",
    )
}

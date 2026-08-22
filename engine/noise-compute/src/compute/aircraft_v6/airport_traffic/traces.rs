//! Per-microsegment `SegmentTrace` emission for airport ground-traffic ops —
//! the popup-only trace builder for the aircraft_v6 airport_traffic kernel.
use super::*;

/// Build one `SegmentTrace` per microsegment from the per-microsegment
/// energy accumulator. Geometry is the microsegment's start/end pair;
/// `cp_lat/cp_lon` is the closest point on the segment to the receiver,
/// matching the hot loop's energy anchor (see commit fe053b0 heatmap
/// parity).
///
/// `osm_ref_lookup` maps real-OSM `osm_id` → `ref` tag (e.g. "06/24"
/// on a runway way). When present for this microsegment's id, the
/// trace name becomes `"{airport_key} {RWY|TWY|apron} {ref}"` instead
/// of the generic `"{airport_key} {ops_kind_label}"`. Stopway/airstrip
/// rows collapse to `ops_kind = RUNWAY_ROLL` in Stage 2C
/// (`stage_2c/airport_traffic_writer.rs`), so they share the `RWY`
/// prefix here — there is no distinct `STW` arm. Synthetic osm_ids
/// (top bit set) are never in the lookup by construction in the
/// source-reader builder.
/// `microsegs_by_id` arrives sorted on `(osm_id, segment_idx)` — see the
/// call site in [`super::run`]; both passes below depend on that order.
pub(super) fn emit_segment_traces(
    traces: &mut crate::types::TraceCollector,
    microsegs_by_id: Vec<((u64, u16), MicrosegAcc)>,
    microseg_cache: &HashMap<(u64, u16), MicrosegPath>,
    n_days_f: f64,
    // GA-class divisor for the split-union microsegment movement counts.
    ga_n_days_f: f64,
    recv_lat: f64,
    recv_lon: f64,
    refl_db: f64,
    osm_ref_lookup: &HashMap<u64, String>,
) {
    // Mirror the hot loop's half-pixel divergence floor so the trace's
    // `dist_m`, `flc_delta_trace`, and `geometric_db` match the energy
    // the kernel above evaluated (and the HM3 pixel under the cursor).
    let pixel_floor_m = popup_pixel_floor_m(recv_lat);
    // Synth osm_id bit-test: synthetic IDs (Stage 1.5 DBSCAN output)
    // set bit 63. Real OSM IDs are positive `i64` so bit 63 = 0.
    // Defined inline rather than imported from aircraft-extract
    // (this crate doesn't depend on aircraft-extract).
    const SYNTHETIC_OSM_ID_BIT: u64 = 1u64 << 63;
    use crate::types::{
        BaselineTrace, CnossosBreakdown, EmissionTrace, GroundTrace, LdenVariants,
        PathProfileTrace, PerPeriod, ProfileMixEntry, PropagationBreakdown, ScreeningTrace,
        SegmentTrace, TerrainTrace, VegetationTrace,
    };

    let zero_bands = [0.0f64; NUM_BANDS];
    // Per-band A-weighted source Lw per period. `band_energy_lin_per_period`
    // accumulates raw Σ over n_days from v6 `band_energy_lin`; `period_leq`
    // divides by `n_days_f × period_seconds` to recover dB(A) per m of source
    // (matching CNOSSOS line sources). Returns `NEG_INFINITY` for silent /
    // zero-length cases so the frontend can hide them via `Number.isFinite`.
    let lw_bands_per_period = |acc: &MicrosegAcc| -> PerPeriod<[f64; NUM_BANDS]> {
        let calc = |p: usize| -> [f64; NUM_BANDS] {
            let mut out = [f64::NEG_INFINITY; NUM_BANDS];
            let ps = aircraft::PERIOD_SECONDS[p];
            if acc.length_m <= 0.0 {
                return out;
            }
            for i in 0..NUM_BANDS {
                let lin_a = acc.band_energy_lin_per_period[p][i] * A_WEIGHT_LIN[i];
                out[i] = aircraft::period_leq(lin_a / acc.length_m, n_days_f, ps);
            }
            out
        };
        PerPeriod {
            day: calc(0),
            evening: calc(1),
            night: calc(2),
        }
    };
    // A-weighted scalar Lw per period — sum-of-energies across the
    // 8 A-weighted bands divided by `n_days × period_seconds × length`.
    // Used by the popup's per-period table next to L_rec.
    let lw_db_a_per_period = |acc: &MicrosegAcc| -> PerPeriod<f64> {
        let calc = |p: usize| -> f64 {
            let ps = aircraft::PERIOD_SECONDS[p];
            if acc.length_m <= 0.0 {
                return f64::NEG_INFINITY;
            }
            let lin_a: f64 = (0..NUM_BANDS)
                .map(|i| acc.band_energy_lin_per_period[p][i] * A_WEIGHT_LIN[i])
                .sum();
            aircraft::period_leq(lin_a / acc.length_m, n_days_f, ps)
        };
        PerPeriod {
            day: calc(0),
            evening: calc(1),
            night: calc(2),
        }
    };
    // Per-band received Lp at the popup point — already A-weighted
    // and propagation-applied in the hot loop. Divide by
    // `n_days × period_seconds` to recover average power per period.
    let received_bands_per_period = |acc: &MicrosegAcc| -> PerPeriod<[f64; NUM_BANDS]> {
        let calc = |p: usize| -> [f64; NUM_BANDS] {
            let mut out = [f64::NEG_INFINITY; NUM_BANDS];
            let ps = aircraft::PERIOD_SECONDS[p];
            // `i` indexes the 2D `received_bands_lin_per_period[p][i]` and `out[i]`;
            // index loop kept, per-band Leq order is part of popup byte parity.
            #[allow(clippy::needless_range_loop)]
            for i in 0..NUM_BANDS {
                out[i] =
                    aircraft::period_leq(acc.received_bands_lin_per_period[p][i], n_days_f, ps);
            }
            out
        };
        PerPeriod {
            day: calc(0),
            evening: calc(1),
            night: calc(2),
        }
    };

    let periods_from_energy = |pe: [f64; 3]| -> crate::types::NoisePeriods {
        let ld = aircraft::period_leq(pe[0], n_days_f, aircraft::PERIOD_SECONDS[0]);
        let le = aircraft::period_leq(pe[1], n_days_f, aircraft::PERIOD_SECONDS[1]);
        let ln = aircraft::period_leq(pe[2], n_days_f, aircraft::PERIOD_SECONDS[2]);
        periods::periods(ld, le, ln)
    };

    // Pre-compute Lden per microseg (single pass) for both the per-
    // group dominant flag AND the per-source top-K cap. The cap
    // mirrors `source-reader/lib.rs:SEGMENT_TOP_K_PER_KIND`; capping
    // at emission avoids ~2 800 SegmentTrace + Box<PropagationBreakdown>
    // allocations per LKPR popup (≈ 100 ms cascade drop cost
    // previously paid in `apply_segment_top_k_with_cap`).
    const GROUND_TRACE_CAP: usize = 150;
    let mut by_lden: Vec<((u64, u16), f64)> = Vec::with_capacity(microsegs_by_id.len());
    let mut dominant_lden: HashMap<(String, u8), f64> = HashMap::new();
    for ((osm_id, segment_idx), acc) in microsegs_by_id.iter() {
        let lden = periods_from_energy(acc.period_energy_full).lden_db;
        if !lden.is_finite() {
            continue;
        }
        by_lden.push(((*osm_id, *segment_idx), lden));
        let key = (acc.airport_key.clone(), acc.ops_kind);
        dominant_lden
            .entry(key)
            .and_modify(|m| {
                if lden > *m {
                    *m = lden;
                }
            })
            .or_insert(lden);
    }
    by_lden.sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    by_lden.truncate(GROUND_TRACE_CAP);
    let keep: std::collections::HashSet<(u64, u16)> = by_lden.into_iter().map(|(k, _)| k).collect();

    for ((osm_id, segment_idx), acc) in microsegs_by_id {
        if !keep.contains(&(osm_id, segment_idx)) {
            continue;
        }
        let periods = periods_from_energy(acc.period_energy_full);
        if !periods.lden_db.is_finite() {
            continue;
        }
        // Per-microsegment variant Ldens — for the per-row
        // path-effect breakdown in the Noise Segments tab drill-down.
        // Each variant's Lden minus the full Lden gives the ΔL_A
        // attributable to that effect at THIS specific microsegment.
        let lden_or_neg_inf = |pe: [f64; 3]| -> f64 {
            let v = periods_from_energy(pe).lden_db;
            if v.is_finite() {
                v
            } else {
                f64::NEG_INFINITY
            }
        };
        let lden_no_terrain = lden_or_neg_inf(acc.period_energy_no_terrain);
        let lden_no_screening = lden_or_neg_inf(acc.period_energy_no_screening);
        let lden_no_vegetation = lden_or_neg_inf(acc.period_energy_no_vegetation);
        let lden_no_atmospheric = lden_or_neg_inf(acc.period_energy_no_atmospheric);
        let lden_no_ground = lden_or_neg_inf(acc.period_energy_no_ground);
        // `free_field` (no path effects, no ground): use no_atm with
        // ground removed. Approximation: use no_ground value (which
        // strips the A_gr from the gob max), close enough for the
        // popup baseline-vs-path-effects comparison.
        let lden_free = lden_no_ground;
        let (ops_class, ops_kind_label) = match acc.ops_kind {
            GROUND_OPS_KIND_RUNWAY_ROLL => ("runway", "runway-roll"),
            GROUND_OPS_KIND_TAXI => ("taxi", "taxi"),
            GROUND_OPS_KIND_APRON_MOVEMENT => ("apron", "apron"),
            _ => ("unknown", "unknown"),
        };
        // When the OSM way carries a `ref` tag (e.g. runway "06/24",
        // taxiway "A"), label the trace with it so the popup reads
        // "LKPR RWY 06/24" rather than the generic "LKPR runway-roll".
        // Synth osm_ids (bit 63 set) never enter the lookup, so they
        // fall through to the generic label automatically.
        let osm_ref = osm_ref_lookup.get(&osm_id).map(String::as_str);
        let name = match (acc.ops_kind, osm_ref) {
            (GROUND_OPS_KIND_RUNWAY_ROLL, Some(r)) => format!("{} RWY {r}", acc.airport_key),
            (GROUND_OPS_KIND_TAXI, Some(r)) => format!("{} TWY {r}", acc.airport_key),
            (GROUND_OPS_KIND_APRON_MOVEMENT, Some(r)) => {
                format!("{} apron {r}", acc.airport_key)
            }
            _ => format!("{} {ops_kind_label}", acc.airport_key),
        };
        // Synthetic osm_ids set bit 63; reinterpret-cast to i64
        // would land in negative-i64 territory which downstream
        // OSM-aware tools interpret as a relation member, and JS
        // `Number` truncates for u64 > 2⁵³. Drop the id for synth
        // rows; the popup labels them by airport_key + ops_kind.
        let osm_id_out = if osm_id & SYNTHETIC_OSM_ID_BIT != 0 {
            None
        } else {
            Some(osm_id as i64)
        };
        // Closest-point geometry — matches the hot-loop's distance + path
        // anchor so the trace's `dist_m` and `cp_lat/cp_lon` line up
        // with the energy the kernel actually evaluated.
        let pts_trace = point_to_segment_full(
            recv_lat,
            recv_lon,
            acc.start_lat,
            acc.start_lon,
            acc.end_lat,
            acc.end_lon,
        );
        let cp_lat = pts_trace.cp_lat;
        let cp_lon = pts_trace.cp_lon;
        let dist_m = pts_trace.d_endpoint_m.max(pixel_floor_m);
        // Line-source angle term for the popup propagation breakdown —
        // the dB the receiver formula picks up from finite-line geometry
        // vs an infinite line at the same perpendicular distance.
        // GSE rows have no line-source angle (point-source kinematic
        // integral); zero them for the display field. The legacy semantic
        // was "FLC delta vs (L=segment, d=25m, frac=0.5) reference"; the
        // new value uses signed-fraction θ math identical to the hot
        // loop, so the trace reflects what the kernel actually evaluated.
        let flc_delta_trace = {
            let d_perp = pts_trace.d_perp_m.max(pixel_floor_m);
            let l = acc.length_m;
            let rx_along = pts_trace.fraction * l;
            let theta = ((l - rx_along) / d_perp).atan() + (rx_along / d_perp).atan();
            10.0 * (theta.max(1e-12) / std::f64::consts::PI).log10()
        };
        let is_dominant = dominant_lden
            .get(&(acc.airport_key.clone(), acc.ops_kind))
            .map(|&max_lden| (periods.lden_db - max_lden).abs() < 1e-6)
            .unwrap_or(false);

        // Per-microsegment path effects (terrain / screening /
        // vegetation / ground) come straight from the cached
        // `MicrosegPath` the hot loop populated. Geometric divergence
        // and atmospheric absorption are re-derived from `dist_m`
        // here (cheap, avoids carrying them on `MicrosegAcc`).
        let path = microseg_cache.get(&(osm_id, segment_idx));
        // Aircraft line-source geometric divergence, stored as a POSITIVE
        // magnitude (`10·log10(d_perp / π)`) to match `traces::baseline_trace`'s
        // convention — the popup renders `−geometric_db`, so the on-screen value
        // is the actual `10·log10(π / d_perp)` attenuation (negative, growing
        // with distance). Storing the raw (negative) line term here made the FE
        // negate it into a bogus positive "+15 dB gain". The finite-line
        // correction is surfaced separately in `flc_delta_trace`. GSE rows would
        // prefer the point-source `20·log10(d) − …` form, but they're typically
        // ≪ 5 % of microsegment energy, so the aircraft decomposition dominates.
        let d_perp_disp = pts_trace.d_perp_m.max(pixel_floor_m);
        let geometric_db = 10.0 * (d_perp_disp / std::f64::consts::PI).log10();
        let mut atmospheric_bands = zero_bands;
        let mut ground_bands = zero_bands;
        let mut terrain_bands = zero_bands;
        let mut screening_bands = zero_bands;
        let mut vegetation_bands = zero_bands;
        let mut ground_g = 0.0;
        if let Some(p) = path {
            ground_g = p.ground_g;
            let d_km = (dist_m - GROUND_OPS_REF_OFFSET_M).max(0.0) / 1000.0;
            // Road/airborne convention: POSITIVE = attenuation (loss),
            // NEGATIVE = boost (rare, soft-ground LF interference can
            // give A_gr < 0 per CNOSSOS-EU §2.5.15). The per-band
            // tooltips render with `signed=true` so the +/- direction
            // matches between road and ground rows.
            for i in 0..NUM_BANDS {
                atmospheric_bands[i] = ALPHA_ATM[i] * d_km;
                ground_bands[i] =
                    crate::propagation::iso9613::aircraft_ground_atten_db(i, p.ground_g);
                terrain_bands[i] = p.terrain_atten_db[i];
                screening_bands[i] = p.screening_atten_db[i];
                vegetation_bands[i] = p.vegetation_atten_db[i];
            }
        }
        // Per-period source Lw + received Lp bands — both derived
        // from the per-period energy accumulators populated in the
        // hot loop.
        let lw_bands = lw_bands_per_period(&acc);
        let lw_db_a = lw_db_a_per_period(&acc);
        let received_bands = received_bands_per_period(&acc);
        // v5: per-microsegment counts are row-replicated scalars
        // (`microseg_unique_*`) captured into MicrosegAcc on first
        // insert — popup reads them directly without HashSet union.
        // v9: each split into `non_ga / n_days + ga / ga_n_days` so a
        // one-off GA movement reads at its full-year frequency.
        let split = |non_ga: u32, ga: u32| non_ga as f64 / n_days_f + ga as f64 / ga_n_days_f;
        let observed_movements = split(acc.unique_count, acc.unique_ga_count);
        let arrivals_per_day = split(acc.unique_arr_count, acc.unique_ga_arr_count);
        let departures_per_day = split(acc.unique_dep_count, acc.unique_ga_dep_count);
        // GSE is airline-pass only — no GA split.
        let gse_per_day: [f64; NUM_GSE_CLASSES] =
            std::array::from_fn(|i| acc.unique_gse_count_per_class[i] as f64 / n_days_f);
        // Top-3 aircraft classes by energy share at this microsegment.
        // Mirrors the airport-level `profile_mix` so the popup row
        // can use the same renderer.
        let total_class_energy: f64 = acc.class_energy.iter().sum();
        let mut class_mix: Vec<ProfileMixEntry> = Vec::new();
        if total_class_energy > 0.0 {
            let mut ranked: Vec<(usize, f64)> = acc
                .class_energy
                .iter()
                .enumerate()
                .filter(|(_, &e)| e > 0.0)
                .map(|(i, &e)| (i, e))
                .collect();
            ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            for (idx, energy) in ranked.into_iter().take(3) {
                let rep_profile_idx = aircraft::CLASS_REP_PROFILE_IDX[idx] as usize;
                class_mix.push(ProfileMixEntry {
                    class: idx as u8,
                    share: energy / total_class_energy,
                    rep_typecode: aircraft::profile_typecode(rep_profile_idx).to_string(),
                });
            }
        }
        let osm_ref_owned = osm_ref.map(|s| s.to_string());

        traces.segments.push(SegmentTrace {
            kind: LayerKind::Aircraft,
            osm_id: osm_id_out,
            segment_idx: segment_idx as i16,
            name,
            subtype: "ground_ops".to_string(),
            is_dominant_of_group: is_dominant,
            start_lat: acc.start_lat,
            start_lon: acc.start_lon,
            end_lat: acc.end_lat,
            end_lon: acc.end_lon,
            cp_lat,
            cp_lon,
            length_m: acc.length_m,
            dist_m,
            d_slant_m: dist_m,
            bridge: false,
            tunnel: false,
            emission: EmissionTrace::AircraftGround {
                class: ops_class,
                observed_movements,
                modeled_movements: 0.0,
                arrivals_per_day,
                departures_per_day,
                gse_per_day,
                class_mix,
                osm_ref: osm_ref_owned,
            },
            propagation: PropagationBreakdown::Cnossos(Box::new(CnossosBreakdown {
                baseline: BaselineTrace {
                    geometric_db,
                    atmospheric_bands,
                    ground_factor_g: ground_g,
                    source_height_m: GROUND_OPS_SOURCE_HEIGHT_M,
                    finite_line_corr_db: flc_delta_trace,
                    reflection_boost_db: refl_db,
                },
                path_profile: PathProfileTrace {
                    t: Vec::new(),
                    elevation_m: Vec::new(),
                    building_h_m: Vec::new(),
                    forest_u8: Vec::new(),
                    imd_u8: Vec::new(),
                    dist_m,
                    step_m_med: 0.0,
                    src_lat: cp_lat,
                    src_lon: cp_lon,
                    rcv_lat: recv_lat,
                    rcv_lon: recv_lon,
                    src_alt_m: 0.0,
                    rcv_alt_m: 0.0,
                },
                terrain: TerrainTrace {
                    delta_m: 0.0,
                    attenuation_bands: terrain_bands,
                    edges: Vec::new(),
                    delta_star_m: 0.0,
                },
                screening: ScreeningTrace {
                    attenuation_bands: screening_bands,
                    obstacle: None,
                },
                vegetation: VegetationTrace {
                    forest_depth_m: 0.0,
                    sampled_path_m: 0.0,
                    attenuation_bands: vegetation_bands,
                    forest_runs: Vec::new(),
                },
                ground: GroundTrace {
                    factor_g: ground_g,
                    attenuation_bands: ground_bands,
                },
                lw_bands,
                lw_db_a,
                received_bands,
            })),
            received_lden: LdenVariants {
                full: periods.lden_db,
                free_field: lden_free,
                no_terrain: lden_no_terrain,
                no_screening: lden_no_screening,
                no_vegetation: lden_no_vegetation,
                no_ground: lden_no_ground,
                no_atmospheric: lden_no_atmospheric,
            },
            aircraft_subtype: 1,
            polyline: Some(vec![
                (acc.start_lat, acc.start_lon),
                (acc.end_lat, acc.end_lon),
            ]),
            hex_polygon: None,
            cruise_buckets: None,
            cruise_top_flights: None,
            length_m_per_kind: None,
        });
    }
}

//! Shared cell-local cruise geometry and density-weighted Doc 29 scatter.

use std::collections::HashMap;

use crate::compute::aircraft_v6::dates::{date_from_unix, time_from_unix};
use crate::compute::aircraft_v6::state::{
    BandStats, CruiseFlightStats, FlightAccum, TopFlightCandidate,
};
use crate::compute::aircraft_v6::views::CruiseRowView;
use crate::emission::aircraft;
use crate::flight_id::pack_synth;
use crate::propagation::iso9613::fast_exp_f64;
use crate::types::{
    AircraftSegment, CruiseBucketBreakdown, CruiseCellTopFlight, RasterSampler, Receiver,
    TraceCollector,
};

/// Slant floor (m) — clamps the receiver-overhead degenerate case so
/// `1 / d²` doesn't blow up when the cruise rep-line passes directly
/// above the receiver. 5 m matches Doc 29 §A.2 minimum non-zero CPA.
pub const SLANT_FLOOR_M: f64 = 5.0;

/// Eight axial directions retain crossing tracks without duplicating reverse travel.
pub const CRUISE_HEADING_BINS: u8 = 8;

/// Nearest undirected track bearing: eight bins over 180 degrees, east = zero.
pub fn cruise_heading_bin(start_lat: f64, start_lon: f64, end_lat: f64, end_lon: f64) -> u8 {
    let dx = grid::geo::wrapped_longitude_delta(start_lon, end_lon)
        * ((start_lat + end_lat) * 0.5).to_radians().cos().max(0.2);
    let angle = (end_lat - start_lat)
        .atan2(dx)
        .rem_euclid(std::f64::consts::PI);
    ((angle * f64::from(CRUISE_HEADING_BINS) / std::f64::consts::PI).round() as u8)
        % CRUISE_HEADING_BINS
}

/// Cell-local segment offsets and length in the same projection used by Doc 29.
/// Its diagonal length also normalizes traffic density; source chord length does not.
pub fn cruise_geometry(lat: f64, heading_bin: u8) -> (f64, f64, f64) {
    assert!(
        heading_bin < CRUISE_HEADING_BINS,
        "cruise heading_bin must be in 0..8"
    );
    let cos_lat = lat.to_radians().cos();
    let length_m = aircraft::CRUISE_CELL_DIAGONAL_EQUATOR_M * cos_lat;
    let angle = f64::from(heading_bin) * std::f64::consts::PI / f64::from(CRUISE_HEADING_BINS);
    let (sin, cos) = angle.sin_cos();
    (
        length_m * 0.5 * sin / aircraft::M_PER_DEG_LAT,
        length_m * 0.5 * cos / (aircraft::M_PER_DEG_LAT * cos_lat.max(0.2)),
        length_m,
    )
}

/// Build the representative segment and its fractional traffic weight once per bucket.
/// Receiver admission and terrain checks remain caller-owned; geometry and density do not.
/// The density carries the bucket's provenance weight, so a secondary-only
/// bucket divides by the increment days like every other consumer.
pub fn cruise_segment(
    row: &CruiseRowView<'_>,
    index: usize,
    weights: &aircraft::ProvenanceWeights,
) -> Option<(AircraftSegment, f64)> {
    let (lat, lon) = (row.lat, row.lon);
    if !lat.is_finite() || !lon.is_finite() {
        return None;
    }
    let (lat_off, lon_off, length_m) = cruise_geometry(lat, row.heading_bin);
    let density = f64::from(row.sum_length_m) / length_m
        * weights.for_secondary_only(row.secondary_only);
    if !density.is_finite() || density <= 0.0 {
        return None;
    }
    let synth_fid = pack_synth(index as u64);
    let seg = AircraftSegment {
        flight_id: synth_fid,
        profile_idx: row.rep_profile_idx,
        // Doc 29 §A.3.2 — cruise NPD curves are taken from the
        // departure family (en-route is closest to climb-out).
        is_departure: true,
        on_ground: false,
        period: row.period,
        date_id: 0,
        start_lat: lat - lat_off,
        start_lon: grid::geo::normalize_longitude(lon - lon_off),
        start_alt_m: row.rep_alt_m,
        end_lat: lat + lat_off,
        end_lon: grid::geo::normalize_longitude(lon + lon_off),
        end_alt_m: row.rep_alt_m,
        speed_kt: row.rep_speed_kt,
        segment_length_m: length_m as f32,
        count_weight: density as f32,
        surface_model: false,
        ground_context: aircraft::GROUND_CONTEXT_NONE,
        ground_ops_kind: aircraft::GROUND_OPS_KIND_NONE,
        source_id: row.source_id as u16,
    };
    Some((seg, density))
}

/// Per-cell top-flight tracker. Same fid can appear in multiple
/// Stage 2B buckets within the same grid cell (e.g. crossing an FL boundary
/// mid-cell), so dedup via HashMap with "max peak_lmax wins" merge — a
/// Vec would inflate top-5 with duplicates of the loudest fid.
struct CellTopFlight {
    peak_lmax: f64,
    altitude_m: f64,
    class_idx: u8,
    aircraft_type: [u8; 4],
    callsign: String,
}

struct CellAccum {
    n_unique_flights: std::collections::HashSet<u64>,
    rep_alt_m: f32,
    centroid_lat: f64,
    centroid_lon: f64,
    d_slant_m: f64,
    period_energy: [f64; 3],
    buckets: Vec<CruiseBucketBreakdown>,
    top_fids: HashMap<u64, CellTopFlight>,
}

pub fn scatter(
    receiver: &Receiver,
    rows: &[CruiseRowView<'_>],
    rasters: &dyn RasterSampler,
    n_days_f: f64,
    weights: &aircraft::ProvenanceWeights,
    flights: &mut HashMap<u64, FlightAccum>,
    cruise_flight_stats: &mut HashMap<u64, CruiseFlightStats>,
    top_flight_candidates: &mut HashMap<u64, TopFlightCandidate>,
    traces: Option<&mut TraceCollector>,
) {
    let rx_elev = receiver.altitude_m();
    let npd_luts = aircraft::NpdLuts::shared();
    // Trace aggregates keyed by the (i32, i32) z30 cell pair of the
    // bucket centroid via the grid crate.
    let mut cell_accums: HashMap<(i32, i32), CellAccum> = HashMap::new();

    // The cell-local segment extends at most half a cell diagonal from its centre.
    let m_per_lat = crate::constants::M_PER_DEG_LAT;
    let m_per_lon = crate::constants::m_per_deg_lon(receiver.lat.to_radians());

    for (idx, row) in rows.iter().enumerate() {
        // Explicit centroid straight off the row — no cell-id lookup.
        let (lat, lon) = (row.lat, row.lon);
        if !lat.is_finite() || !lon.is_finite() {
            continue;
        }
        let half_len_m = aircraft::CRUISE_CELL_DIAGONAL_EQUATOR_M * lat.to_radians().cos() * 0.5;

        // Wrap longitude before the cheap centre-distance gate at the dateline.
        let dlat_m = (lat - receiver.lat) * m_per_lat;
        let mut dlon = lon - receiver.lon;
        if dlon > 180.0 {
            dlon -= 360.0;
        } else if dlon < -180.0 {
            dlon += 360.0;
        }
        let dlon_m = dlon * m_per_lon;
        let dist2_m2 = dlat_m * dlat_m + dlon_m * dlon_m;
        let cap_m = aircraft::AIRCRAFT_MAX_HORIZONTAL_REACH_M + half_len_m;
        if dist2_m2 > cap_m * cap_m {
            continue;
        }

        let Some((seg, density)) = cruise_segment(row, idx, weights) else {
            continue;
        };
        let synth_fid = seg.flight_id;
        // Terrain first cost five DEM probes — each through the tile
        // cache's per-tile lock — for buckets the kernel then dropped on
        // distance alone. The kernel's own first gate is purely geometric,
        // so run it here, before the rasters are touched. Measured at
        // Dobříš: 7 611 of 9 622 buckets that clear the centroid
        // prefilter die here. Bit-identical arithmetic, so no bucket that
        // used to contribute stops contributing (see `within_kernel_reach`).
        if !aircraft::within_kernel_reach(&seg, receiver.lat, receiver.lon, rx_elev) {
            continue;
        }
        let terrain = aircraft::SegmentTerrain::sample(&seg, rasters);
        if !aircraft::is_valid_airborne_with_terrain(&seg, &terrain) {
            continue;
        }
        // C2 horizon screening: cruise never screens — `with_terrain`
        // hard-wires `horizon = None` (structural exemption: cruise AGL
        // floor 7 200 m + 16 km slant cap ⇒ β ≥ 26.6° > any horizon).
        let Some((sel, cpa)) = aircraft::segment_sel_with_terrain(
            &seg,
            receiver.lat,
            receiver.lon,
            rx_elev,
            &terrain,
            npd_luts,
        ) else {
            continue;
        };
        let energy = fast_exp_f64(sel * std::f64::consts::LN_10 * 0.1) * density;
        let period = (row.period.min(2)) as usize;
        let acc = flights.entry(synth_fid).or_insert_with(|| {
            // Cruise rows have no per-flight callsign / typecode (one
            // grid-cell bucket aggregates many flights), so leave both empty.
            FlightAccum::new(row.rep_profile_idx, density, true, [0; 4], String::new())
        });
        acc.period_energy[period] += energy;
        // Cruise is structurally above the terrain/building screening
        // envelope, so all popup variants are the same received energy.
        acc.free_period_energy[period] += energy;
        acc.no_terrain_period_energy[period] += energy;
        acc.no_screening_period_energy[period] += energy;
        acc.flight_weight = acc.flight_weight.max(density);

        let class_idx = aircraft::noise_class_of(seg.profile_idx) as usize;
        // Clamp the DISPLAY CPA onto the observed segment so an off-segment
        // infinite-line foot can't report a phantom near pass. Cruise synthetic
        // segments are level (start == end == rep_alt_m), so the altitude clamp
        // is a no-op (sdz = 0) today — but both distance and altitude go through
        // the helper to stay symmetric with airborne and stay correct if a
        // gradient is ever modelled. `sel` / `energy` keep the unclamped `cpa`
        // (ΔF needs the infinite-line `q_m`).
        let (disp_dist, disp_alt) = aircraft::clamped_display_cpa(&cpa, 0.0);
        let log_d = (disp_dist * aircraft::FT_PER_M).max(100.0).log10();
        let thrust = aircraft::thrust_input_for_segment(
            &seg,
            seg.start_alt_m as f64,
            seg.end_alt_m as f64,
            terrain.start_elev - 30.0,
            terrain.end_elev - 30.0,
        );
        let (power_row, power_w) =
            aircraft::power_bracket(aircraft::thrust_model_for_class(class_idx), &thrust);
        // Cruise rep-segments are level, so the heli descent gate never fires.
        let lmax = npd_luts.lookup_lmax(class_idx, true, power_row, power_w, log_d)
            + aircraft::heli_correction_db(seg.profile_idx, true, 0.0);
        if lmax > acc.peak_lmax {
            acc.peak_lmax = lmax;
            acc.peak_sel = sel;
            acc.peak_altitude_m = disp_alt;
            acc.peak_period = row.period;
            acc.peak_seg_start = [seg.start_lon, seg.start_lat];
            acc.peak_seg_end = [seg.end_lon, seg.end_lat];
        }
        if disp_dist < acc.min_dist_m {
            acc.min_dist_m = disp_dist;
        }
        // v14: walk the row's bounded top-K candidate slice instead of
        // per-fid lists. Identity (typecode / callsign / fid) comes
        // from the candidate; receiver-side ranking (`lmax`, `disp_alt`)
        // comes from the row's already-computed values per Codex W2 —
        // re-deriving Lmax from candidate's
        // source-side peak would discard the popup-receiver geometry.
        for cand_view in row.top_candidates.iter() {
            let fid = cand_view.flight_id;
            let row_weight = weights.for_secondary_only(row.secondary_only);
            let entry = cruise_flight_stats.entry(fid).or_insert(CruiseFlightStats {
                peak_lmax: f64::NEG_INFINITY,
                alt_at_peak: 0.0,
                class_at_peak: class_idx,
                weight: row_weight,
            });
            entry.weight = entry.weight.min(row_weight);
            if lmax > entry.peak_lmax {
                entry.peak_lmax = lmax;
                entry.alt_at_peak = disp_alt;
                entry.class_at_peak = class_idx;
            }

            let cand = top_flight_candidates
                .entry(fid)
                .or_insert(TopFlightCandidate {
                    peak_lmax: f64::NEG_INFINITY,
                    peak_altitude_m: 0.0,
                    peak_period: row.period,
                    peak_seg_start: [0.0; 2],
                    peak_seg_end: [0.0; 2],
                    min_dist_m: f64::MAX,
                    profile_idx: row.rep_profile_idx,
                    aircraft_type: *cand_view.aircraft_type,
                    callsign: cand_view.callsign.to_string(),
                });
            if lmax > cand.peak_lmax {
                cand.peak_lmax = lmax;
                cand.peak_altitude_m = disp_alt;
                cand.peak_period = row.period;
                cand.peak_seg_start = [seg.start_lon, seg.start_lat];
                cand.peak_seg_end = [seg.end_lon, seg.end_lat];
            }
            if disp_dist < cand.min_dist_m {
                cand.min_dist_m = disp_dist;
            }
        }

        if traces.is_some() {
            // Per-bucket received_lden uses the row's energy density; the
            // popup tab only renders relative ordering inside one cell,
            // so a stable proxy (received_lden ≈ SEL on a per-event basis)
            // is enough.
            let received_lden = sel + 10.0 * density.max(1e-9).log10();
            let cell_key = grid::lonlat_to_grid(lon, lat);
            let entry = cell_accums.entry(cell_key).or_insert(CellAccum {
                n_unique_flights: std::collections::HashSet::new(),
                rep_alt_m: row.rep_alt_m,
                centroid_lat: lat,
                centroid_lon: lon,
                d_slant_m: disp_dist.max(SLANT_FLOOR_M),
                period_energy: [0.0; 3],
                buckets: Vec::new(),
                top_fids: HashMap::new(),
            });
            for cand_view in row.top_candidates.iter() {
                entry.n_unique_flights.insert(cand_view.flight_id);
                let cand = entry
                    .top_fids
                    .entry(cand_view.flight_id)
                    .or_insert(CellTopFlight {
                        peak_lmax: f64::NEG_INFINITY,
                        altitude_m: 0.0,
                        class_idx: class_idx as u8,
                        aircraft_type: *cand_view.aircraft_type,
                        callsign: cand_view.callsign.to_string(),
                    });
                if lmax > cand.peak_lmax {
                    cand.peak_lmax = lmax;
                    cand.altitude_m = disp_alt;
                    cand.class_idx = class_idx as u8;
                }
            }
            entry.period_energy[period] += energy;
            if disp_dist < entry.d_slant_m {
                entry.d_slant_m = disp_dist.max(SLANT_FLOOR_M);
            }
            entry.buckets.push(CruiseBucketBreakdown {
                class: row.class,
                fl_bin: row.fl_bin,
                period: row.period,
                // v14: `unique_count` is the full bucket count (not
                // just top-K) — display semantics match v13.
                n_flights: row.unique_count,
                received_lden,
            });
        }
    }

    if let Some(t) = traces {
        let mut cell_keys: Vec<(i32, i32)> = cell_accums.keys().copied().collect();
        cell_keys.sort();
        for cell_key in cell_keys {
            let mut acc = cell_accums
                .remove(&cell_key)
                .expect("sorted key from live map");
            acc.buckets.sort_by(|a, b| {
                b.received_lden
                    .partial_cmp(&a.received_lden)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let cruise_top_flights = top_flights_for_cell(&acc.top_fids);
            // Cell aggregate placeholder: no individual kernel breakdown is retained.
            let placeholder_doc29 = crate::types::Doc29Breakdown {
                sel_npd_db: 0.0,
                delta_v_db: 0.0,
                delta_i_db: 0.0,
                lambda_db: 0.0,
                delta_f_db: 0.0,
                d_p_m: acc.d_slant_m,
                lateral_m: 0.0,
                beta_deg: 90.0,
                seg_len_m: 0.0,
                d_lambda_m: acc.d_slant_m,
                installation: "wing",
                screening_kind: "none",
                screening_db: 0.0,
            };
            t.segments
                .push(crate::traces::build_aircraft_cruise_cell_trace(
                    crate::traces::BuildAircraftCruiseCellTrace {
                        lon: acc.centroid_lon,
                        lat: acc.centroid_lat,
                        n_unique_flights: acc.n_unique_flights.len() as u32,
                        rep_alt_m: acc.rep_alt_m,
                        d_slant_m: acc.d_slant_m,
                        period_energies: acc.period_energy,
                        n_days: n_days_f,
                        cruise_buckets: acc.buckets,
                        cruise_top_flights,
                        doc29: placeholder_doc29,
                    },
                ));
        }
    }
}

const TOP_FLIGHTS_PER_CELL: usize = 5;

fn top_flights_for_cell(top_fids: &HashMap<u64, CellTopFlight>) -> Vec<CruiseCellTopFlight> {
    let mut entries: Vec<(u64, &CellTopFlight)> = top_fids.iter().map(|(f, t)| (*f, t)).collect();
    entries.sort_by(|a, b| {
        b.1.peak_lmax
            .partial_cmp(&a.1.peak_lmax)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    entries.truncate(TOP_FLIGHTS_PER_CELL);
    entries
        .into_iter()
        .map(|(fid, t)| {
            let (icao_hex, start_unix) = crate::flight_id::icao_hex_and_start_unix(fid);
            let date = start_unix.map(date_from_unix).unwrap_or_default();
            let time_utc = start_unix.map(time_from_unix).unwrap_or_default();
            CruiseCellTopFlight {
                lmax_db: round1(t.peak_lmax),
                altitude_m: round1(t.altitude_m),
                date,
                time_utc,
                icao_hex,
                aircraft_type: aircraft::typecode_to_string(&t.aircraft_type),
                callsign: t.callsign.clone(),
                class_name: aircraft::CLASS_NAMES[t.class_idx as usize].to_string(),
            }
        })
        .collect()
}

#[inline]
fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// Per-band cruise dedup → `[band_faint, band_audible, band_disruptive]`.
/// Each real fid contributes once per band it crosses (not once per grid-cell
/// bucket). v14: walks `cruise_flight_stats` which is keyed on real fid
/// and populated from each row's `top_candidates` slice — so a fid
/// touching multiple grid-cell buckets dedupes naturally via HashMap insert.
/// Tail fids outside the per-row top-K cap silently undercount; this is a
/// documented display-only regression.
pub fn band_stats(cruise_flight_stats: &HashMap<u64, CruiseFlightStats>) -> [BandStats; 3] {
    let mut out = [BandStats::new(), BandStats::new(), BandStats::new()];
    // Ascending fid: `add_event` sums `alt_sum` in f64, so HashMap order
    // would move the popup's `avg_altitude_m` by ±1 ULP per run.
    for (_, stats) in crate::compute::key_sorted(cruise_flight_stats) {
        if stats.peak_lmax > 30.0 {
            let cls = stats.class_at_peak;
            let (w, alt) = (stats.weight, stats.alt_at_peak * stats.weight);
            let class_w = w.round().max(1.0) as u32;
            out[0].add_event(w, alt, cls, class_w);
            if stats.peak_lmax > 45.0 {
                out[1].add_event(w, alt, cls, class_w);
                if stats.peak_lmax > 60.0 {
                    out[2].add_event(w, alt, cls, class_w);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(lat: f64, lon: f64, altitude: f32, heading_bin: u8) -> CruiseRowView<'static> {
        CruiseRowView {
            lat,
            lon,
            class: 5,
            rep_profile_idx: 0,
            fl_bin: 3,
            period: 0,
            sum_length_m: 250.0,
            heading_bin,
            rep_alt_m: altitude,
            rep_speed_kt: 450.0,
            source_id: 0,
            origin: 0,
            secondary_only: false,
            unique_count: 1,
            top_candidates: &[],
        }
    }

    #[test]
    fn axial_geometry_preserves_density_length_and_wrapped_direction() {
        for lat in [0.0_f64, 49.8, 68.0, 85.0] {
            for heading in 0..8 {
                let r = row(lat, 179.999, 11000.0, heading);
                let (segment, density) = cruise_segment(&r, 0, &aircraft::ProvenanceWeights::PRIMARY_ONLY).unwrap();
                let dx = grid::geo::wrapped_longitude_delta(segment.start_lon, segment.end_lon)
                    * aircraft::M_PER_DEG_LAT
                    * lat.to_radians().cos().max(0.2);
                let dy = (segment.end_lat - segment.start_lat) * aircraft::M_PER_DEG_LAT;
                let length = dx.hypot(dy);
                assert!((length * density - 250.0).abs() < 1e-7);
                assert_eq!(
                    cruise_heading_bin(
                        segment.start_lat,
                        segment.start_lon,
                        segment.end_lat,
                        segment.end_lon
                    ),
                    heading
                );
                assert_eq!(
                    cruise_heading_bin(
                        segment.end_lat,
                        segment.end_lon,
                        segment.start_lat,
                        segment.start_lon
                    ),
                    heading
                );
                if lat < 80.0 {
                    assert!(
                        density < 1.0,
                        "fractional traffic is not rounded to one flight"
                    );
                }
            }
        }
    }

    #[test]
    fn cell_local_heading_preserves_physical_trajectory_energy() {
        let (lat, lon) = (55.99_f64, -74.34);
        let mlat = aircraft::M_PER_DEG_LAT;
        let mlon = mlat * lat.to_radians().cos();
        let energy = |segment: &AircraftSegment, weight: f64| {
            aircraft::segment_sel_with_cuts(
                segment,
                lat,
                lon,
                281.0,
                247.0,
                247.0,
                aircraft::NpdLuts::shared(),
                None,
            )
            .map_or(0.0, |(sel, _)| 10.0_f64.powf(sel / 10.0) * weight)
        };
        let mut maximum_error = 0.0_f64;
        for altitude in [7500.0, 11000.0, 15000.0] {
            for degrees in [
                0.0_f64, 11.25, 22.5, 33.75, 45.0, 78.75, 90.0, 123.75, 135.0, 168.75,
            ] {
                let (sin, cos) = degrees.to_radians().sin_cos();
                let (mut original, _) = cruise_segment(&row(lat, lon, altitude, 0), 0, &aircraft::ProvenanceWeights::PRIMARY_ONLY).unwrap();
                original.start_lat = lat - 100000.0 * sin / mlat;
                original.end_lat = lat + 100000.0 * sin / mlat;
                original.start_lon = lon - 100000.0 * cos / mlon;
                original.end_lon = lon + 100000.0 * cos / mlon;
                original.segment_length_m = 200000.0;
                let reference = energy(&original, 1.0);
                let heading = cruise_heading_bin(
                    original.start_lat,
                    original.start_lon,
                    original.end_lat,
                    original.end_lon,
                );
                let sum: f64 = (0..800)
                    .map(|index| {
                        let distance = -100000.0 + (index as f64 + 0.5) * 250.0;
                        let r = row(
                            lat + distance * sin / mlat,
                            lon + distance * cos / mlon,
                            altitude,
                            heading,
                        );
                        let (segment, density) = cruise_segment(&r, index, &aircraft::ProvenanceWeights::PRIMARY_ONLY).unwrap();
                        energy(&segment, density)
                    })
                    .sum();
                let error = 10.0 * (sum / reference).log10();
                maximum_error = maximum_error.max(error.abs());
                assert!(
                    error.abs() < 0.1,
                    "height={altitude} heading={degrees} error={error}dB"
                );
            }
        }
        eprintln!("cruise heading: 30 physical trajectories; max_abs_error_db={maximum_error:.9}");
    }
}

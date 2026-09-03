//! `compute_aircraft_v6` — popup entry point that consumes the popup
//! aircraft arrows directly via typed column views (no Arrow / IPC
//! dependency in noise-compute).
//!
//! Architecture: airborne and cruise rows each scatter directly onto
//! their own per-row kernels:
//! * airborne: per-sub-segment Doc 29 SEL → `FlightAccum` per real fid
//! * cruise: per-bucket Doc 29 SEL × density → `FlightAccum` per synth fid
//!   + `CruiseFlightStats` per real fid for band counter dedup
//!
//! Ground operations live in the parallel `airport_traffic` compute
//! path invoked by source-reader after this function returns.

use std::collections::HashMap;

use crate::compute::aircraft_v6::state::{FlightAccum, TopFlightCandidate};
use crate::emission::aircraft::{BuildingHorizon, ReceiverHorizon};
use crate::types::{
    AircraftBandData, AircraftMetadata, Contributor, LayerKind, NoisePeriods, PropagationBaseline,
    RasterSampler, Receiver, ScreeningBreakdown, SourceMetadata, TerrainBreakdown, TraceCollector,
    VegetationBreakdown,
};

pub mod airborne;
pub mod airport_traffic;
pub mod cruise;
pub mod dates;
pub mod state;
pub mod views;

pub use views::{
    AirborneRowView, AirportTrafficRowView, BBox, CruiseRowView, CruiseTopCandidateView,
    SubSegmentSlice, NUM_GSE_CLASSES,
};

const NUM_BANDS: usize = 8;

/// Pure-view popup compute. Consumes typed slices borrowed from the v6
/// popup arrows; emits `(NoisePeriods, Vec<Contributor>, AircraftBandData)`
/// matching the legacy entry point's contract.
#[allow(clippy::too_many_arguments)]
pub fn compute_aircraft_v6(
    receiver: &Receiver,
    airborne_rows: &[AirborneRowView<'_>],
    cruise_rows: &[CruiseRowView<'_>],
    rasters: &dyn RasterSampler,
    // Production airborne screening, built once for this receiver. `None` is
    // valid only when `airborne_rows` is empty. Cruise is structurally exempt
    // (β ≥ 26.6°, see segment_sel).
    horizon: Option<&ReceiverHorizon>,
    buildings: Option<&BuildingHorizon>,
    n_days: u16,
    // GA hybrid per-class weight LUT, built from the arrows'
    // `sample_days_by_class` metadata by the caller.
    // Threads into the airborne scatter; cruise is airline-only (no GA
    // classes reach cruise altitude) so it ignores this.
    class_weights: &crate::emission::aircraft::ClassWeights,
    // Max airborne sub-segment traces to keep in TraceCollector (the
    // bounded top-K heap). `0` = don't allocate any traces — used by
    // callers that pass `traces = None` anyway.
    trace_cap: usize,
    traces: Option<&mut TraceCollector>,
    timings: Option<&mut crate::types::LayerTimings>,
) -> (NoisePeriods, Vec<Contributor>, AircraftBandData) {
    let n_days_f = (n_days as f64).max(1.0);

    // Per-layer timing probes. The print is env-gated (POPUP_TIMING=1);
    // the 4 Instant::now()/elapsed() calls run unconditionally but cost
    // <1 µs total per popup. Inline timing > perf/flamegraph for this
    // app: one log line per popup, no perf.data on disk.
    let timing_on = std::env::var("POPUP_TIMING").as_deref() == Ok("1");
    let t_start = std::time::Instant::now();

    let mut traces = traces;
    let flights = if airborne_rows.is_empty() {
        HashMap::new()
    } else {
        let horizon = horizon.expect("non-empty airborne rows require a receiver terrain horizon");
        airborne::scatter(
            receiver,
            airborne_rows,
            n_days_f,
            class_weights,
            horizon,
            buildings,
            trace_cap,
            traces.as_deref_mut(),
        )
    };
    let t_airborne_scatter = t_start.elapsed();
    let mut cruise_flight_stats = HashMap::new();
    // Cruise gets its own FlightAccum table — the cruise synth fids
    // (`flight_id::pack_synth(idx)` with idx = row index) share the
    // SYNTHETIC_BIT tagging used by airborne TIS-B / anonymous flights
    // at extract time. An airborne synth fid can collide with a cruise
    // idx, and the merged accumulator (now
    // tagged `is_cruise = true`) silently swallows the airborne energy
    // inside `build_detail`'s `if acc.is_cruise { continue }` branch.
    // Keeping the maps disjoint makes the namespaces structurally
    // independent. Cruise contributions to airborne periods come from
    // accumulating their `period_energy` into `airborne_energy`; cruise
    // band counters come from `cruise_flight_stats` (real fid dedup).
    let mut cruise_flights: HashMap<u64, FlightAccum> = HashMap::new();
    let mut top_flight_candidates: HashMap<u64, TopFlightCandidate> = HashMap::new();
    cruise::scatter(
        receiver,
        cruise_rows,
        rasters,
        n_days_f,
        &mut cruise_flights,
        &mut cruise_flight_stats,
        &mut top_flight_candidates,
        traces,
    );
    let t_cruise_scatter = t_start.elapsed() - t_airborne_scatter;

    let cruise_band = cruise::band_stats(&cruise_flight_stats);
    let (airborne_periods, airborne_periods_free, airborne_impacts, airborne_detail) =
        airborne::build_detail(
            &flights,
            &cruise_flights,
            cruise_flight_stats.len(),
            &top_flight_candidates,
            &cruise_band,
            n_days_f,
            (class_weights.ga_n_days() as f64).max(1.0),
        );
    let t_airborne_detail = t_start.elapsed() - t_airborne_scatter - t_cruise_scatter;

    let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
    if timing_on {
        let t_total = t_start.elapsed();
        eprintln!(
            "ac-v6 total={:.0}ms airb_scatter={:.0}ms cr_scatter={:.0}ms airb_detail={:.0}ms (n_airb={} n_cr={})",
            ms(t_total),
            ms(t_airborne_scatter),
            ms(t_cruise_scatter),
            ms(t_airborne_detail),
            airborne_rows.len(),
            cruise_rows.len(),
        );
    }
    if let Some(t) = timings {
        // `airb_detail` is a few ms of post-processing — fold into the
        // airborne bucket so the popup breakdown stays 3 aircraft buckets.
        t.aircraft_airborne_ms = ms(t_airborne_scatter) + ms(t_airborne_detail);
        t.aircraft_cruise_ms = ms(t_cruise_scatter);
    }

    let mut contributors: Vec<Contributor> = Vec::new();
    if airborne_periods.lden_db.is_finite() {
        contributors.push(Contributor {
            osm_id: None,
            geometry: None,
            baseline: PropagationBaseline::default(),
            terrain: TerrainBreakdown::default(),
            screening: ScreeningBreakdown::default(),
            vegetation: VegetationBreakdown::default(),
            terrain_impact_db: airborne_impacts.terrain,
            screening_impact_db: airborne_impacts.screening,
            vegetation_impact_db: 0.0,
            atmospheric_impact_db: 0.0,
            ground_impact_db: 0.0,
            source_type: LayerKind::Aircraft,
            name: "Aircraft - airborne".to_string(),
            subtype: "airborne".to_string(),
            distance_m: 0.0,
            periods: airborne_periods.clone(),
            // Pre-screen Doc 29 energy is retained by the popup accumulator;
            // the painter remains received-only for the GPU/CPU tile path.
            periods_free: airborne_periods_free.clone(),
            emission_db: airborne_periods.lden_db,
            received_bands: [0.0; NUM_BANDS],
            metadata: Some(SourceMetadata::Aircraft(Box::new(AircraftMetadata {
                variant: "airborne".to_string(),
                airport_name: None,
                airport_key: None,
                airborne: Some(airborne_detail.clone()),
                ground_ops: None,
            }))),
        });
    }

    let band_data = AircraftBandData {
        airborne: airborne_detail,
        ground_ops: Default::default(),
    };

    (airborne_periods, contributors, band_data)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FlatGround;
    impl RasterSampler for FlatGround {
        fn elevation(&self, _: f64, _: f64) -> f64 {
            0.0
        }
        fn ground_g(&self, _: f64, _: f64) -> f64 {
            1.0
        }
        fn building_enclosure(&self, _: f64, _: f64) -> f64 {
            0.0
        }
    }

    #[test]
    fn silence_when_no_data() {
        let receiver = Receiver::new(50.10, 14.262, 0.0);
        let w = crate::emission::aircraft::ClassWeights::uniform();
        let (periods, contribs, _band) = compute_aircraft_v6(
            &receiver,
            &[],
            &[],
            &FlatGround,
            None,
            None,
            1,
            &w,
            0,
            None,
            None,
        );
        assert!(!periods.lden_db.is_finite());
        assert!(contribs.is_empty());
    }

    /// The popup is the project's acoustic reference, so one click must
    /// return bit-identical numbers every time. It did not: `RandomState`
    /// re-seeds on every `HashMap::new()` — i.e. on every query, not merely
    /// every process — and the aircraft kernel sums f64 energies across
    /// whole accumulator maps, so the iteration order moved `total_lden` by
    /// ±1 ULP (measured 2026-08-05: three distinct values in four runs of
    /// the same Praha click). This test runs one synthetic click ten times
    /// in one process and demands identical bytes.
    ///
    /// It needs MANY flights to be a real test: with a handful of keys the
    /// hash order can repeat by luck. 300 airborne flights + 300 cruise
    /// buckets each land in their own bucket table, so a `.values()` walk
    /// anywhere in the chain reorders essentially every run.
    ///
    /// Layer coverage: this pins the aircraft path, which is where the
    /// defect was measured. `compute::roads` / `railways` / `point_sources`
    /// carry the same `crate::compute::key_sorted` contract on their own
    /// energy totals; `scripts/check-popup-determinism.mjs` covers all of
    /// them end to end against real prepared data.
    #[test]
    fn repeated_identical_clicks_are_bit_identical() {
        use crate::compute::aircraft_v6::views::{BBox, CruiseTopCandidateView, SubSegmentSlice};

        const N_FLIGHTS: usize = 300;
        let receiver = Receiver::new(50.0, 14.0, 300.0);
        let w = crate::emission::aircraft::ClassWeights::uniform();
        let horizon = ReceiverHorizon::build(
            |_, _| 300.0,
            receiver.lat,
            receiver.lon,
            receiver.altitude_m(),
        );

        // Airborne: one two-sub-segment flight per row. The flights must
        // land at COMPARABLE energies with differing low bits — a wide
        // spread would be order-independent for the opposite reason (a
        // term below `max * 2^-53` is a no-op wherever it is added). So
        // the tracks are jittered inside a ~1 km box a couple of km from
        // the receiver rather than fanned across the whole reach.
        let jitter = |i: usize, salt: u64| -> f32 {
            ((i as u64).wrapping_mul(2_654_435_761).wrapping_add(salt) % 997) as f32 * 1.0e-5
        };
        let mut sub_store = Vec::with_capacity(N_FLIGHTS);
        for i in 0..N_FLIGHTS {
            let off = 0.018 + jitter(i, 11);
            sub_store.push((
                vec![49.98 + off, 49.99 + off],
                vec![13.98 + off, 13.99 + off],
                vec![
                    900.0 + jitter(i, 23) * 9_000.0,
                    950.0 + jitter(i, 29) * 9_000.0,
                ],
                vec![49.99 + off, 50.01 + off],
                vec![13.99 + off, 14.01 + off],
                vec![
                    950.0 + jitter(i, 31) * 9_000.0,
                    1000.0 + jitter(i, 37) * 9_000.0,
                ],
                vec![220.0f32, 220.0],
                vec![1500.0f32, 1500.0],
                vec![(i % 3) as u8, ((i + 1) % 3) as u8],
                vec![10i16, 10],
                vec![1u8, 1],
                vec![300.0f32, 300.0],
                vec![300.0f32, 300.0],
            ));
        }
        let callsigns: Vec<String> = (0..N_FLIGHTS).map(|i| format!("CSA{i:04}")).collect();
        let airborne: Vec<AirborneRowView<'_>> = (0..N_FLIGHTS)
            .map(|i| {
                let s = &sub_store[i];
                AirborneRowView {
                    // Real (non-synthetic) fids carrying a start_unix, so
                    // `energy_by_day` and `top_flights` both populate.
                    flight_id: crate::flight_id::pack_real(
                        0x40_0000 + i as u32,
                        1_750_000_000 + (i as u32 % 7) * 86_400,
                    )
                    .expect("test fid"),
                    callsign: callsigns[i].as_str(),
                    aircraft_type: *b"A320",
                    profile_idx: (i % 8) as u8,
                    source_id: 0,
                    origin: 0,
                    sub_segments: SubSegmentSlice {
                        start_lat: &s.0,
                        start_lon: &s.1,
                        start_alt_m: &s.2,
                        end_lat: &s.3,
                        end_lon: &s.4,
                        end_alt_m: &s.5,
                        speed_kt: &s.6,
                        length_m: &s.7,
                        period: &s.8,
                        date_id: &s.9,
                        flags: &s.10,
                        terrain_start_elev_m: &s.11,
                        terrain_end_elev_m: &s.12,
                    },
                    bbox: BBox {
                        min_lat: 49.9,
                        max_lat: 50.1,
                        min_lon: 13.9,
                        max_lon: 14.1,
                    },
                }
            })
            .collect();

        // Cruise: R7 buckets around the receiver, each with its own
        // top-candidate identity so `cruise_flight_stats` and
        // `top_flight_candidates` both fill up.
        let r7_cells: Vec<u64> = {
            let origin = h3o::LatLng::new(50.0, 14.0)
                .unwrap()
                .to_cell(h3o::Resolution::Seven);
            origin
                .grid_disk::<Vec<_>>(6)
                .into_iter()
                .map(u64::from)
                .collect()
        };
        let cruise_cs: Vec<String> = (0..r7_cells.len()).map(|i| format!("DLH{i:04}")).collect();
        let cand_store: Vec<Vec<CruiseTopCandidateView<'_>>> = (0..r7_cells.len())
            .map(|i| {
                vec![CruiseTopCandidateView {
                    flight_id: crate::flight_id::pack_real(
                        0x50_0000 + i as u32,
                        1_750_000_000 + (i as u32 % 5) * 86_400,
                    )
                    .expect("test fid"),
                    callsign: cruise_cs[i].as_str(),
                    aircraft_type: b"B738",
                    peak_lmax_25m_db: 90.0 + (i % 11) as f32,
                    altitude_m: 10_000.0,
                }]
            })
            .collect();
        let cruise: Vec<CruiseRowView<'_>> = r7_cells
            .iter()
            .enumerate()
            .map(|(i, &hex)| CruiseRowView {
                r7_hex: hex,
                class: 0,
                rep_profile_idx: (i % 8) as u8,
                fl_bin: 3,
                period: (i % 3) as u8,
                sum_length_m: 3_000.0 + jitter(i, 41) * 40_000.0,
                rep_len_m: 40_000.0 + jitter(i, 43) * 90_000.0,
                rep_alt_m: 9_000.0 + jitter(i, 47) * 300_000.0,
                rep_speed_kt: 450.0,
                source_id: 0,
                origin: 0,
                unique_count: 3,
                top_candidates: &cand_store[i],
            })
            .collect();

        let run = || {
            let (periods, _contribs, band) = compute_aircraft_v6(
                &receiver,
                &airborne,
                &cruise,
                &FlatGround,
                Some(&horizon),
                None,
                7,
                &w,
                0,
                None,
                None,
            );
            // JSON round-trips f64 as shortest-roundtrip decimal, which is
            // injective on f64 — equal strings mean equal bits.
            serde_json::to_string(&(&periods, &band.airborne)).unwrap()
        };

        let first = run();
        // Guard against a vacuous pass: the click has to actually produce
        // aircraft energy and a populated top-flights table.
        assert!(
            first.contains("\"top_flights\":[{"),
            "synthetic click produced no top flights — test would be vacuous: {first:.400}"
        );
        for i in 1..10 {
            let again = run();
            if again != first {
                // Point at the divergence instead of dumping two 8 kB
                // JSON blobs — the interesting part is one field's digits.
                let at = first
                    .bytes()
                    .zip(again.bytes())
                    .position(|(a, b)| a != b)
                    .unwrap_or(first.len().min(again.len()));
                let from = at.saturating_sub(70);
                panic!(
                    "popup aircraft output changed on repeat {i} of the SAME click — \
                     something sums f64 (or picks a max) over a HashMap walk again; \
                     see crate::compute::key_sorted.\n  first byte {at} differs\n  \
                     run 0: …{}\n  run {i}: …{}",
                    &first[from..(at + 30).min(first.len())],
                    &again[from..(at + 30).min(again.len())],
                );
            }
        }
    }
}

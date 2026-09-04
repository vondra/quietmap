use super::*;
use crate::flight_id;

/// `rank_key = energy * AIRBORNE_RANK_W[period]` must produce the same
/// ordering as the actual `received_lden.full` Lden computed by the
/// trace builder. If this drifts (e.g. someone changes Lden weights
/// without updating AIRBORNE_RANK_W), the heap will drop the wrong
/// sub-segments — Lden parity holds (heap doesn't gate energy) but
/// the popup "top-K segments" tab shows misranked rows.
#[test]
fn rank_key_ordering_matches_received_lden_full() {
    use crate::periods::compute_lden;

    // 12 mixed-period mixed-energy cases. Period 0 = day (no penalty),
    // 1 = evening (+5 dB), 2 = night (+10 dB).
    let cases: Vec<(usize, f64)> = vec![
        (0, 1e-6),
        (0, 1e-4),
        (0, 1e-2),
        (0, 1.0),
        (1, 1e-6),
        (1, 1e-4),
        (1, 1e-2),
        (1, 1.0),
        (2, 1e-6),
        (2, 1e-4),
        (2, 1e-2),
        (2, 1.0),
    ];
    let n_days = 1.0;

    let scored: Vec<(f64, f64)> = cases
        .iter()
        .map(|&(period, energy)| {
            let rank_key = energy * AIRBORNE_RANK_W[period];
            // Mirror traces/aircraft.rs:aircraft_period_variants:
            // each period's input is energy / (n_days * PERIOD_SECONDS[i]).
            let mut normed = [0.0_f64; 3];
            normed[period] = energy / (n_days * aircraft::PERIOD_SECONDS[period]);
            let lden = compute_lden(normed[0], normed[1], normed[2]);
            (rank_key, lden)
        })
        .collect();

    // Sort ascending by each key; the two orders must match index-for-index.
    let mut by_key: Vec<usize> = (0..scored.len()).collect();
    by_key.sort_by(|&a, &b| scored[a].0.total_cmp(&scored[b].0));
    let mut by_lden: Vec<usize> = (0..scored.len()).collect();
    by_lden.sort_by(|&a, &b| scored[a].1.total_cmp(&scored[b].1));

    assert_eq!(
        by_key, by_lden,
        "rank_key order diverges from received_lden.full order: \
             rank_key = energy * AIRBORNE_RANK_W[period] is no longer \
             monotone with the Lden formula in periods::compute_lden. \
             Update AIRBORNE_RANK_W to match the new weights."
    );
}

/// Sanity: the cutoff sits well below the empirical popup top-K
/// rank floor (~40 dB Lden ≈ ~50 dB Lmax at LKPR). Bumping this
/// above 35 dB risks dropping segments the user might rank in.
/// If a future ranking change moves the floor, fail loudly here
/// rather than silently culling segments.
// assertions_on_constants: deliberate — these guard a hand-tuned calibration
// constant and carry an operator-facing message (which a `const {}` block can't),
// so they stay runtime test assertions that fail loudly if the constant drifts.
#[allow(clippy::assertions_on_constants)]
#[test]
fn trace_cutoff_constant_safely_below_lkpr_rank_floor() {
    assert!(
        AIRBORNE_TRACE_CUTOFF_DB <= 35.0,
        "AIRBORNE_TRACE_CUTOFF_DB ({}) crept above the empirical LKPR top-150 \
             rank floor (~50 dB Lmax). Drop the constant or recalibrate against \
             a fresh LKPR popup baseline.",
        AIRBORNE_TRACE_CUTOFF_DB
    );
    // Lower bound guard: a near-zero cutoff would defeat the optimisation.
    assert!(AIRBORNE_TRACE_CUTOFF_DB >= 10.0);
}

// The former `is_near_airport_*` tests were removed when the fn
// itself was deleted (2026-05-23). The carve-out it gated never
// fired in production — Stage 1 + Stage 2A already correctly
// classify low-AGL near-airport approaches; aircraft Lden delta
// when the popup-side stale filter is bypassed entirely:
// 0.000 dB across LKPR / Praha / Brdy / Šumava / 10 km W Praha.

fn make_flight(
    peak_lmax: f64,
    period_energy_total: f64,
    is_cruise: bool,
    typecode: [u8; 4],
    callsign: &str,
) -> FlightAccum {
    let mut acc = FlightAccum::new(0, 1.0, is_cruise, typecode, callsign.to_string());
    acc.peak_lmax = peak_lmax;
    acc.peak_sel = peak_lmax - 5.0;
    acc.period_energy[0] = period_energy_total;
    acc.free_period_energy = acc.period_energy;
    acc.no_terrain_period_energy = acc.period_energy;
    acc.no_screening_period_energy = acc.period_energy;
    acc.peak_altitude_m = 200.0;
    acc.min_dist_m = 500.0;
    acc.peak_seg_start = [14.26, 50.10];
    acc.peak_seg_end = [14.27, 50.11];
    acc
}

#[test]
fn build_top_flights_orders_descending_and_drops_silent() {
    let mut flights: HashMap<u64, FlightAccum> = HashMap::new();
    // Three loud, one silent (zero energy → dropped), one cruise (dropped).
    flights.insert(
        flight_id::pack_real(0xAAAAAA, 1_700_000_000).unwrap(),
        make_flight(70.0, 100.0, false, *b"B738", "TVS100P"),
    );
    flights.insert(
        flight_id::pack_real(0xBBBBBB, 1_700_000_001).unwrap(),
        make_flight(80.0, 200.0, false, *b"A320", "CSA1"),
    );
    flights.insert(
        flight_id::pack_real(0xCCCCCC, 1_700_000_002).unwrap(),
        make_flight(60.0, 50.0, false, *b"CRJ\0", "RYR1"),
    );
    flights.insert(
        flight_id::pack_real(0xDDDDDD, 1_700_000_003).unwrap(),
        make_flight(90.0, 0.0, false, *b"H25B", "EJM"), // silent
    );
    flights.insert(
        flight_id::pack_real(0xEEEEEE, 1_700_000_004).unwrap(),
        make_flight(95.0, 300.0, true, *b"B789", ""), // cruise
    );

    let cruise_cands: HashMap<u64, TopFlightCandidate> = HashMap::new();
    let out = build_top_flights(&crate::compute::key_sorted(&flights), &cruise_cands, 350.0);
    assert_eq!(out.len(), 3, "got {} top flights", out.len());
    assert_eq!(out[0].lmax_db, 80.0);
    assert_eq!(out[0].aircraft_type, "A320");
    assert_eq!(out[0].callsign, "CSA1");
    assert!(!out[0].synthetic);
    assert_eq!(out[0].icao_hex.len(), 6);
    // 200/350 = 57.1 % → round1 = 57.1
    assert!((out[0].energy_pct - 57.1).abs() < 0.05);
    assert_eq!(out[1].lmax_db, 70.0);
    assert_eq!(out[1].aircraft_type, "B738");
    assert_eq!(out[2].lmax_db, 60.0);
    assert_eq!(out[2].aircraft_type, "CRJ", "NUL pad must be trimmed");
}

#[test]
fn build_top_flights_caps_at_top_n() {
    let mut flights: HashMap<u64, FlightAccum> = HashMap::new();
    // 30 unique flights with descending peak_lmax; expect TOP_FLIGHTS_N kept.
    for i in 0..30u64 {
        let lmax = 100.0 - i as f64;
        flights.insert(
            flight_id::pack_real(0x100000 + i as u32, 1_700_000_000 + i as u32).unwrap(),
            make_flight(lmax, 10.0, false, *b"B738", "TVS"),
        );
    }
    let cruise_cands: HashMap<u64, TopFlightCandidate> = HashMap::new();
    let out = build_top_flights(&crate::compute::key_sorted(&flights), &cruise_cands, 300.0);
    assert_eq!(out.len(), TOP_FLIGHTS_N);
    // Loudest preserved.
    assert_eq!(out[0].lmax_db, 100.0);
    // Last kept = TOP_FLIGHTS_N - 1th loudest = 100 - 19 = 81.
    assert_eq!(out[TOP_FLIGHTS_N - 1].lmax_db, 81.0);
}

#[test]
fn build_top_flights_synth_fid_marks_synthetic() {
    let mut flights: HashMap<u64, FlightAccum> = HashMap::new();
    let synth_fid = flight_id::pack_synth(0x1234_5678);
    flights.insert(synth_fid, make_flight(75.0, 50.0, false, [0; 4], ""));
    let cruise_cands: HashMap<u64, TopFlightCandidate> = HashMap::new();
    let out = build_top_flights(&crate::compute::key_sorted(&flights), &cruise_cands, 50.0);
    assert_eq!(out.len(), 1);
    assert!(out[0].synthetic);
    assert!(out[0].icao_hex.is_empty());
    assert!(out[0].start_unix.is_none());
    assert!(out[0].aircraft_type.is_empty());
    assert!(out[0].callsign.is_empty());
}

fn make_cruise_cand(peak_lmax: f64, typecode: [u8; 4], callsign: &str) -> TopFlightCandidate {
    TopFlightCandidate {
        peak_lmax,
        peak_altitude_m: 9000.0,
        peak_period: 0,
        peak_seg_start: [14.4, 50.0],
        peak_seg_end: [14.5, 50.0],
        min_dist_m: 9100.0,
        profile_idx: 0,
        aircraft_type: typecode,
        callsign: callsign.to_string(),
    }
}

#[test]
fn build_top_flights_interleaves_airborne_and_cruise() {
    let mut flights: HashMap<u64, FlightAccum> = HashMap::new();
    flights.insert(
        flight_id::pack_real(0xA1, 1_700_000_000).unwrap(),
        make_flight(80.0, 100.0, false, *b"A320", "AIR1"),
    );
    flights.insert(
        flight_id::pack_real(0xA2, 1_700_000_001).unwrap(),
        make_flight(60.0, 50.0, false, *b"B738", "AIR2"),
    );

    let mut cruise_cands: HashMap<u64, TopFlightCandidate> = HashMap::new();
    // Cruise candidate louder than AIR2 → must interleave between
    // the two airborne entries.
    cruise_cands.insert(
        flight_id::pack_real(0xC1, 1_700_000_010).unwrap(),
        make_cruise_cand(70.0, *b"B777", "CRZ1"),
    );

    let out = build_top_flights(&crate::compute::key_sorted(&flights), &cruise_cands, 150.0);
    assert_eq!(out.len(), 3);
    assert_eq!(out[0].callsign, "AIR1");
    assert_eq!(out[1].callsign, "CRZ1", "cruise must interleave by Lmax");
    assert_eq!(out[1].aircraft_type, "B777");
    assert_eq!(out[1].energy_pct, 0.0, "cruise rows carry energy_pct=0");
    assert!(!out[1].synthetic, "real cruise fid is not synthetic");
    assert_eq!(out[2].callsign, "AIR2");
}

#[test]
fn build_top_flights_dedupes_same_fid_in_both_maps() {
    let dual_fid = flight_id::pack_real(0xDEAD, 1_700_000_100).unwrap();
    let mut flights: HashMap<u64, FlightAccum> = HashMap::new();
    flights.insert(dual_fid, make_flight(75.0, 50.0, false, *b"A320", "DUAL"));

    let mut cruise_cands: HashMap<u64, TopFlightCandidate> = HashMap::new();
    cruise_cands.insert(dual_fid, make_cruise_cand(60.0, *b"A320", "DUAL"));

    let out = build_top_flights(&crate::compute::key_sorted(&flights), &cruise_cands, 50.0);
    assert_eq!(out.len(), 1, "same real fid must not appear twice");
    assert_eq!(out[0].callsign, "DUAL");
    assert!(out[0].energy_pct > 0.0, "airborne entry kept (real energy)");
}

#[test]
fn build_top_flights_synth_cruise_fid_is_marked_synthetic() {
    let flights: HashMap<u64, FlightAccum> = HashMap::new();
    let mut cruise_cands: HashMap<u64, TopFlightCandidate> = HashMap::new();
    let synth = flight_id::pack_synth(0xABCD);
    cruise_cands.insert(synth, make_cruise_cand(50.0, [0; 4], ""));
    let out = build_top_flights(&crate::compute::key_sorted(&flights), &cruise_cands, 1.0);
    assert_eq!(out.len(), 1);
    assert!(
        out[0].synthetic,
        "synth cruise fid must surface as synthetic"
    );
    assert!(out[0].start_unix.is_none());
}

/// One airborne sub-seg passing ~250 m abeam the receiver at ~150 m
/// AGL, for the given ICAO typecode. Used by the mixed-window GA
/// hybrid scatter test.
fn one_subseg_row<'a>(fid: u64, typecode: &str, cols: &'a OneSubSeg) -> AirborneRowView<'a> {
    use crate::compute::aircraft_v6::views::{BBox, SubSegmentSlice};
    AirborneRowView {
        flight_id: fid,
        callsign: "",
        aircraft_type: cols.typebuf,
        profile_idx: aircraft::profile_idx(typecode),
        source_id: 0,
        origin: 0,
        sub_segments: SubSegmentSlice {
            start_lat: &cols.start_lat,
            start_lon: &cols.start_lon,
            start_alt_m: &cols.alt,
            end_lat: &cols.end_lat,
            end_lon: &cols.end_lon,
            end_alt_m: &cols.alt,
            speed_kt: &cols.speed,
            length_m: &cols.length,
            period: &cols.period,
            date_id: &cols.date_id,
            flags: &cols.flags,
            terrain_start_elev_m: &cols.elev,
            terrain_end_elev_m: &cols.elev,
        },
        bbox: BBox {
            min_lat: cols.start_lat[0].min(cols.end_lat[0]),
            max_lat: cols.start_lat[0].max(cols.end_lat[0]),
            min_lon: cols.start_lon[0].min(cols.end_lon[0]),
            max_lon: cols.start_lon[0].max(cols.end_lon[0]),
        },
    }
}

struct OneSubSeg {
    typebuf: [u8; 4],
    start_lat: [f32; 1],
    start_lon: [f32; 1],
    end_lat: [f32; 1],
    end_lon: [f32; 1],
    alt: [f32; 1],
    speed: [f32; 1],
    length: [f32; 1],
    period: [u8; 1],
    date_id: [i16; 1],
    flags: [u8; 1],
    elev: [f32; 1],
}

fn one_subseg(typecode: &str) -> OneSubSeg {
    let mut typebuf = [0u8; 4];
    let b = typecode.as_bytes();
    typebuf[..b.len()].copy_from_slice(b);
    OneSubSeg {
        typebuf,
        // ~250 m E-W track abeam a receiver at 14.250 / 50.100.
        start_lat: [50.1015],
        start_lon: [14.2480],
        end_lat: [50.1015],
        end_lon: [14.2520],
        alt: [150.0],
        speed: [120.0],
        length: [285.0],
        period: [0],
        date_id: [0],
        flags: [0], // arrival
        elev: [0.0],
    }
}

struct FlatGround;
impl crate::types::RasterSampler for FlatGround {
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

/// Mixed-window GA hybrid scatter: with a 12-day airline window and a
/// 365-day GA window, a GA-class (C172) flight's accumulated energy +
/// count weight must be exactly `12/365` of the same flight scattered
/// under the uniform LUT, while an airline-class (B738) flight stays
/// at `1.0`. This is the +14.8 dB Kytín phantom kill, in one assert.
#[test]
fn mixed_window_ga_weighted_airline_unchanged() {
    let receiver = Receiver::new(50.100, 14.250, 0.0);
    let horizon = aircraft::ReceiverHorizon::build(
        |_, _| 0.0,
        receiver.lat,
        receiver.lon,
        receiver.altitude_m(),
    );
    // Build the hybrid LUT: GA classes → 365, airline classes → 12.
    let vec: String = (0..aircraft::NUM_CLASSES)
        .map(|c| {
            if aircraft::is_ga_sampled_class(c as u8) {
                "365"
            } else {
                "12"
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    let hybrid = aircraft::ClassWeights::parse(Some(&vec), 12).unwrap();
    let uniform = aircraft::ClassWeights::uniform();

    for (typecode, expect_ga) in [("C172", true), ("R44", true), ("B738", false)] {
        let cols = one_subseg(typecode);
        let row = [one_subseg_row(
            flight_id::pack_real(0xABCD01, 1_700_000_000).unwrap(),
            typecode,
            &cols,
        )];
        let uni = scatter(&receiver, &row, 12.0, &uniform, &horizon, None, 0, None);
        let hyb = scatter(&receiver, &row, 12.0, &hybrid, &horizon, None, 0, None);
        let e_uni: f64 = uni
            .values()
            .map(|a| a.period_energy.iter().sum::<f64>())
            .sum();
        let e_hyb: f64 = hyb
            .values()
            .map(|a| a.period_energy.iter().sum::<f64>())
            .sum();
        assert!(
            e_uni > 0.0,
            "{typecode}: sub-seg must be audible at the receiver"
        );
        let expected_ratio = if expect_ga { 12.0 / 365.0 } else { 1.0 };
        assert!(
            (e_hyb / e_uni - expected_ratio).abs() < 1e-9,
            "{typecode}: hybrid/uniform energy ratio {} != {expected_ratio}",
            e_hyb / e_uni
        );
        // Count weight rides the same factor (helicopter_flights_per_day,
        // observed_flights_per_day).
        let fw = hyb.values().next().unwrap().flight_weight;
        assert!(
            (fw - expected_ratio).abs() < 1e-9,
            "{typecode}: flight_weight {fw} != {expected_ratio}"
        );
    }
}

/// The popup aggregation carries the GA weight end-to-end: the aircraft
/// periods of a lone GA flight drop ~10·log10(365/12) ≈ 14.8 dB vs the
/// uniform window (the Kytín correction).
#[test]
fn ga_hybrid_drops_airborne_lden_by_14_8_db() {
    use crate::compute::aircraft_v6::compute_aircraft_v6;
    let receiver = Receiver::new(50.100, 14.250, 0.0);
    let cols = one_subseg("R44");
    let row = [one_subseg_row(
        flight_id::pack_real(0xBEEF02, 1_700_000_000).unwrap(),
        "R44",
        &cols,
    )];
    let vec: String = (0..aircraft::NUM_CLASSES)
        .map(|c| {
            if aircraft::is_ga_sampled_class(c as u8) {
                "365"
            } else {
                "12"
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    let hybrid = aircraft::ClassWeights::parse(Some(&vec), 12).unwrap();
    let uniform = aircraft::ClassWeights::uniform();
    let horizon = aircraft::ReceiverHorizon::build(
        |_, _| 0.0,
        receiver.lat,
        receiver.lon,
        receiver.altitude_m(),
    );
    let uni = compute_aircraft_v6(
        &receiver,
        &row,
        &[],
        &FlatGround,
        Some(&horizon),
        None,
        12,
        &uniform,
        0,
        None,
        None,
    )
    .0;
    let hyb = compute_aircraft_v6(
        &receiver,
        &row,
        &[],
        &FlatGround,
        Some(&horizon),
        None,
        12,
        &hybrid,
        0,
        None,
        None,
    )
    .0;
    let drop = uni.lden_db - hyb.lden_db;
    let expected = 10.0 * (365.0f64 / 12.0).log10(); // ≈ 14.83 dB
    assert!(
        (drop - expected).abs() < 0.05,
        "GA hybrid airborne Lden drop {drop:.2} dB != {expected:.2} dB"
    );
}

#[test]
fn blocked_popup_retains_free_field_above_received() {
    let receiver = Receiver::new(50.100, 14.250, 0.0);
    let horizon = aircraft::ReceiverHorizon::build(
        |_, _| 100.0,
        receiver.lat,
        receiver.lon,
        receiver.altitude_m(),
    );
    let cols = one_subseg("R44");
    let row = [one_subseg_row(
        flight_id::pack_real(0xBEEF03, 1_700_000_000).unwrap(),
        "R44",
        &cols,
    )];
    let flights = scatter(
        &receiver,
        &row,
        1.0,
        &aircraft::ClassWeights::uniform(),
        &horizon,
        None,
        0,
        None,
    );
    let acc = flights.values().next().expect("blocked flight retained");
    assert!(
        acc.free_period_energy[0] > acc.period_energy[0],
        "screened aircraft must retain pre-screen energy: free={} received={}",
        acc.free_period_energy[0],
        acc.period_energy[0]
    );

    let cruise_flights = HashMap::new();
    let candidates = HashMap::new();
    let cruise_bands = std::array::from_fn(|_| BandStats::new());
    let (received, free, impacts, _) = build_detail(
        &flights,
        &cruise_flights,
        0,
        &candidates,
        &cruise_bands,
        1.0,
        1.0,
    );
    assert!(
        free.lden_db > received.lden_db,
        "blocked popup must show free-field above received: free={} received={}",
        free.lden_db,
        received.lden_db
    );
    assert!(impacts.terrain < 0.0, "terrain impact must be negative");
}

/// Every f64 total the popup's aircraft detail exposes must be a function
/// of the accumulator CONTENTS, never of the map walk order. `RandomState`
/// re-seeds per `HashMap::new()`, so building the same content twice in one
/// process already gives two different walk orders — this test rebuilds
/// each map on every repeat and demands identical bytes.
///
/// It complements the end-to-end
/// `compute::aircraft_v6::tests::repeated_identical_clicks_are_bit_identical`:
/// that one drives real geometry but can only make the dominant energy sums
/// order-sensitive. Here the inputs are chosen to be adversarial for each
/// individual accumulator — non-uniform `flight_weight`
/// (`observed_flights_per_day`), scattered altitudes (`avg_altitude_m` via
/// `band_stats`), and a wall of exactly-equal `peak_lmax` straddling the
/// 20-row `top_flights` cut.
#[test]
fn aircraft_detail_ignores_map_iteration_order() {
    use crate::compute::aircraft_v6::cruise::band_stats;
    use crate::compute::aircraft_v6::state::CruiseFlightStats;

    const N: usize = 400;
    // Irrational-ish spread: consecutive terms differ across the whole
    // mantissa, so any re-association shows up in the low bits.
    let spread = |i: usize, salt: f64| -> f64 {
        ((i as f64 * 0.617_924_313_7 + salt).sin() * 0.5 + 0.5) * 0.999 + 0.001
    };

    let build = || {
        let mut flights: HashMap<u64, FlightAccum> = HashMap::new();
        let mut cruise_flights: HashMap<u64, FlightAccum> = HashMap::new();
        let mut stats: HashMap<u64, CruiseFlightStats> = HashMap::new();
        let mut cands: HashMap<u64, TopFlightCandidate> = HashMap::new();
        for i in 0..N {
            let fid = flight_id::pack_real(
                0x40_0000 + i as u32,
                1_750_000_000 + (i as u32 % 9) * 86_400,
            )
            .expect("test fid");
            let mut acc = FlightAccum::new(
                (i % 8) as u8,
                spread(i, 1.0) * 3.0,
                false,
                *b"A320",
                format!("CS{i:04}"),
            );
            // Airborne energies deliberately two decades BELOW the cruise
            // ones below: `build_detail` folds cruise in first and airborne
            // on top, so an airborne total that dominated would round the
            // cruise re-association away and hide a regression there.
            acc.period_energy = [
                spread(i, 2.0) * 1e-5,
                spread(i, 3.0) * 1e-5,
                spread(i, 4.0) * 1e-5,
            ];
            acc.free_period_energy = acc.period_energy;
            acc.no_terrain_period_energy = acc.period_energy;
            acc.no_screening_period_energy = acc.period_energy;
            // 12 airborne flights sit on ONE Lmax value and everything else
            // is strictly below it. The cruise side (below) puts a much
            // larger wall on the SAME value, so the 20-row cut is filled by
            // 12 airborne ties plus 8 of the cruise ties — which 8 depends
            // entirely on the cruise walk order, and which 12 on the
            // airborne one.
            acc.peak_lmax = if i < 12 {
                72.5
            } else {
                40.0 + spread(i, 5.0) * 25.0
            };
            acc.peak_altitude_m = 200.0 + spread(i, 6.0) * 9_000.0;
            acc.min_dist_m = 100.0 + spread(i, 7.0) * 5_000.0;
            flights.insert(fid, acc);

            let synth = flight_id::pack_synth(i as u64);
            let mut cacc =
                FlightAccum::new((i % 8) as u8, spread(i, 8.0), true, [0; 4], String::new());
            // Three decades of spread: big enough that re-association moves
            // the low bits of the total, small enough that no term is lost
            // under the sum's ULP (which would make order irrelevant again).
            let mag = 1e-3 * 10f64.powi(-((i % 4) as i32));
            cacc.period_energy = [
                spread(i, 9.0) * mag,
                spread(i, 10.0) * mag,
                spread(i, 11.0) * mag,
            ];
            cacc.free_period_energy = cacc.period_energy;
            cacc.no_terrain_period_energy = cacc.period_energy;
            cacc.no_screening_period_energy = cacc.period_energy;
            cacc.peak_lmax = 30.0 + spread(i, 12.0) * 30.0;
            cruise_flights.insert(synth, cacc);

            let real = flight_id::pack_real(
                0x50_0000 + i as u32,
                1_750_000_000 + (i as u32 % 6) * 86_400,
            )
            .expect("test fid");
            stats.insert(
                real,
                CruiseFlightStats {
                    peak_lmax: 31.0 + spread(i, 13.0) * 40.0,
                    alt_at_peak: 8_000.0 + spread(i, 14.0) * 4_000.0,
                    class_at_peak: i % aircraft::NUM_CLASSES,
                },
            );
            cands.insert(
                real,
                TopFlightCandidate {
                    peak_lmax: if i % 3 == 0 {
                        72.5
                    } else {
                        45.0 + spread(i, 15.0) * 20.0
                    },
                    peak_altitude_m: 9_000.0 + spread(i, 16.0) * 2_000.0,
                    peak_period: (i % 3) as u8,
                    peak_seg_start: [14.0, 50.0],
                    peak_seg_end: [14.1, 50.1],
                    min_dist_m: 5_000.0 + spread(i, 17.0) * 5_000.0,
                    profile_idx: (i % 8) as u8,
                    aircraft_type: *b"B738",
                    callsign: format!("DL{i:04}"),
                },
            );
        }
        (flights, cruise_flights, stats, cands)
    };

    let run = || {
        let (flights, cruise_flights, stats, cands) = build();
        let bands = band_stats(&stats);
        let (periods, periods_free, _impacts, detail) = build_detail(
            &flights,
            &cruise_flights,
            stats.len(),
            &cands,
            &bands,
            7.0,
            7.0,
        );
        // JSON round-trips f64 as shortest-roundtrip decimal, which is
        // injective on f64 — equal strings mean equal bits.
        serde_json::to_string(&(&periods, &periods_free, &detail)).unwrap()
    };

    let first = run();
    assert!(
        first.contains("\"top_flights\":[{"),
        "no top flights built — test would be vacuous"
    );
    for i in 1..10 {
        let again = run();
        if again != first {
            let at = first
                .bytes()
                .zip(again.bytes())
                .position(|(a, b)| a != b)
                .unwrap_or(first.len().min(again.len()));
            let from = at.saturating_sub(70);
            panic!(
                "aircraft detail changed on repeat {i} with identical accumulator \
                 contents — an f64 sum, max or top-N cut is walking a HashMap again; \
                 see crate::compute::key_sorted.\n  first byte {at} differs\n  \
                 run 0: …{}\n  run {i}: …{}",
                &first[from..(at + 30).min(first.len())],
                &again[from..(at + 30).min(again.len())],
            );
        }
    }
}

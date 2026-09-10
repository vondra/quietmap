//! Split chords take the 20 dB floors as one event and one trace slot, drawn whole.

use super::*;
use crate::compute::aircraft_v6::airborne::chords::{CHORD_START, SPLIT_PIECE};

const CHORD_END: u8 = 1 << 5;

/// An east-west level chord at the equator `from_km..to_km` east of the
/// receiver at (0, 0), `alt_m` above it, as one row or as `pieces` rows.
fn equator_chord(
    cols: &mut SynthColumns,
    fid: u64,
    key: i32,
    from_km: f64,
    to_km: f64,
    alt_m: i16,
    speed: f32,
    departure: bool,
    pieces: u32,
) {
    let lon = |km: f64| km * 1_000.0 / 111_320.0;
    let point = |km: f64| grid::lonlat_to_grid(f64::from(lon(km) as f32), 0.0);
    let length = ((to_km - from_km) * 1_000.0 / pieces as f64) as f32;
    for k in 0..pieces {
        let a = from_km + (to_km - from_km) * k as f64 / pieces as f64;
        let b = from_km + (to_km - from_km) * (k + 1) as f64 / pieces as f64;
        let mut flags = u8::from(departure);
        if pieces > 1 {
            flags |= SPLIT_PIECE;
            if k == 0 {
                flags |= CHORD_START;
            }
            if k + 1 == pieces {
                flags |= CHORD_END;
            }
        }
        cols.push_row(
            fid,
            key,
            point(a),
            point(b),
            (alt_m, alt_m),
            speed,
            length,
            0,
            flags,
            0,
        );
    }
}

fn equator_receiver() -> (Receiver, aircraft::ReceiverHorizon) {
    let receiver = Receiver::new(0.0, 0.0, 0.0);
    let horizon = aircraft::ReceiverHorizon::build(
        |_, _| 0.0,
        receiver.lat,
        receiver.lon,
        receiver.altitude_m(),
    );
    (receiver, horizon)
}

fn received_sel_db(flights: &HashMap<u64, FlightAccum>, fid: u64) -> f64 {
    10.0 * flights[&fid].period_energy.iter().sum::<f64>().log10()
}

/// Reviewer example: a B738 approach at 250 kt, 1 500 m up, 8–16 km east.
/// The whole chord is 20.90 dB SEL; its two 4 km pieces are 19.95 and
/// 13.82 dB and would each fail the per-event floor. Floored as one chord,
/// the flight keeps the whole chord's energy (and its top-flight row).
#[test]
fn split_chord_takes_the_event_floor_as_one_event() {
    let (receiver, horizon) = equator_receiver();
    let weights = aircraft::ClassWeights::uniform();
    let fid = flight_id::pack_real(0xB738, 1_750_000_000).unwrap();
    let mut whole = SynthColumns::new();
    let key = whole.add_flight("CSA1", "B738", aircraft::profile_idx("B738"));
    equator_chord(&mut whole, fid, key, 8.0, 16.0, 1_500, 250.0, false, 1);
    let mut split = SynthColumns::new();
    let key = split.add_flight("CSA1", "B738", aircraft::profile_idx("B738"));
    equator_chord(&mut split, fid, key, 8.0, 16.0, 1_500, 250.0, false, 2);
    let run = |cols: &SynthColumns| {
        scatter(
            &receiver,
            &cols.batches(1),
            1.0,
            &weights,
            &horizon,
            None,
            0,
            None,
        )
    };
    let whole_flights = run(&whole);
    let split_flights = run(&split);
    let whole_sel = received_sel_db(&whole_flights, fid);
    let split_sel = received_sel_db(&split_flights, fid);
    // Each piece on its own, unfloored, for the record.
    let ctx = super::super::ScatterContext::new(&receiver, 1.0, &weights, &horizon, None);
    let batches = split.batches(1);
    let piece_sel: Vec<f64> = batches
        .iter()
        .map(|b| {
            super::super::row::evaluate_row::<false>(&ctx, b, 0)
                .unwrap()
                .kernel
                .sel
        })
        .collect();
    eprintln!("floor example: whole {whole_sel:.2} dB, pieces {piece_sel:.2?} dB, chord {split_sel:.2} dB");
    assert!((whole_sel - 20.90).abs() < 0.05, "{whole_sel}");
    assert!(piece_sel.iter().all(|&sel| sel < 20.0), "{piece_sel:?}");
    assert!(
        (split_sel - whole_sel).abs() <= 0.01,
        "{split_sel} vs {whole_sel}"
    );
    assert_eq!(split_flights[&fid].peak_lmax, whole_flights[&fid].peak_lmax);

    // A chord whose summed free-field SEL stays below 20 dB is not an
    // event at all: no accumulator, exactly like an unsplit sub-segment
    // the kernel refuses.
    let mut faint = SynthColumns::new();
    let key = faint.add_flight("CSA1", "B738", aircraft::profile_idx("B738"));
    equator_chord(&mut faint, fid, key, 12.0, 16.0, 1_500, 250.0, false, 2);
    assert!(run(&faint).is_empty());
}

/// 200 identical 4.02 km chords centred over the receiver, each split in
/// two, against a 150-slot cap: a chord holds one slot and is drawn whole,
/// so the drawn length equals the unsplit run's 150 × 4.02 km.
#[test]
fn split_chords_hold_one_trace_slot_each_and_draw_whole() {
    const CAP: usize = 150;
    let (receiver, horizon) = equator_receiver();
    let weights = aircraft::ClassWeights::uniform();
    let build = |pieces: u32| {
        let mut cols = SynthColumns::new();
        for flight in 0..200u64 {
            let key = cols.add_flight(
                &format!("CS{flight:03}"),
                "A320",
                aircraft::profile_idx("A320"),
            );
            let fid = flight_id::pack_real(0x40_0000 + flight as u32, 1_750_000_000).unwrap();
            equator_chord(&mut cols, fid, key, -2.01, 2.01, 300, 220.0, true, pieces);
        }
        cols
    };
    let drawn = |cols: &SynthColumns| {
        let mut traces = TraceCollector::new();
        let flights = scatter(
            &receiver,
            &cols.batches(4_096),
            1.0,
            &weights,
            &horizon,
            None,
            CAP,
            Some(&mut traces),
        );
        assert_eq!(flights.len(), 200);
        let length: f64 = traces
            .segments
            .iter()
            .map(|t| grid::geo::flat_dist(t.start_lat, t.start_lon, t.end_lat, t.end_lon))
            .sum();
        let mut names: Vec<_> = traces.segments.iter().map(|t| t.name.clone()).collect();
        names.sort();
        names.dedup();
        (
            traces.segments.len(),
            names.len(),
            length,
            traces.airborne_above_cutoff,
        )
    };
    let (unsplit_traces, unsplit_flights, unsplit_length, unsplit_visible) = drawn(&build(1));
    let (split_traces, split_flights, split_length, split_visible) = drawn(&build(2));
    eprintln!("trace slots: unsplit {unsplit_traces} traces / {unsplit_length:.0} m, split {split_traces} traces / {split_length:.0} m");
    assert_eq!((unsplit_traces, unsplit_flights), (CAP, CAP));
    assert_eq!((split_traces, split_flights), (2 * CAP, CAP));
    assert!((unsplit_length - 150.0 * 4_020.0).abs() < 150.0 * 2.0);
    assert!(
        (split_length - unsplit_length).abs() < 1.0,
        "{split_length} vs {unsplit_length}"
    );
    assert_eq!((unsplit_visible, split_visible), (200, 400));
}

/// Two consecutive split chords of one flight share the sample point
/// between them but are two events: `CHORD_START` stops the chain.
#[test]
fn consecutive_split_chords_stay_separate_events() {
    let (receiver, horizon) = equator_receiver();
    let weights = aircraft::ClassWeights::uniform();
    let fid = flight_id::pack_real(0xC0DE, 1_750_000_000).unwrap();
    let mut cols = SynthColumns::new();
    let key = cols.add_flight("TWO", "A320", aircraft::profile_idx("A320"));
    equator_chord(&mut cols, fid, key, -8.0, 0.0, 300, 220.0, true, 2);
    equator_chord(&mut cols, fid, key, 0.0, 8.0, 300, 220.0, true, 2);
    let mut traces = TraceCollector::new();
    scatter(
        &receiver,
        &cols.batches(4_096),
        1.0,
        &weights,
        &horizon,
        None,
        1,
        Some(&mut traces),
    );
    assert_eq!(
        traces.segments.len(),
        2,
        "one chord = one slot = its two pieces"
    );
    let lons: Vec<f64> = traces
        .segments
        .iter()
        .flat_map(|t| [t.start_lon, t.end_lon])
        .collect();
    assert!(
        lons.iter().all(|&lon| lon <= 0.0) || lons.iter().all(|&lon| lon >= 0.0),
        "{lons:?}"
    );
}

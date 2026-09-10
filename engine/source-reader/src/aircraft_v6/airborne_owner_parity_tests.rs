//! Owner-square rows through shuffle, Stage 2A and the popup reader reproduce the
//! kernel run once over every sub-segment; split chords hold the SPEC contract.

use super::AirborneRowAccum;
use aircraft_extract::flight::{FlightSegment, Phase};
use noise_compute::compute::aircraft_v6::{
    compute_aircraft_v6, AirborneFlightTable, AirborneSegmentBatch,
};
use noise_compute::emission::aircraft::{self, AIRBORNE_SUB_SEGMENT_MAX_LENGTH_M};
use noise_compute::types::{
    AircraftTopFlight, NoisePeriods, RasterSampler, Receiver, TraceCollector,
};
use std::path::Path;

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

const RECEIVER: (f64, f64) = (50.0, 14.0);
/// Degrees of longitude per metre at the receiver latitude, the metric of
/// `flat_dist`, the envelope and the owner-square box.
fn lon_per_m() -> f32 {
    (1.0 / (111_320.0 * RECEIVER.0.to_radians().cos())) as f32
}

fn chord(
    flight_id: u64,
    callsign: &str,
    start: (f32, f32),
    end: (f32, f32),
    period: u8,
) -> FlightSegment {
    FlightSegment {
        flight_id,
        callsign: callsign.into(),
        aircraft_type: *b"B738",
        profile_idx: noise_compute::emission::profiles_generated::profile_idx("B738"),
        source_id: 2,
        origin: 0,
        veh_kind: 0,
        gse_class: 0,
        period,
        date_id: 40,
        phase: Phase::Airborne,
        flags: 1,
        start_lat: start.0,
        start_lon: start.1,
        start_alt_m: 400.0,
        end_lat: end.0,
        end_lon: end.1,
        end_alt_m: 500.0,
        speed_kt: 220.0,
        length_m: aircraft_extract::geo::flat_dist(start.0, start.1, end.0, end.1),
        agl_avg_m: 450.0,
        start_elev_m: 0.0,
        end_elev_m: 0.0,
    }
}

/// Three flights of chained chords straddling the z9 column boundary at
/// 14.0625° E, 2–6 km from the receiver; every chord within the cap.
fn flights_within_cap() -> Vec<FlightSegment> {
    let mut rows = Vec::new();
    for (flight, callsign, lat) in [
        (1, "CSA1", 50.02_f32),
        (2, "TVS2", 50.03),
        (3, "RYR3", 50.04),
    ] {
        let fid = noise_compute::flight_id::pack_real(0x40_0000 + flight, 1_750_000_000).unwrap();
        let mut lon = 13.96_f32;
        for k in 0..8 {
            let next = lon + 2_500.0 * lon_per_m();
            rows.push(chord(
                fid,
                callsign,
                (lat, lon),
                (lat + 0.001, next),
                (k % 3) as u8,
            ));
            lon = next;
        }
    }
    rows
}

/// The kernel over every row in one in-memory batch — what today's popup
/// evaluated for all copies in the receiver square.
fn reference_batch(rows: &[FlightSegment]) -> ReferenceColumns {
    let mut cols = ReferenceColumns::default();
    cols.callsign_offsets.push(0);
    for row in rows {
        let key = match cols.flight_ids.iter().position(|&id| id == row.flight_id) {
            Some(key) => key,
            None => {
                cols.flight_ids.push(row.flight_id);
                cols.callsign_bytes
                    .extend_from_slice(row.callsign.as_bytes());
                cols.callsign_offsets.push(cols.callsign_bytes.len() as i32);
                cols.aircraft_type.extend_from_slice(&row.aircraft_type);
                cols.profile.push(row.profile_idx);
                cols.source.push(row.source_id);
                cols.origin.push(row.origin);
                cols.flight_ids.len() - 1
            }
        };
        cols.flight_id.push(row.flight_id);
        cols.flight_key.push(key as i32);
        let (gx, gy) = grid::lonlat_to_grid(row.start_lon as f64, row.start_lat as f64);
        cols.start_gx.push(gx);
        cols.start_gy.push(gy);
        let (gx, gy) = grid::lonlat_to_grid(row.end_lon as f64, row.end_lat as f64);
        cols.end_gx.push(gx);
        cols.end_gy.push(gy);
        cols.start_alt.push(row.start_alt_m.round() as i16);
        cols.end_alt.push(row.end_alt_m.round() as i16);
        cols.speed.push(row.speed_kt);
        cols.length.push(row.length_m);
        cols.period.push(row.period);
        cols.date_id.push(row.date_id);
        cols.flags.push(row.flags & 1);
        cols.start_elev.push(row.start_elev_m.round() as i16);
        cols.end_elev.push(row.end_elev_m.round() as i16);
    }
    cols
}

#[derive(Default)]
struct ReferenceColumns {
    flight_ids: Vec<u64>,
    flight_id: Vec<u64>,
    flight_key: Vec<i32>,
    start_gx: Vec<i32>,
    start_gy: Vec<i32>,
    end_gx: Vec<i32>,
    end_gy: Vec<i32>,
    start_alt: Vec<i16>,
    end_alt: Vec<i16>,
    speed: Vec<f32>,
    length: Vec<f32>,
    period: Vec<u8>,
    date_id: Vec<i16>,
    flags: Vec<u8>,
    start_elev: Vec<i16>,
    end_elev: Vec<i16>,
    callsign_offsets: Vec<i32>,
    callsign_bytes: Vec<u8>,
    aircraft_type: Vec<u8>,
    profile: Vec<u8>,
    source: Vec<u8>,
    origin: Vec<u8>,
}

impl ReferenceColumns {
    fn batch(&self) -> AirborneSegmentBatch<'_> {
        AirborneSegmentBatch {
            flight_id: &self.flight_id,
            flight_key: &self.flight_key,
            flights: AirborneFlightTable {
                callsign_offsets: &self.callsign_offsets,
                callsign_bytes: &self.callsign_bytes,
                aircraft_type: &self.aircraft_type,
                profile_idx: &self.profile,
                source_id: &self.source,
                origin: &self.origin,
            },
            start_gy: &self.start_gy,
            start_gx: &self.start_gx,
            start_alt_m: &self.start_alt,
            end_gy: &self.end_gy,
            end_gx: &self.end_gx,
            end_alt_m: &self.end_alt,
            speed_kt: &self.speed,
            length_m: &self.length,
            period: &self.period,
            date_id: &self.date_id,
            flags: &self.flags,
            terrain_start_elev_m: &self.start_elev,
            terrain_end_elev_m: &self.end_elev,
        }
    }
}

struct Popup {
    periods: NoisePeriods,
    top_flights: Vec<AircraftTopFlight>,
    traces: TraceCollector,
}

fn popup(receiver: &Receiver, batches: &[AirborneSegmentBatch<'_>]) -> Popup {
    let horizon = aircraft::ReceiverHorizon::build(
        |_, _| 0.0,
        receiver.lat,
        receiver.lon,
        receiver.altitude_m(),
    );
    let mut traces = TraceCollector::new();
    let (periods, _, bands) = compute_aircraft_v6(
        receiver,
        batches,
        &[],
        &FlatGround,
        Some(&horizon),
        None,
        12,
        &aircraft::ClassWeights::uniform(),
        1_000,
        Some(&mut traces),
        None,
    );
    Popup {
        periods,
        top_flights: bands.airborne.top_flights,
        traces,
    }
}

/// Stage 1 day → shuffle → Stage 2A under `root`, returning the prepared year directory.
fn build_prepared(root: &Path, rows: &[FlightSegment]) -> std::path::PathBuf {
    let day = root.join("segments/2025-02-09.arrow");
    let mut rows = rows.to_vec();
    for row in &mut rows {
        row.date_id = aircraft_extract::period::parse_date_id("2025-02-09").unwrap();
    }
    aircraft_extract::arrow_io::write_segments(&day, &rows).unwrap();
    let by_square = root.join("segments_by_square");
    aircraft_extract::shuffle::shuffle_per_square(&[day], &[], &by_square, None).unwrap();
    let prepared = root.join("prepared");
    aircraft_extract::stage_2a::run_stage_2a(&by_square, &prepared, 12, 0, None).unwrap();
    prepared
}

fn top_flight_keys(flights: &[AircraftTopFlight]) -> Vec<(String, f64)> {
    flights
        .iter()
        .map(|flight| (flight.callsign.clone(), flight.lmax_db))
        .collect()
}

fn energy(db: f64) -> f64 {
    if db.is_finite() {
        10f64.powf(db / 10.0)
    } else {
        0.0
    }
}

fn assert_periods_within(actual: &NoisePeriods, expected: &NoisePeriods, relative: f64) {
    for (name, a, e) in [
        ("ld", actual.ld_db, expected.ld_db),
        ("le", actual.le_db, expected.le_db),
        ("ln", actual.ln_db, expected.ln_db),
    ] {
        let (a, e) = (energy(a), energy(e));
        assert!(
            (a - e).abs() <= relative * e.max(a),
            "{name}: {a:e} vs reference {e:e}"
        );
    }
}

/// Where no chord was split, rows read from two owner squares give the
/// reference's energy per period within 1e-9 relative, the same top
/// flights and the same trace set.
#[test]
fn owner_square_rows_reproduce_the_all_in_one_reference_without_splits() {
    let rows = flights_within_cap();
    assert!(rows
        .iter()
        .all(|row| row.length_m <= AIRBORNE_SUB_SEGMENT_MAX_LENGTH_M));
    let tmp = tempfile::tempdir().unwrap();
    let prepared = build_prepared(tmp.path(), &rows);
    let owners = aircraft_extract::spatial::square_directories(&prepared).unwrap();
    assert_eq!(owners.len(), 2, "chords straddle the z9 column boundary");
    let receiver = Receiver::new(RECEIVER.0, RECEIVER.1, 0.0);
    let collected =
        crate::query::collect_sources_at_point(&prepared, receiver.lat, receiver.lon).unwrap();
    let decoded = AirborneRowAccum::new(&collected.aircraft_airborne_batches).unwrap();
    let candidate = popup(&receiver, decoded.views());
    let reference_columns = reference_batch(&rows);
    let reference = popup(&receiver, &[reference_columns.batch()]);
    assert!(reference.periods.lden_db > 10.0, "{:?}", reference.periods);
    assert_periods_within(&candidate.periods, &reference.periods, 1e-9);
    assert_eq!(
        top_flight_keys(&candidate.top_flights),
        top_flight_keys(&reference.top_flights)
    );
    assert_eq!(candidate.top_flights.len(), 3);
    let key = |trace: &noise_compute::types::SegmentTrace| {
        (
            trace.name.clone(),
            trace.start_lat.to_bits(),
            trace.start_lon.to_bits(),
            trace.end_lat.to_bits(),
            trace.end_lon.to_bits(),
        )
    };
    let mut want: Vec<_> = reference.traces.segments.iter().map(key).collect();
    let mut got: Vec<_> = candidate.traces.segments.iter().map(key).collect();
    want.sort();
    got.sort();
    assert_eq!(got, want, "same rows, each once, above the trace cutoff");
    assert!(
        got.len() > rows.len() / 2,
        "trace set too small to be a proof"
    );
}

/// Where a chord was split, Lden moves at most 0.01 dB per period, the top
/// flights are the same and the split flight draws one polyline of the
/// chord's length.
#[test]
fn split_chord_holds_the_lden_top_flight_and_trace_length_contract() {
    let mut rows = flights_within_cap();
    let fid = noise_compute::flight_id::pack_real(0x40_0009, 1_750_000_000).unwrap();
    // 18 km due east 3 km north of the receiver: five pieces.
    let long = chord(
        fid,
        "LONG9",
        (50.027, 13.9),
        (50.027, 13.9 + 18_000.0 * lon_per_m()),
        0,
    );
    // `flat_dist` scales by the chord's own latitude; 3 km north costs ~10 m.
    assert!((long.length_m - 18_000.0).abs() < 25.0, "{}", long.length_m);
    rows.push(long.clone());
    let tmp = tempfile::tempdir().unwrap();
    let prepared = build_prepared(tmp.path(), &rows);
    let receiver = Receiver::new(RECEIVER.0, RECEIVER.1, 0.0);
    let collected =
        crate::query::collect_sources_at_point(&prepared, receiver.lat, receiver.lon).unwrap();
    let decoded = AirborneRowAccum::new(&collected.aircraft_airborne_batches).unwrap();
    let candidate = popup(&receiver, decoded.views());
    let reference_columns = reference_batch(&rows);
    let reference = popup(&receiver, &[reference_columns.batch()]);
    for (name, a, e) in [
        ("ld", candidate.periods.ld_db, reference.periods.ld_db),
        ("le", candidate.periods.le_db, reference.periods.le_db),
        ("ln", candidate.periods.ln_db, reference.periods.ln_db),
        ("lden", candidate.periods.lden_db, reference.periods.lden_db),
    ] {
        assert!((a - e).abs() <= 0.01, "{name}: {a} vs reference {e}");
    }
    assert_eq!(
        candidate
            .top_flights
            .iter()
            .map(|f| f.callsign.clone())
            .collect::<Vec<_>>(),
        reference
            .top_flights
            .iter()
            .map(|f| f.callsign.clone())
            .collect::<Vec<_>>()
    );
    assert!(candidate.top_flights.iter().any(|f| f.callsign == "LONG9"));
    let drawn = |popup: &Popup| -> (usize, f64) {
        let pieces: Vec<_> = popup
            .traces
            .segments
            .iter()
            .filter(|trace| trace.name.contains("LONG9"))
            .collect();
        let length: f64 = pieces
            .iter()
            .map(|t| grid::geo::flat_dist(t.start_lat, t.start_lon, t.end_lat, t.end_lon))
            .sum();
        (pieces.len(), length)
    };
    let (reference_pieces, reference_length) = drawn(&reference);
    let (candidate_pieces, candidate_length) = drawn(&candidate);
    assert_eq!((reference_pieces, candidate_pieces), (1, 5));
    assert!(
        (candidate_length - reference_length).abs() < 1.0,
        "polyline {candidate_length} m vs chord {reference_length} m"
    );
}

/// A sub-segment whose midpoint is 17 km from the click but whose geometry
/// comes within 16 km is owned by a neighbouring square and still found;
/// one whose geometry stays beyond 16 km contributes nothing.
#[test]
fn reader_pad_finds_rows_owned_by_squares_seventeen_km_away() {
    let fid = noise_compute::flight_id::pack_real(0x40_0011, 1_750_000_000).unwrap();
    let east = |km: f32| {
        (
            RECEIVER.0 as f32,
            RECEIVER.1 as f32 + km * 1_000.0 * lon_per_m(),
        )
    };
    let within = chord(fid, "PAD17", east(15.0), east(19.0), 0);
    let far_id = noise_compute::flight_id::pack_real(0x40_0012, 1_750_000_000).unwrap();
    let beyond = chord(far_id, "FAR19", east(17.0), east(21.0), 0);
    let receiver_square = grid::square_of(RECEIVER.0, RECEIVER.1);
    let (mid_lat, mid_lon) = aircraft_extract::geo::midpoint(
        within.start_lat,
        within.start_lon,
        within.end_lat,
        within.end_lon,
    );
    let owner = grid::square_of(mid_lat as f64, mid_lon as f64);
    assert_ne!(owner, receiver_square, "the owner is a neighbouring square");
    let tmp = tempfile::tempdir().unwrap();
    let prepared = build_prepared(tmp.path(), &[within, beyond]);
    let receiver = Receiver::new(RECEIVER.0, RECEIVER.1, 0.0);
    assert!(
        crate::query::squares_within_reach(receiver.lat, receiver.lon)
            .unwrap()
            .contains(&owner)
    );
    let collected =
        crate::query::collect_sources_at_point(&prepared, receiver.lat, receiver.lon).unwrap();
    let decoded = AirborneRowAccum::new(&collected.aircraft_airborne_batches).unwrap();
    let result = popup(&receiver, decoded.views());
    assert_eq!(
        result
            .top_flights
            .iter()
            .map(|f| f.callsign.as_str())
            .collect::<Vec<_>>(),
        ["PAD17"]
    );
    assert!(result.periods.lden_db.is_finite());
}

/// Reviewer example: a B738 departure at 350 kt, 1 000 m over a receiver at
/// (0, 0), chord 14–32 km east. The whole chord passes the 16 km envelope;
/// after the split only the first piece does. The dropped pieces lie beyond
/// the envelope, each below the reach threshold on its own, and the energy
/// the popup loses is exactly their own energy.
#[test]
fn pieces_beyond_reach_are_dropped_and_the_loss_is_their_own_level() {
    let fid = noise_compute::flight_id::pack_real(0xB738, 1_750_000_000).unwrap();
    let lon = |km: f32| km * 1_000.0 / 111_320.0;
    let mut chord = chord(fid, "REACH", (0.0, lon(14.0)), (0.0, lon(32.0)), 0);
    chord.start_alt_m = 1_000.0;
    chord.end_alt_m = 1_000.0;
    chord.speed_kt = 350.0;
    let receiver = Receiver::new(0.0, 0.0, 0.0);
    let whole = popup(
        &receiver,
        &[reference_batch(std::slice::from_ref(&chord)).batch()],
    );
    let mut pieces = Vec::new();
    aircraft_extract::segment::split::split_airborne_segment(chord.clone(), &mut pieces);
    assert_eq!(pieces.len(), 5);
    let split = popup(&receiver, &[reference_batch(&pieces).batch()]);
    // Ld = 10·log10(E / (n_days × 43 200 s)) with one day-period event.
    let sel = |p: &Popup| 10.0 * (energy(p.periods.ld_db) * 43_200.0 * 12.0).log10();
    let whole_sel = sel(&whole);
    let kept_sel = sel(&split);
    // Each piece on its own, with the class reach lifted, through the
    // hoisted pixel kernel (the same Doc 29 chain).
    let luts = aircraft::NpdLuts::shared();
    let rx_elev = receiver.altitude_m();
    let horizon = aircraft::ReceiverHorizon::build(|_, _| 0.0, 0.0, 0.0, rx_elev);
    let own_sel: Vec<f64> = pieces
        .iter()
        .map(|piece| {
            let [s_lat, s_lon] = aircraft_extract::support::airborne_decoded_endpoint(
                piece.start_lat,
                piece.start_lon,
            )
            .unwrap();
            let [e_lat, e_lon] =
                aircraft_extract::support::airborne_decoded_endpoint(piece.end_lat, piece.end_lon)
                    .unwrap();
            let segment = noise_compute::types::AircraftSegment {
                flight_id: fid,
                profile_idx: piece.profile_idx,
                is_departure: true,
                on_ground: false,
                period: 0,
                date_id: 0,
                start_lat: s_lat as f64,
                start_lon: s_lon as f64,
                start_alt_m: 1_000.0,
                end_lat: e_lat as f64,
                end_lon: e_lon as f64,
                end_alt_m: 1_000.0,
                speed_kt: 350.0,
                segment_length_m: piece.length_m,
                count_weight: 1.0,
                surface_model: false,
                ground_context: aircraft::GROUND_CONTEXT_NONE,
                ground_ops_kind: aircraft::GROUND_OPS_KIND_NONE,
                source_id: 2,
            };
            let mut prepared = aircraft::prepare_segment(&segment, -30.0, -30.0);
            prepared.reach_sq = f64::INFINITY;
            let row = aircraft::prepare_row(&prepared, 0.0, aircraft::M_PER_DEG_LAT);
            aircraft::segment_sel_at_pixel(&prepared, &row, 0.0, rx_elev, luts, Some(&horizon))
                .map(|(sel, _)| sel)
                .unwrap_or(f64::NEG_INFINITY)
        })
        .collect();
    // A piece the floored kernel refuses (-inf) is below its 20 dB floor.
    let dropped: f64 = own_sel[1..].iter().map(|&sel| 10f64.powf(sel / 10.0)).sum();
    let accounted = 10.0 * (10f64.powf(kept_sel / 10.0) + dropped).log10();
    let remainder = 10.0 * (10f64.powf(whole_sel / 10.0) - 10f64.powf(accounted / 10.0)).log10();
    eprintln!(
        "beyond-reach bound: whole {whole_sel:.2} dB, kept {kept_sel:.2} dB, pieces {own_sel:.2?} dB, kept + dropped {accounted:.2} dB, remainder {remainder:.2} dB"
    );
    assert!((whole_sel - 33.58).abs() < 0.05, "{whole_sel}");
    assert!((kept_sel - 30.92).abs() < 0.05, "{kept_sel}");
    assert!(
        (own_sel[0] - kept_sel).abs() < 1e-6,
        "the kept piece is the first one"
    );
    assert!(
        own_sel[1..]
            .iter()
            .all(|&sel| sel < aircraft::AIRCRAFT_NPD_REACH_THRESHOLD_DB && sel < own_sel[0]),
        "{own_sel:?}"
    );
    // The loss is the dropped pieces' own energy: what the evaluated pieces
    // do not account for is one piece below the kernel's 20 dB floor.
    assert!(accounted <= whole_sel, "{accounted} vs {whole_sel}");
    assert!(remainder < 20.0, "unaccounted {remainder} dB");
    assert_eq!(
        split.traces.segments.len(),
        1,
        "only the piece in reach is drawn"
    );
}

//! Producer IPC, batch selection and actual popup must agree on periodic airborne geometry.

use super::AirborneRowAccum;
use aircraft_extract::{arrow_io, flight::*};
use noise_compute::{compute::aircraft_v6::*, emission::aircraft, types::*};

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

/// A 1 km chord at the equator between the two longitudes: the periodic
/// selection is about the arc, not its stored length.
fn segment(start_lon: f32, end_lon: f32) -> FlightSegment {
    FlightSegment {
        flight_id: 42,
        callsign: "PERIODIC42".into(),
        aircraft_type: *b"B738",
        profile_idx: noise_compute::emission::profiles_generated::profile_idx("B738"),
        source_id: 2,
        origin: 0,
        veh_kind: 0,
        gse_class: 0,
        period: 0,
        date_id: 0,
        phase: Phase::Airborne,
        flags: 1,
        start_lat: 0.0,
        start_lon,
        start_alt_m: 1000.0,
        end_lat: 0.0,
        end_lon,
        end_alt_m: 1000.0,
        speed_kt: 450.0,
        length_m: 1000.0,
        agl_avg_m: 1000.0,
        start_elev_m: 0.0,
        end_elev_m: 0.0,
    }
}

fn output(receiver: &Receiver, rows: &[AirborneSegmentBatch<'_>]) -> serde_json::Value {
    let horizon = aircraft::ReceiverHorizon::build(
        |_, _| 0.0,
        receiver.lat,
        receiver.lon,
        receiver.altitude_m(),
    );
    let (periods, contributors, bands) = compute_aircraft_v6(
        receiver,
        rows,
        &[],
        &FlatGround,
        Some(&horizon),
        None,
        12,
        &aircraft::ClassWeights::uniform(),
        0,
        None,
        None,
    );
    serde_json::json!({"periods": periods, "contributors": contributors, "bands": bands.airborne})
}

#[test]
fn periodic_producer_batches_preserve_positive_seam_flights_and_row_identity() {
    for (segments, receiver_lon, selected, positive) in [
        (vec![segment(179.0, -179.0)], 179.5, true, true),
        (vec![segment(179.0, -179.0)], -179.5, true, true),
        (vec![segment(-1.0, 1.0)], -0.5, true, true),
        (vec![segment(-179.85, -179.75)], 180.0, false, false),
        (vec![segment(0.15, 0.25)], 0.0, false, false),
        // Wide aggregates can contain unrelated local and seam segments.
        (
            vec![segment(179.0, -179.0), segment(-0.01, 0.01)],
            0.0,
            true,
            true,
        ),
        (vec![segment(179.0, -179.0)], 0.0, true, false),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("airborne.arrow");
        // Stored as the shuffle stores it: the 222 km chords become pieces,
        // one of which crosses the seam with a wide envelope of its own.
        let mut pieces = Vec::new();
        for segment in &segments {
            aircraft_extract::segment::split::split_airborne_segment(segment.clone(), &mut pieces);
        }
        arrow_io::write_airborne(&path, &pieces, 12, 0).unwrap();
        let (_, batches) = arrow_io::read_record_batches(&path).unwrap();
        let all = AirborneRowAccum::new(&batches).unwrap();
        let rows = all.views();
        assert_eq!(airborne_row_count(rows), pieces.len());
        let first = &rows[0];
        assert_eq!(
            (
                first.flight_id[0],
                first.flights.callsign(first.flight_key[0] as usize)
            ),
            (42, "PERIODIC42")
        );
        let receiver = Receiver::new(0.001, receiver_lon, 0.0);
        let square = square_store::store::load_square(directory.path()).unwrap();
        let collected = crate::query::collect_from_square_data(
            &[(grid::square_of(receiver.lat, receiver.lon), &square)],
            receiver.lat,
            receiver.lon,
        )
        .unwrap();
        assert_eq!(
            !collected.aircraft_airborne_batches.is_empty(),
            selected,
            "receiver={receiver_lon}"
        );
        let filtered = AirborneRowAccum::new(&collected.aircraft_airborne_batches).unwrap();
        let actual = output(&receiver, filtered.views());
        assert_eq!(actual, output(&receiver, rows));
        assert_eq!(
            actual["periods"]["lden_db"].is_number(),
            positive,
            "receiver={receiver_lon}, output={actual}"
        );
    }
}

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

fn segment(start_lon: f32, end_lon: f32) -> AirborneSubSegment {
    AirborneSubSegment {
        start_lat: 0.0,
        start_lon,
        end_lat: 0.0,
        end_lon,
        start_alt_m: 1000.0,
        end_alt_m: 1000.0,
        speed_kt: 450.0,
        length_m: 221080.0,
        period: 0,
        date_id: 0,
        flags: 1,
        terrain_start_elev_m: 0.0,
        terrain_end_elev_m: 0.0,
    }
}

fn output(receiver: &Receiver, rows: &[AirborneRowView<'_>]) -> serde_json::Value {
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
        arrow_io::write_airborne(
            &path,
            &[AirborneEvent {
                flight_id: 42,
                callsign: "PERIODIC42".into(),
                aircraft_type: *b"B738",
                profile_idx: noise_compute::emission::profiles_generated::profile_idx("B738"),
                source_id: 2,
                origin: 0,
                sub_segments: segments.clone(),
            }],
            12,
            0,
        )
        .unwrap();
        let (_, batches) = arrow_io::read_record_batches(&path).unwrap();
        let all = AirborneRowAccum::new(&batches).unwrap();
        let rows = all.views();
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].flight_id, rows[0].callsign), (42, "PERIODIC42"));
        assert_eq!(rows[0].sub_segments.len(), segments.len());
        let receiver = Receiver::new(0.001, receiver_lon, 0.0);
        let square = square_store::store::load_square(directory.path()).unwrap();
        let collected = crate::query::collect_from_square_data(
            &[(grid::square_of(receiver.lat, receiver.lon), &square)],
            receiver.lat,
            receiver.lon,
        )
        .unwrap();
        assert_eq!(
            collected.aircraft_airborne_batches.len(),
            usize::from(selected)
        );
        let filtered = AirborneRowAccum::new(&collected.aircraft_airborne_batches).unwrap();
        let actual = output(&receiver, &filtered.views());
        assert_eq!(actual, output(&receiver, &rows));
        assert_eq!(
            actual["periods"]["lden_db"].is_number(),
            positive,
            "receiver={receiver_lon}, output={actual}"
        );
        for index in 0..rows[0].sub_segments.len() {
            let sub = rows[0].sub_segments;
            let start = [sub.start_lat[index], sub.start_lon[index]];
            let end = [sub.end_lat[index], sub.end_lon[index]];
            if aircraft::AirborneEnvelope::new(receiver.lat, receiver.lon)
                .intersects_segment(start, end)
            {
                assert!(aircraft::airborne_support_cells(start, end)
                    .unwrap()
                    .contains(grid::square_of(receiver.lat, receiver.lon)));
            }
        }
    }
}

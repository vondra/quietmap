//! Stage 2A airborne writer: one sub-segment row per line, flight identity as one dictionary.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use arrow::array::{
    ArrayRef, DictionaryArray, FixedSizeBinaryBuilder, Float32Builder, Int16Builder, Int32Builder,
    StringBuilder, StructArray, UInt64Builder, UInt8Builder,
};
use arrow::datatypes::Int32Type;
use noise_compute::emission::aircraft::AIRBORNE_SUB_SEGMENT_MAX_LENGTH_M;

use crate::arrow_schemas;
use crate::flight::{segment_flags, FlightSegment, Phase};

use super::write_record_batches;

/// Write the square's airborne sub-segments (`Phase::Airborne`, aircraft only,
/// each no longer than `AIRBORNE_SUB_SEGMENT_MAX_LENGTH_M` — the reader pad
/// counts on it). `n_days` (airline window) + `ga_n_days` (GA-class window,
/// 0 = single-window) stamp the GA hybrid metadata so the popup/heatmap weight
/// GA rows at `1/ga_n_days`. The `flight` dictionary lists distinct flights in
/// order of first appearance; arrow's file writer refuses a replaced
/// dictionary, so it is built once for the whole file.
pub fn write_airborne(
    path: &Path,
    rows: &[FlightSegment],
    n_days: u16,
    ga_n_days: u16,
) -> Result<()> {
    let schema =
        arrow_schemas::with_n_days_and_windows(arrow_schemas::airborne_schema(), n_days, ga_n_days);
    let n = rows.len();
    let mut flight_id = UInt64Builder::with_capacity(n);
    let mut flight_key = Int32Builder::with_capacity(n);
    let mut key_of_flight: HashMap<u64, i32> = HashMap::new();
    let mut callsign = StringBuilder::new();
    let mut aircraft_type = FixedSizeBinaryBuilder::new(4);
    let mut profile_idx = UInt8Builder::new();
    let mut source_id = UInt8Builder::new();
    let mut origin = UInt8Builder::new();
    let mut sgx = Int32Builder::with_capacity(n);
    let mut sgy = Int32Builder::with_capacity(n);
    let mut sal = Int16Builder::with_capacity(n);
    let mut egx = Int32Builder::with_capacity(n);
    let mut egy = Int32Builder::with_capacity(n);
    let mut eal = Int16Builder::with_capacity(n);
    let mut speed = Float32Builder::with_capacity(n);
    let mut length = Float32Builder::with_capacity(n);
    let mut period = UInt8Builder::with_capacity(n);
    let mut date_id = Int16Builder::with_capacity(n);
    let mut flags = UInt8Builder::with_capacity(n);
    let mut t_start = Int16Builder::with_capacity(n);
    let mut t_end = Int16Builder::with_capacity(n);
    // Block pruning compares the batch envelope against the f32 endpoints the
    // reader physics consumes; f32→f64 is exact, so bounding the decoded
    // values bounds exactly what the reader tests.
    let mut row_bboxes = Vec::with_capacity(n);
    let mut row_altitudes = Vec::with_capacity(n);
    for r in rows {
        anyhow::ensure!(
            r.phase == Phase::Airborne && r.veh_kind == 0,
            "airborne.arrow takes aircraft airborne rows only (flight {})",
            r.flight_id
        );
        // The pad bounds the stored geometry, not the source length: near
        // the poles the Mercator clamp stretches a short chord.
        anyhow::ensure!(
            crate::support::airborne_stored_length_within_cap(r),
            "airborne sub-segment of flight {} stores {:?} m of geometry, longer than the {} m cap the reader pad assumes; rerun shuffle",
            r.flight_id,
            crate::support::airborne_stored_length_m(r),
            AIRBORNE_SUB_SEGMENT_MAX_LENGTH_M
        );
        flight_id.append_value(r.flight_id);
        let next_key = i32::try_from(key_of_flight.len())?;
        let key = *key_of_flight.entry(r.flight_id).or_insert(next_key);
        if key == next_key {
            callsign.append_value(&r.callsign);
            aircraft_type.append_value(r.aircraft_type)?;
            profile_idx.append_value(r.profile_idx);
            source_id.append_value(r.source_id);
            origin.append_value(r.origin);
        }
        flight_key.append_value(key);
        let mut bounds = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        for (lat, lon) in [(r.start_lat, r.start_lon), (r.end_lat, r.end_lon)] {
            let [lat, lon] = crate::support::airborne_decoded_endpoint(lat, lon)
                .ok_or_else(|| anyhow::anyhow!("invalid airborne endpoint"))?;
            let (lat, lon) = (f64::from(lat), f64::from(lon));
            bounds[0] = bounds[0].min(lat);
            bounds[1] = bounds[1].min(lon);
            bounds[2] = bounds[2].max(lat);
            bounds[3] = bounds[3].max(lon);
        }
        row_bboxes.push(bounds);
        row_altitudes.push([
            r.start_alt_m.min(r.end_alt_m),
            r.start_alt_m.max(r.end_alt_m),
        ]);
        let (gx, gy) = grid::lonlat_to_grid(r.start_lon as f64, r.start_lat as f64);
        sgx.append_value(gx);
        sgy.append_value(gy);
        sal.append_value(super::height_meters(r.start_alt_m)?);
        let (gx, gy) = grid::lonlat_to_grid(r.end_lon as f64, r.end_lat as f64);
        egx.append_value(gx);
        egy.append_value(gy);
        eal.append_value(super::height_meters(r.end_alt_m)?);
        speed.append_value(r.speed_kt);
        length.append_value(r.length_m);
        period.append_value(r.period);
        date_id.append_value(r.date_id);
        flags.append_value(
            r.flags
                & (segment_flags::IS_DEPARTURE
                    | segment_flags::SPLIT_PIECE
                    | segment_flags::CHORD_START
                    | segment_flags::CHORD_END),
        );
        t_start.append_value(super::height_meters(r.start_elev_m)?);
        t_end.append_value(super::height_meters(r.end_elev_m)?);
    }
    let flights = StructArray::new(
        arrow_schemas::airborne_flight_fields(),
        vec![
            Arc::new(callsign.finish()) as ArrayRef,
            Arc::new(aircraft_type.finish()),
            Arc::new(profile_idx.finish()),
            Arc::new(source_id.finish()),
            Arc::new(origin.finish()),
        ],
        None,
    );
    let flight = DictionaryArray::<Int32Type>::try_new(flight_key.finish(), Arc::new(flights))?;
    let columns: Vec<ArrayRef> = vec![
        Arc::new(flight_id.finish()),
        Arc::new(flight),
        Arc::new(sgx.finish()),
        Arc::new(sgy.finish()),
        Arc::new(sal.finish()),
        Arc::new(egx.finish()),
        Arc::new(egy.finish()),
        Arc::new(eal.finish()),
        Arc::new(speed.finish()),
        Arc::new(length.finish()),
        Arc::new(period.finish()),
        Arc::new(date_id.finish()),
        Arc::new(flags.finish()),
        Arc::new(t_start.finish()),
        Arc::new(t_end.finish()),
    ];
    let (schema, batches) = arrow_batching::blocked_by_z14_cell_with_altitude(
        schema.as_ref().clone(),
        columns,
        &row_bboxes,
        &row_altitudes,
    )?;
    write_record_batches(path, &schema, &batches)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arrow_io::read_record_batches;
    use arrow::array::{Array, FixedSizeBinaryArray, Int16Array, StringArray, UInt8Array};
    use tempfile::tempdir;

    /// Two rows of one flight and one of another: the identity is stored once
    /// per flight and every field survives write → read.
    #[test]
    fn airborne_round_trip_stores_identity_once_per_flight() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("airborne.arrow");
        let mut rows = vec![
            FlightSegment::airborne_fixture(7, 50.0, 14.0),
            FlightSegment::airborne_fixture(7, 50.01, 14.01),
            FlightSegment::airborne_fixture(9, 50.02, 14.02),
        ];
        rows[0].callsign = "TVS100P".into();
        rows[1].callsign = "TVS100P".into();
        rows[2].callsign = "CSA1".into();
        rows[2].aircraft_type = *b"B738";
        rows[2].flags = segment_flags::IS_DEPARTURE | segment_flags::SYNTHETIC;
        write_airborne(&p, &rows, 1, 0).unwrap();
        let (_, batches) = read_record_batches(&p).unwrap();
        assert_eq!(batches.iter().map(|b| b.num_rows()).sum::<usize>(), 3);
        let mut seen = Vec::new();
        for batch in &batches {
            let flight = batch
                .column_by_name("flight")
                .unwrap()
                .as_any()
                .downcast_ref::<DictionaryArray<Int32Type>>()
                .unwrap();
            let identity = flight
                .values()
                .as_any()
                .downcast_ref::<StructArray>()
                .unwrap();
            assert_eq!(identity.len(), 2, "one dictionary entry per flight");
            let callsigns = identity
                .column_by_name("callsign")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            let types = identity
                .column_by_name("aircraft_type")
                .unwrap()
                .as_any()
                .downcast_ref::<FixedSizeBinaryArray>()
                .unwrap();
            let flags = batch
                .column_by_name("flags")
                .unwrap()
                .as_any()
                .downcast_ref::<UInt8Array>()
                .unwrap();
            let t_start = batch
                .column_by_name("terrain_start_elev_m")
                .unwrap()
                .as_any()
                .downcast_ref::<Int16Array>()
                .unwrap();
            for i in 0..batch.num_rows() {
                let key = flight.keys().value(i) as usize;
                seen.push((
                    callsigns.value(key).to_string(),
                    types.value(key).to_vec(),
                    flags.value(i),
                    t_start.value(i),
                ));
            }
        }
        seen.sort();
        assert_eq!(
            seen,
            [
                ("CSA1".to_string(), b"B738".to_vec(), 1, 250),
                ("TVS100P".to_string(), b"A320".to_vec(), 0, 250),
                ("TVS100P".to_string(), b"A320".to_vec(), 0, 250),
            ]
        );
    }

    #[test]
    fn writer_refuses_rows_the_reader_pad_cannot_cover() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("airborne.arrow");
        let mut long = FlightSegment::airborne_fixture(1, 50.0, 14.0);
        long.end_lon = 14.0 + 4_001.0 / (111_320.0 * 50.0_f32.to_radians().cos());
        let error = write_airborne(&p, &[long], 1, 0).unwrap_err();
        assert!(error.to_string().contains("rerun shuffle"), "{error}");
        let mut ground = FlightSegment::airborne_fixture(1, 50.0, 14.0);
        ground.phase = Phase::Ground;
        assert!(write_airborne(&p, &[ground], 1, 0).is_err());
        assert!(!p.exists());
    }
}

//! Regression tests for segments behavior.

use super::*;
use crate::flight::Phase;
use tempfile::tempdir;

#[test]
fn malformed_or_null_segment_columns_fail_without_panicking() {
    let schema = arrow_schemas::segments_schema();
    let empty = RecordBatch::new_empty(Arc::new(Schema::new_with_metadata(
        Vec::<arrow::datatypes::Field>::new(),
        schema.metadata().clone(),
    )));
    assert!(segments_from_batch(&empty).is_err());
    let batch = build_segments_batch(&[seg_with_id(1)], &schema).unwrap();
    let index = schema.index_of("start_lat").unwrap();
    let mut fields: Vec<_> = schema
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect();
    fields[index] =
        arrow::datatypes::Field::new("start_lat", arrow::datatypes::DataType::Float32, true);
    let mut columns = batch.columns().to_vec();
    columns[index] = Arc::new(Float32Array::from(vec![None]));
    let null = RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).unwrap();
    assert!(segments_from_batch(&null).is_err());
}

#[test]
fn segments_round_trip() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("segments.arrow");
    let segs = vec![FlightSegment {
        flight_id: 0xDEAD_BEEF,
        callsign: "TVS100P".into(),
        aircraft_type: *b"A320",
        profile_idx: 0,
        source_id: 0,
        origin: 0,
        // Distinct non-zero values catch builder/schema column-order
        // transposition (would otherwise round-trip identically when
        // both fields happen to default to 0).
        veh_kind: 1,
        gse_class: 2,
        period: 1,
        date_id: 1234,
        phase: Phase::Airborne,
        flags: 0b011,
        start_lat: 50.0,
        start_lon: 14.0,
        start_alt_m: 1000.0,
        end_lat: 50.001,
        end_lon: 14.001,
        end_alt_m: 1100.0,
        speed_kt: 250.0,
        length_m: 300.0,
        agl_avg_m: 700.0,
        start_elev_m: 250.0,
        end_elev_m: 280.0,
    }];
    write_segments(&p, &segs).unwrap();
    let read = read_segments(&p).unwrap();
    assert_eq!(read.len(), 1);
    let r = &read[0];
    assert_eq!(r.flight_id, 0xDEAD_BEEF);
    assert_eq!(r.callsign, "TVS100P");
    assert_eq!(&r.aircraft_type, b"A320");
    assert_eq!(r.phase, Phase::Airborne);
    assert!((r.length_m - 300.0).abs() < 1e-3);
    assert_eq!(r.veh_kind, 1);
    assert_eq!(r.gse_class, 2);
    assert!((r.start_elev_m - 250.0).abs() < 1e-3);
    assert!((r.end_elev_m - 280.0).abs() < 1e-3);
}

fn seg_with_id(id: u64) -> FlightSegment {
    FlightSegment {
        flight_id: id,
        callsign: String::new(),
        aircraft_type: *b"A320",
        profile_idx: 0,
        source_id: 0,
        origin: 0,
        veh_kind: 0,
        gse_class: 0,
        period: 0,
        date_id: 0,
        phase: Phase::Cruise,
        flags: 0,
        start_lat: 0.0,
        start_lon: 0.0,
        start_alt_m: 0.0,
        end_lat: 0.0,
        end_lon: 0.0,
        end_alt_m: 0.0,
        speed_kt: 0.0,
        length_m: 0.0,
        agl_avg_m: 0.0,
        start_elev_m: 0.0,
        end_elev_m: 0.0,
    }
}

/// A multi-batch file (the shape Stage 2B's streaming reader needs to
/// bound memory) must round-trip in original row order, and
/// `for_each_segment_batch` must yield one Vec per batch.
#[test]
fn write_chunks_into_streamable_batches() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("seg.arrow");
    let segs: Vec<FlightSegment> = (0..5u64).map(seg_with_id).collect();
    write_segments_chunked(&p, &segs, 2).unwrap(); // 5 rows / chunk 2 → 3 batches

    let mut nbatch = 0;
    for_each_batch(&p, |_| {
        nbatch += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(nbatch, 3, "5 rows / chunk 2 → 3 record batches");

    let ids: Vec<u64> = read_segments(&p)
        .unwrap()
        .iter()
        .map(|s| s.flight_id)
        .collect();
    assert_eq!(
        ids,
        vec![0, 1, 2, 3, 4],
        "read_segments concatenates in row order"
    );

    let mut streamed = Vec::new();
    for_each_segment_batch(&p, |b| {
        streamed.extend(b.iter().map(|s| s.flight_id));
        Ok(())
    })
    .unwrap();
    assert_eq!(streamed, vec![0, 1, 2, 3, 4]);
}

/// A single large batch (the legacy ~100M-row shard shape) must decode in
/// row-slices — preserving order across slice boundaries — so it never
/// materialises whole.
#[test]
fn single_batch_decodes_in_ordered_slices() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("seg.arrow");
    let segs: Vec<FlightSegment> = (0..5u64).map(seg_with_id).collect();
    write_segments_chunked(&p, &segs, 1000).unwrap(); // one batch of 5 rows

    let mut nbatch = 0;
    for_each_batch(&p, |_| {
        nbatch += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(nbatch, 1, "5 rows / chunk 1000 → one batch");

    let mut slices = 0;
    let mut ids = Vec::new();
    for_each_segment_slice(&p, 2, |s| {
        slices += 1;
        ids.extend(s.iter().map(|x| x.flight_id));
        Ok(())
    })
    .unwrap();
    assert_eq!(slices, 3, "one 5-row batch / slice 2 → 3 decode slices");
    assert_eq!(ids, vec![0, 1, 2, 3, 4]);
}

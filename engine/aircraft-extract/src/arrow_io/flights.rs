//! Stage 0 flights writer.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use arrow::array::{
    ArrayRef, FixedSizeBinaryBuilder, Float32Builder, Float64Builder, ListArray, StringBuilder,
    StructArray, UInt64Builder, UInt8Builder,
};
use arrow::buffer::OffsetBuffer;
use arrow::datatypes::{DataType, Field};
use arrow::record_batch::RecordBatch;

use crate::arrow_schemas;
use crate::trace::TracePoint;

use super::write_record_batches;

pub struct FlightRow<'a> {
    pub flight_id: u64,
    pub callsign: &'a str,
    pub aircraft_type: &'a [u8; 4],
    pub profile_idx: u8,
    pub source_id: u8,
    pub origin: u8,
    pub veh_kind: u8,
    pub gse_class: u8,
    pub base_timestamp: f64,
    pub points: &'a [TracePoint],
}

pub fn write_flights(path: &Path, rows: &[FlightRow<'_>]) -> Result<()> {
    let schema = arrow_schemas::flights_schema();
    let mut flight_id = UInt64Builder::with_capacity(rows.len());
    let mut callsign = StringBuilder::new();
    let mut atype = FixedSizeBinaryBuilder::with_capacity(rows.len(), 4);
    let mut profile_idx = UInt8Builder::with_capacity(rows.len());
    let mut source_id = UInt8Builder::with_capacity(rows.len());
    let mut origin = UInt8Builder::with_capacity(rows.len());
    let mut veh_kind = UInt8Builder::with_capacity(rows.len());
    let mut gse_class = UInt8Builder::with_capacity(rows.len());
    let mut base_ts = Float64Builder::with_capacity(rows.len());

    let pt_struct_field = match schema.field_with_name("points")?.data_type() {
        DataType::List(item) => match item.data_type() {
            DataType::Struct(f) => f.clone(),
            other => anyhow::bail!("flights points list element not Struct (got {other:?})"),
        },
        other => anyhow::bail!("flights points field not List (got {other:?})"),
    };

    let mut pt_offsets: Vec<i32> = Vec::with_capacity(rows.len() + 1);
    pt_offsets.push(0);
    let mut total_pts = 0usize;
    for r in rows {
        flight_id.append_value(r.flight_id);
        callsign.append_value(r.callsign);
        atype.append_value(r.aircraft_type)?;
        profile_idx.append_value(r.profile_idx);
        source_id.append_value(r.source_id);
        origin.append_value(r.origin);
        veh_kind.append_value(r.veh_kind);
        gse_class.append_value(r.gse_class);
        base_ts.append_value(r.base_timestamp);
        total_pts += r.points.len();
        pt_offsets.push(total_pts as i32);
    }

    let mut ts_off = Float32Builder::with_capacity(total_pts);
    let mut lat = Float32Builder::with_capacity(total_pts);
    let mut lon = Float32Builder::with_capacity(total_pts);
    let mut alt_ft = Float32Builder::with_capacity(total_pts);
    let mut speed = Float32Builder::with_capacity(total_pts);
    let mut track = Float32Builder::with_capacity(total_pts);
    let mut baro = Float32Builder::with_capacity(total_pts);
    let mut flags = UInt8Builder::with_capacity(total_pts);
    for r in rows {
        for pt in r.points {
            ts_off.append_value((pt.timestamp - r.base_timestamp) as f32);
            lat.append_value(pt.lat);
            lon.append_value(pt.lon);
            alt_ft.append_value(pt.alt_ft);
            speed.append_value(pt.speed_kt);
            track.append_value(pt.track_deg);
            baro.append_value(pt.baro_rate_fpm);
            flags.append_value(pt.flags);
        }
    }
    let pt_struct = StructArray::new(
        pt_struct_field,
        vec![
            Arc::new(ts_off.finish()) as ArrayRef,
            Arc::new(lat.finish()),
            Arc::new(lon.finish()),
            Arc::new(alt_ft.finish()),
            Arc::new(speed.finish()),
            Arc::new(track.finish()),
            Arc::new(baro.finish()),
            Arc::new(flags.finish()),
        ],
        None,
    );

    let pts_field = match schema.field_with_name("points")?.data_type() {
        DataType::List(item) => Arc::new(Field::new(
            item.name(),
            DataType::Struct(match item.data_type() {
                DataType::Struct(f) => f.clone(),
                _ => unreachable!(),
            }),
            item.is_nullable(),
        )),
        _ => unreachable!(),
    };
    let points_list = ListArray::new(
        pts_field,
        OffsetBuffer::new(arrow::buffer::ScalarBuffer::from(pt_offsets)),
        Arc::new(pt_struct),
        None,
    );

    let columns: Vec<ArrayRef> = vec![
        Arc::new(flight_id.finish()),
        Arc::new(callsign.finish()),
        Arc::new(atype.finish()),
        Arc::new(profile_idx.finish()),
        Arc::new(source_id.finish()),
        Arc::new(origin.finish()),
        Arc::new(veh_kind.finish()),
        Arc::new(gse_class.finish()),
        Arc::new(base_ts.finish()),
        Arc::new(points_list),
    ];
    let batch = RecordBatch::try_new(schema.clone(), columns)?;
    write_record_batches(path, &schema, &[batch])
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn flights_round_trip() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("flights.arrow");
        let pts = vec![TracePoint {
            timestamp: 1_700_000_010.0,
            lat: 50.0,
            lon: 14.0,
            alt_ft: 5000.0,
            speed_kt: 250.0,
            track_deg: 90.0,
            baro_rate_fpm: 0.0,
            flags: 0,
        }];
        let rows = vec![FlightRow {
            flight_id: 1,
            callsign: "TEST",
            aircraft_type: b"B738",
            profile_idx: 0,
            source_id: 0,
            origin: 0,
            // Non-zero distinct values catch a column-order transposition
            // between schema and builder list (veh_kind would otherwise
            // pass the round-trip if both fields happen to be 0).
            veh_kind: 1,
            gse_class: 2,
            base_timestamp: 1_700_000_000.0,
            points: &pts,
        }];
        write_flights(&p, &rows).unwrap();
        // End-to-end round-trip via the typed reader so writer + reader
        // are exercised together — catches column-rename / transposition
        // / type-mismatch bugs that a raw batch read would miss.
        let flights = crate::stage_1::read_flights(&p).unwrap();
        assert_eq!(flights.len(), 1);
        assert_eq!(flights[0].veh_kind, 1);
        assert_eq!(flights[0].gse_class, 2);
    }
}

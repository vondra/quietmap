//! Stage 2A airborne writer.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use arrow::array::{
    Array, ArrayRef, FixedSizeBinaryBuilder, Float32Builder, Int16Builder, Int32Builder, ListArray,
    StringBuilder, StructArray, UInt64Builder, UInt8Builder,
};
use arrow::buffer::OffsetBuffer;
use arrow::datatypes::{DataType, Field};

use crate::arrow_schemas;
use crate::flight::AirborneEvent;

use super::write_record_batches;

/// `n_days` (airline window) + `ga_n_days` (GA-class window, 0 =
/// single-window) stamp the GA hybrid metadata so the popup/heatmap
/// weight GA rows at `1/ga_n_days`.
pub fn write_airborne(
    path: &Path,
    rows: &[AirborneEvent],
    n_days: u16,
    ga_n_days: u16,
) -> Result<()> {
    let schema =
        arrow_schemas::with_n_days_and_windows(arrow_schemas::airborne_schema(), n_days, ga_n_days);
    let n = rows.len();
    let mut flight_id = UInt64Builder::with_capacity(n);
    let mut callsign = StringBuilder::with_capacity(n, 8 * n);
    let mut aircraft_type = FixedSizeBinaryBuilder::with_capacity(n, 4);
    let mut profile_idx = UInt8Builder::with_capacity(n);
    let mut source_id = UInt8Builder::with_capacity(n);
    let mut origin = UInt8Builder::with_capacity(n);
    // Popup batch pruning: the per-event bbox doubles as the spatial-sort key
    // f32→f64 is exact, so the batch bbox
    // bounds the f32 coordinates the reader tests against.
    let mut row_bboxes = Vec::with_capacity(n);

    let sub_struct_fields = match schema.field_with_name("sub_segments")?.data_type() {
        DataType::List(item) => match item.data_type() {
            DataType::Struct(f) => f.clone(),
            other => anyhow::bail!("airborne sub_segments item not Struct ({other:?})"),
        },
        _ => unreachable!(),
    };
    let mut sub_off: Vec<i32> = Vec::with_capacity(n + 1);
    sub_off.push(0);
    let mut total_subs = 0usize;
    for r in rows {
        flight_id.append_value(r.flight_id);
        callsign.append_value(&r.callsign);
        aircraft_type.append_value(r.aircraft_type)?;
        profile_idx.append_value(r.profile_idx);
        source_id.append_value(r.source_id);
        origin.append_value(r.origin);
        anyhow::ensure!(
            !r.sub_segments.is_empty(),
            "airborne event {} has no geometry",
            r.flight_id
        );
        let mut bounds = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        for segment in &r.sub_segments {
            for (lat, lon) in [
                (segment.start_lat, segment.start_lon),
                (segment.end_lat, segment.end_lon),
            ] {
                anyhow::ensure!(
                    lat.is_finite() && lon.is_finite() && (-90.0..=90.0).contains(&lat),
                    "invalid airborne endpoint"
                );
                let (gx, gy) = grid::lonlat_to_grid(f64::from(lon), f64::from(lat));
                let (lon, lat) = square_store::grid_cols::grid_cell_lonlat(gx, gy);
                // Reader physics consumes f32; prune against those exact decoded values.
                let (lat, lon) = (f64::from(lat as f32), f64::from(lon as f32));
                bounds[0] = bounds[0].min(lat);
                bounds[1] = bounds[1].min(lon);
                bounds[2] = bounds[2].max(lat);
                bounds[3] = bounds[3].max(lon);
            }
        }
        row_bboxes.push(bounds);
        total_subs += r.sub_segments.len();
        sub_off.push(i32::try_from(total_subs)?);
    }

    let mut sla = Int32Builder::with_capacity(total_subs);
    let mut slo = Int32Builder::with_capacity(total_subs);
    let mut sal = Int16Builder::with_capacity(total_subs);
    let mut ela = Int32Builder::with_capacity(total_subs);
    let mut elo = Int32Builder::with_capacity(total_subs);
    let mut eal = Int16Builder::with_capacity(total_subs);
    let mut speed = Float32Builder::with_capacity(total_subs);
    let mut length = Float32Builder::with_capacity(total_subs);
    let mut period = UInt8Builder::with_capacity(total_subs);
    let mut date_id = Int16Builder::with_capacity(total_subs);
    let mut flags = UInt8Builder::with_capacity(total_subs);
    let mut t_start = Int16Builder::with_capacity(total_subs);
    let mut t_end = Int16Builder::with_capacity(total_subs);
    for r in rows {
        for s in &r.sub_segments {
            let (gx, gy) = grid::lonlat_to_grid(s.start_lon as f64, s.start_lat as f64);
            sla.append_value(gx);
            slo.append_value(gy);
            sal.append_value(super::height_meters(s.start_alt_m)?);
            let (gx, gy) = grid::lonlat_to_grid(s.end_lon as f64, s.end_lat as f64);
            ela.append_value(gx);
            elo.append_value(gy);
            eal.append_value(super::height_meters(s.end_alt_m)?);
            speed.append_value(s.speed_kt);
            length.append_value(s.length_m);
            period.append_value(s.period);
            date_id.append_value(s.date_id);
            flags.append_value(s.flags);
            t_start.append_value(super::height_meters(s.terrain_start_elev_m)?);
            t_end.append_value(super::height_meters(s.terrain_end_elev_m)?);
        }
    }
    let sub_struct = StructArray::new(
        sub_struct_fields,
        vec![
            Arc::new(sla.finish()) as ArrayRef,
            Arc::new(slo.finish()),
            Arc::new(sal.finish()),
            Arc::new(ela.finish()),
            Arc::new(elo.finish()),
            Arc::new(eal.finish()),
            Arc::new(speed.finish()),
            Arc::new(length.finish()),
            Arc::new(period.finish()),
            Arc::new(date_id.finish()),
            Arc::new(flags.finish()),
            Arc::new(t_start.finish()),
            Arc::new(t_end.finish()),
        ],
        None,
    );
    let item_field = match schema.field_with_name("sub_segments")?.data_type() {
        DataType::List(item) => Arc::new(Field::new(
            item.name(),
            sub_struct.data_type().clone(),
            item.is_nullable(),
        )),
        _ => unreachable!(),
    };
    let sub_list = ListArray::new(
        item_field,
        OffsetBuffer::new(arrow::buffer::ScalarBuffer::from(sub_off)),
        Arc::new(sub_struct),
        None,
    );

    let columns: Vec<ArrayRef> = vec![
        Arc::new(flight_id.finish()),
        Arc::new(callsign.finish()),
        Arc::new(aircraft_type.finish()),
        Arc::new(profile_idx.finish()),
        Arc::new(source_id.finish()),
        Arc::new(origin.finish()),
        Arc::new(sub_list),
    ];
    let (schema, batches) =
        arrow_batching::spatially_batched(schema.as_ref().clone(), columns, &row_bboxes)?;
    write_record_batches(path, &schema, &batches)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arrow_io::read_record_batches;
    use crate::flight::AirborneSubSegment;
    use tempfile::tempdir;

    #[test]
    fn airborne_round_trip() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("airborne.arrow");
        let evs = vec![AirborneEvent {
            flight_id: 1,
            callsign: "TVS100P".into(),
            aircraft_type: *b"A320",
            profile_idx: 0,
            source_id: 0,
            origin: 0,
            sub_segments: vec![AirborneSubSegment {
                start_lat: 50.0,
                start_lon: 14.0,
                start_alt_m: 1000.0,
                end_lat: 50.001,
                end_lon: 14.001,
                end_alt_m: 1100.0,
                speed_kt: 250.0,
                length_m: 300.0,
                period: 0,
                date_id: 1,
                flags: 0,
                terrain_start_elev_m: 200.0,
                terrain_end_elev_m: 220.0,
            }],
        }];
        write_airborne(&p, &evs, 1, 0).unwrap();
        let (_, batches) = read_record_batches(&p).unwrap();
        assert_eq!(batches[0].num_rows(), 1);
        // Verify M1 columns survive write→read so a Stage 2A regression
        // dropping callsign / aircraft_type can't slip through with the
        // num_rows check still passing.
        let cs = batches[0]
            .column_by_name("callsign")
            .unwrap()
            .as_any()
            .downcast_ref::<arrow::array::StringArray>()
            .unwrap();
        let at = batches[0]
            .column_by_name("aircraft_type")
            .unwrap()
            .as_any()
            .downcast_ref::<arrow::array::FixedSizeBinaryArray>()
            .unwrap();
        assert_eq!(cs.value(0), "TVS100P");
        assert_eq!(at.value(0), b"A320");
        // The current shape keeps only start/end terrain elevations; the
        // removed q1/mid/q3 chord check is the SPEC §6.1 known gap.
        let sub_list = batches[0]
            .column_by_name("sub_segments")
            .unwrap()
            .as_any()
            .downcast_ref::<arrow::array::ListArray>()
            .unwrap();
        let sub_struct = sub_list
            .values()
            .as_any()
            .downcast_ref::<arrow::array::StructArray>()
            .unwrap();
        let t_start = sub_struct
            .column_by_name("terrain_start_elev_m")
            .unwrap()
            .as_any()
            .downcast_ref::<arrow::array::Int16Array>()
            .unwrap();
        let t_end = sub_struct
            .column_by_name("terrain_end_elev_m")
            .unwrap()
            .as_any()
            .downcast_ref::<arrow::array::Int16Array>()
            .unwrap();
        assert_eq!(t_start.value(0), 200);
        assert_eq!(t_end.value(0), 220);
        assert!(
            sub_struct.column_by_name("terrain_q1_elev_m").is_none(),
            "v16 must NOT carry q1"
        );
        assert!(
            sub_struct.column_by_name("terrain_mid_elev_m").is_none(),
            "v16 must NOT carry mid"
        );
        assert!(
            sub_struct.column_by_name("terrain_q3_elev_m").is_none(),
            "v16 must NOT carry q3"
        );
    }
}

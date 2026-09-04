//! Schema-checked Stage 0 Arrow decoding; malformed columns fail before indexed access.

use crate::{arrow_io::read_record_batches, flight::Flight, trace::TracePoint};
use anyhow::Result;
use std::path::Path;

/// Read a Stage 0 flights file back into [`Flight`] structs. Matches the
/// schema produced by [`crate::stage_0::write_flights_at`].
pub fn read_flights(path: &Path) -> Result<Vec<Flight>> {
    use arrow::array::{
        Array, FixedSizeBinaryArray, Float32Array, Float64Array, ListArray, StringArray,
        StructArray, UInt64Array, UInt8Array,
    };

    let (schema, batches) = read_record_batches(path)?;
    anyhow::ensure!(
        schema.fields() == crate::arrow_schemas::flights_schema().fields(),
        "incompatible flights schema: {}",
        path.display()
    );
    let mut out = Vec::new();
    for b in batches {
        anyhow::ensure!(
            b.columns().iter().all(|c| c.null_count() == 0),
            "null flight field: {}",
            path.display()
        );
        let flight_id = b
            .column_by_name("flight_id")
            .unwrap()
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        let callsign = b
            .column_by_name("callsign")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let atype = b
            .column_by_name("aircraft_type")
            .unwrap()
            .as_any()
            .downcast_ref::<FixedSizeBinaryArray>()
            .unwrap();
        let prof = b
            .column_by_name("profile_idx")
            .unwrap()
            .as_any()
            .downcast_ref::<UInt8Array>()
            .unwrap();
        let src = b
            .column_by_name("source_id")
            .unwrap()
            .as_any()
            .downcast_ref::<UInt8Array>()
            .unwrap();
        let orig = b
            .column_by_name("origin")
            .unwrap()
            .as_any()
            .downcast_ref::<UInt8Array>()
            .unwrap();
        let veh_kind = b
            .column_by_name("veh_kind")
            .unwrap()
            .as_any()
            .downcast_ref::<UInt8Array>()
            .unwrap();
        let gse_class = b
            .column_by_name("gse_class")
            .unwrap()
            .as_any()
            .downcast_ref::<UInt8Array>()
            .unwrap();
        let base_ts = b
            .column_by_name("base_timestamp")
            .unwrap()
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        let pts_list = b
            .column_by_name("points")
            .unwrap()
            .as_any()
            .downcast_ref::<ListArray>()
            .unwrap();
        let pts_struct = pts_list
            .values()
            .as_any()
            .downcast_ref::<StructArray>()
            .unwrap();
        anyhow::ensure!(
            pts_struct.null_count() == 0
                && pts_struct.columns().iter().all(|c| c.null_count() == 0),
            "null trace point field: {}",
            path.display()
        );
        let pt_ts = pts_struct
            .column(0)
            .as_any()
            .downcast_ref::<Float32Array>()
            .unwrap();
        let pt_lat = pts_struct
            .column(1)
            .as_any()
            .downcast_ref::<Float32Array>()
            .unwrap();
        let pt_lon = pts_struct
            .column(2)
            .as_any()
            .downcast_ref::<Float32Array>()
            .unwrap();
        let pt_alt = pts_struct
            .column(3)
            .as_any()
            .downcast_ref::<Float32Array>()
            .unwrap();
        let pt_speed = pts_struct
            .column(4)
            .as_any()
            .downcast_ref::<Float32Array>()
            .unwrap();
        let pt_track = pts_struct
            .column(5)
            .as_any()
            .downcast_ref::<Float32Array>()
            .unwrap();
        let pt_baro = pts_struct
            .column(6)
            .as_any()
            .downcast_ref::<Float32Array>()
            .unwrap();
        let pt_flags = pts_struct
            .column(7)
            .as_any()
            .downcast_ref::<UInt8Array>()
            .unwrap();
        let offsets = pts_list.value_offsets();

        for i in 0..b.num_rows() {
            let lo = offsets[i] as usize;
            let hi = offsets[i + 1] as usize;
            let mut points = Vec::with_capacity(hi - lo);
            let base = base_ts.value(i);
            for j in lo..hi {
                points.push(TracePoint {
                    timestamp: base + pt_ts.value(j) as f64,
                    lat: pt_lat.value(j),
                    lon: pt_lon.value(j),
                    alt_ft: pt_alt.value(j),
                    speed_kt: pt_speed.value(j),
                    track_deg: pt_track.value(j),
                    baro_rate_fpm: pt_baro.value(j),
                    flags: pt_flags.value(j),
                });
            }
            let bytes = atype.value(i);
            let aircraft_type = std::str::from_utf8(bytes)
                .unwrap_or("")
                .trim_end_matches(char::from(0))
                .to_string();
            out.push(Flight {
                flight_id: flight_id.value(i),
                callsign: callsign.value(i).to_string(),
                aircraft_type,
                profile_idx: prof.value(i),
                source_id: src.value(i),
                origin: orig.value(i),
                veh_kind: veh_kind.value(i),
                gse_class: gse_class.value(i),
                points,
            });
        }
    }
    Ok(out)
}

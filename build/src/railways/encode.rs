//! Encode split children and crossing approaches into the consumed railway contract.

use super::split::ChildRow;
use arrow_array::RecordBatch;
use arrow_array::{
    ArrayRef, Float32Array, Float64Array, Int32Array, UInt8Array, UInt16Array, UInt32Array,
};
use arrow_ipc::writer::FileWriter;
use arrow_schema::{Field, Schema};
use arrow_select::take::take;
use std::sync::Arc;

pub const CONTRACT_KEY: &str = "rail_traffic_contract";
const DROPPED: &[&str] = &["source_id"];
const TRAFFIC: &[&str] = &[
    "trains_passenger_day",
    "trains_passenger_evening",
    "trains_passenger_night",
    "trains_freight_day",
    "trains_freight_evening",
    "trains_freight_night",
    "passenger_status",
    "freight_status",
    "passenger_source_id",
    "freight_source_id",
    "passenger_matching",
    "freight_matching",
    "passenger_missing_calendar_estimate_day",
    "passenger_missing_calendar_estimate_evening",
    "passenger_missing_calendar_estimate_night",
    "freight_missing_calendar_estimate_day",
    "freight_missing_calendar_estimate_evening",
    "freight_missing_calendar_estimate_night",
];

pub(crate) struct Expanded {
    pub parent: u32,
    /// Domestic track evidence until `allocate_over_parallel_tracks` replaces it by the track's
    /// line share (`child.foreign` holds the neighbour-file evidence it weighs against).
    pub child: ChildRow,
    pub prior: super::merge::RowTraffic,
    pub osm_id: i64,
    pub corridor: String,
    pub rail_type: u8,
    pub usage: u8,
    pub service: u8,
    pub traffic_mode: u8,
    pub country_iso: [u8; 2],
}

pub fn encode_children(merged: &RecordBatch, children: &[Expanded]) -> Result<RecordBatch, String> {
    let indices = UInt32Array::from(
        children
            .iter()
            .map(|child| child.parent)
            .collect::<Vec<_>>(),
    );
    let mut fields = Vec::new();
    let mut columns: Vec<ArrayRef> = Vec::new();
    for field in merged.schema().fields() {
        if DROPPED.contains(&field.name().as_str()) || TRAFFIC.contains(&field.name().as_str()) {
            continue;
        }
        let replaced = match field.name().as_str() {
            "start_gx" => i32_col(children, |c| c.child.geom.start_gx),
            "start_gy" => i32_col(children, |c| c.child.geom.start_gy),
            "end_gx" => i32_col(children, |c| c.child.geom.end_gx),
            "end_gy" => i32_col(children, |c| c.child.geom.end_gy),
            "length_m" => Arc::new(Float32Array::from(
                children
                    .iter()
                    .map(|c| c.child.geom.length_m)
                    .collect::<Vec<_>>(),
            )) as ArrayRef,
            _ => take(
                merged.column_by_name(field.name()).unwrap().as_ref(),
                &indices,
                None,
            )
            .map_err(|e| e.to_string())?,
        };
        fields.push(Field::new(
            field.name(),
            replaced.data_type().clone(),
            field.is_nullable(),
        ));
        columns.push(replaced);
    }
    append_traffic(&mut fields, &mut columns, children);
    let mut metadata = merged.schema().metadata().clone();
    metadata.insert(CONTRACT_KEY.to_owned(), "1".to_owned());
    metadata.remove("qm_blocks");
    let schema = Schema::new_with_metadata(fields, metadata);
    let batch = RecordBatch::try_new(Arc::new(schema), columns).map_err(|e| e.to_string())?;
    Ok(batch)
}

pub fn encode(batch: &RecordBatch) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut writer =
        FileWriter::try_new(&mut out, batch.schema().as_ref()).map_err(|e| e.to_string())?;
    writer.write(batch).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    drop(writer);
    Ok(out)
}

fn i32_col(children: &[Expanded], value: fn(&Expanded) -> i32) -> ArrayRef {
    Arc::new(Int32Array::from(
        children.iter().map(value).collect::<Vec<_>>(),
    ))
}

fn append_traffic(fields: &mut Vec<Field>, columns: &mut Vec<ArrayRef>, children: &[Expanded]) {
    let mut pax_day = Vec::with_capacity(children.len());
    let mut pax_eve = Vec::with_capacity(children.len());
    let mut pax_night = Vec::with_capacity(children.len());
    let mut frt_day = Vec::with_capacity(children.len());
    let mut frt_eve = Vec::with_capacity(children.len());
    let mut frt_night = Vec::with_capacity(children.len());
    let mut pax_status = Vec::with_capacity(children.len());
    let mut frt_status = Vec::with_capacity(children.len());
    let mut pax_source = Vec::with_capacity(children.len());
    let mut frt_source = Vec::with_capacity(children.len());
    let mut pax_match = Vec::with_capacity(children.len());
    let mut frt_match = Vec::with_capacity(children.len());
    for child in children {
        let p = child.child.traffic.passenger;
        let f = child.child.traffic.freight;
        pax_day.push(p.periods[0]);
        pax_eve.push(p.periods[1]);
        pax_night.push(p.periods[2]);
        frt_day.push(f.periods[0]);
        frt_eve.push(f.periods[1]);
        frt_night.push(f.periods[2]);
        pax_status.push(p.status);
        frt_status.push(f.status);
        pax_source.push(p.source_id);
        frt_source.push(f.source_id);
        pax_match.push(p.matching);
        frt_match.push(f.matching);
    }
    let mut traffic_arrays: Vec<ArrayRef> = vec![
        Arc::new(Float64Array::from(pax_day)),
        Arc::new(Float64Array::from(pax_eve)),
        Arc::new(Float64Array::from(pax_night)),
        Arc::new(Float64Array::from(frt_day)),
        Arc::new(Float64Array::from(frt_eve)),
        Arc::new(Float64Array::from(frt_night)),
        Arc::new(UInt8Array::from(pax_status)),
        Arc::new(UInt8Array::from(frt_status)),
        Arc::new(UInt16Array::from(pax_source)),
        Arc::new(UInt16Array::from(frt_source)),
        Arc::new(UInt8Array::from(pax_match)),
        Arc::new(UInt8Array::from(frt_match)),
    ];
    for passenger in [true, false] {
        for period in 0..3 {
            traffic_arrays.push(Arc::new(Float64Array::from(
                children
                    .iter()
                    .map(|child| {
                        let traffic = child.child.traffic;
                        if passenger {
                            traffic.passenger
                        } else {
                            traffic.freight
                        }
                        .missing_calendar_estimate[period]
                    })
                    .collect::<Vec<_>>(),
            )));
        }
    }
    for (name, array) in TRAFFIC.iter().zip(traffic_arrays) {
        fields.push(Field::new(*name, array.data_type().clone(), false));
        columns.push(array);
    }
}

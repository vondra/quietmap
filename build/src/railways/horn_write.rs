//! Encode horn approaches with their measured soundings and matched railway attributes.
use super::horn_geometry::{Approach, MergedPiece};
use super::types::Crossing;
use arrow_array::{
    ArrayRef, BooleanArray, Float32Array, Float64Array, Int16Array, Int32Array, Int64Array,
    RecordBatch, StringArray, UInt8Array, UInt16Array, UInt32Array,
};
use arrow_select::take::take;
use std::sync::Arc;
const HORN_RAIL_TYPE: u8 = 6;
#[allow(clippy::too_many_lines)]
pub(super) fn build_horn_batch(
    schema: &arrow_schema::SchemaRef,
    base: &RecordBatch,
    crossings: &[Crossing],
    approaches: &[Approach],
    pieces: &[MergedPiece],
) -> Result<RecordBatch, String> {
    let matched: Vec<u32> = pieces
        .iter()
        .map(|piece| approaches[piece.approach_idx].matched_row)
        .collect();
    let matched_idx = UInt32Array::from(matched);
    let take_col = |name: &str| -> Result<ArrayRef, String> {
        let column = base
            .column_by_name(name)
            .ok_or_else(|| format!("finalized railways Arrow lacks {name}"))?;
        take(column.as_ref(), &matched_idx, None).map_err(|err| err.to_string())
    };
    // Piece geometry: interpolate along the approach, snapping every end
    // (shared boundaries snap identically: the snap is deterministic).
    let mut start_gx = Vec::with_capacity(pieces.len());
    let mut start_gy = Vec::with_capacity(pieces.len());
    let mut end_gx = Vec::with_capacity(pieces.len());
    let mut end_gy = Vec::with_capacity(pieces.len());
    let mut length_m = Vec::with_capacity(pieces.len());
    for piece in pieces {
        let approach = &approaches[piece.approach_idx];
        let at = |frac: f64| {
            (
                approach.start_lon + (approach.end_lon - approach.start_lon) * frac,
                approach.start_lat + (approach.end_lat - approach.start_lat) * frac,
            )
        };
        let (s_lon, s_lat) = at(piece.frac0);
        let (e_lon, e_lat) = at(piece.frac1);
        let (sgx, sgy) = crate::osm::geometry::lonlat_to_grid(s_lon, s_lat);
        let (egx, egy) = crate::osm::geometry::lonlat_to_grid(e_lon, e_lat);
        start_gx.push(sgx);
        start_gy.push(sgy);
        end_gx.push(egx);
        end_gy.push(egy);
        length_m.push(crate::osm::geometry::flat_dist(s_lat, s_lon, e_lat, e_lon) as f32);
    }
    let mut columns: Vec<ArrayRef> = Vec::new();
    for field in schema.fields() {
        let array: ArrayRef = match field.name().as_str() {
            "osm_id" => Arc::new(Int64Array::from(
                pieces
                    .iter()
                    .map(|piece| crossings[approaches[piece.approach_idx].crossing].synth_osm)
                    .collect::<Vec<_>>(),
            )),
            "segment_idx" => Arc::new(Int16Array::from(
                pieces
                    .iter()
                    .map(|piece| {
                        let approach = &approaches[piece.approach_idx];
                        (approach.approach_idx as i16) * 1000 + piece.piece_idx
                    })
                    .collect::<Vec<_>>(),
            )),
            "start_gx" => Arc::new(Int32Array::from(start_gx.clone())),
            "start_gy" => Arc::new(Int32Array::from(start_gy.clone())),
            "end_gx" => Arc::new(Int32Array::from(end_gx.clone())),
            "end_gy" => Arc::new(Int32Array::from(end_gy.clone())),
            "length_m" => Arc::new(Float32Array::from(length_m.clone())),
            "rail_type" => Arc::new(UInt8Array::from(vec![HORN_RAIL_TYPE; pieces.len()])),
            "maxspeed" => Arc::new(UInt16Array::from(
                pieces
                    .iter()
                    .map(|piece| {
                        crossings[approaches[piece.approach_idx].crossing]
                            .speed_kmh
                            .map(|v| v.round().clamp(0.0, 65535.0) as u16)
                            .unwrap_or(0)
                    })
                    .collect::<Vec<_>>(),
            )),
            "name" => Arc::new(StringArray::from(
                pieces
                    .iter()
                    .map(|piece| {
                        format!(
                            "Level-crossing horn {}",
                            crossings[approaches[piece.approach_idx].crossing].id
                        )
                    })
                    .collect::<Vec<_>>(),
            )),
            "ref" => Arc::new(StringArray::from(vec![""; pieces.len()])),
            // Horn rows are at grade by construction: never inherit tunnel.
            // Horn rows never inherit highspeed either: the sounding speed
            // comes from the crossing inventory (or the 60 km/h horn
            // default), never from the line's high-speed flag.
            "tunnel" | "highspeed" => Arc::new(BooleanArray::from(vec![false; pieces.len()])),
            "service" => Arc::new(UInt8Array::from(vec![0u8; pieces.len()])),
            "traffic_mode" => Arc::new(UInt8Array::from(vec![0u8; pieces.len()])),
            "trains_passenger_day" => Arc::new(Float64Array::from(
                pieces.iter().map(|p| p.soundings[0]).collect::<Vec<_>>(),
            )),
            "trains_passenger_evening" => Arc::new(Float64Array::from(
                pieces.iter().map(|p| p.soundings[1]).collect::<Vec<_>>(),
            )),
            "trains_passenger_night" => Arc::new(Float64Array::from(
                pieces.iter().map(|p| p.soundings[2]).collect::<Vec<_>>(),
            )),
            "trains_freight_day" | "trains_freight_evening" | "trains_freight_night" => {
                Arc::new(Float64Array::from(vec![0.0; pieces.len()]))
            }
            name if name.contains("_missing_calendar_estimate_") => {
                Arc::new(Float64Array::from(vec![0.0; pieces.len()]))
            }
            "passenger_status" => Arc::new(UInt8Array::from(vec![2u8; pieces.len()])),
            // No freight term by construction: estimated zero, unattributed.
            "freight_status" => Arc::new(UInt8Array::from(vec![2u8; pieces.len()])),
            "passenger_source_id" => Arc::new(UInt16Array::from(
                pieces
                    .iter()
                    .map(|piece| crossings[approaches[piece.approach_idx].crossing].source_id)
                    .collect::<Vec<_>>(),
            )),
            "freight_source_id" => Arc::new(UInt16Array::from(vec![0u16; pieces.len()])),
            "passenger_matching" | "freight_matching" => {
                Arc::new(UInt8Array::from(vec![0u8; pieces.len()]))
            }
            // Tags + baked identity follow the matched track, as does the
            // retained OSM evidence: the approach runs along the track, and
            // the way extent describes the parent way, not the piece.
            "usage" | "electrified" | "gauge" | "bridge" | "country_iso" | "city_id"
            | "continent" | "osm_tags" | "way_start_node" | "way_end_node" | "way_start_gx"
            | "way_start_gy" | "way_end_gx" | "way_end_gy" => take_col(field.name())?,
            other => return Err(format!("horn rows cannot fill finalized column {other}")),
        };
        if array.data_type() != field.data_type() {
            return Err(format!(
                "horn column {} type {:?}, file wants {:?}",
                field.name(),
                array.data_type(),
                field.data_type()
            ));
        }
        columns.push(array);
    }
    RecordBatch::try_new(schema.clone(), columns).map_err(|err| err.to_string())
}

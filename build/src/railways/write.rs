//! Per-square Arrow rewrite: split, allocate over parallel tracks, stamp `rail_traffic_contract=1`.

use super::encode::{CONTRACT_KEY, Expanded, encode_children};
use super::freight_eligibility::{
    HighSpeedChord, high_speed_line_speeds_from_chords, row_takes_guessed_freight, utf8_value,
};
use super::merge::class_prior;
use super::parallel_tracks::allocate_over_parallel_tracks;
use super::split::{ChildGeom, split_parent};
use super::square_intervals::Interval;
use super::topology::load_square_pieces;
use crate::osm::geometry::Square;
use arrow_array::RecordBatch;
use arrow_array::{
    Array, BooleanArray, Float32Array, Int16Array, Int32Array, Int64Array, StringArray, UInt8Array,
    UInt16Array,
};
use arrow_select::concat::concat_batches;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;

#[allow(clippy::too_many_arguments)]
pub fn finalize_square(
    model: &super::model::RailModel,
    source_dir: &Path,
    crossings: &[super::types::Crossing],
    regions: Option<&crate::osm::geography::Regions>,
    neighbor_horns: Option<super::horns::NeighborHorns<'_>>,
    intervals: &HashMap<(i64, i16), Vec<Interval>>,
    output: &Path,
    square: crate::osm::geometry::Square,
) -> Result<usize, String> {
    std::fs::create_dir_all(output).map_err(|e| e.to_string())?;
    let path = source_dir.join("railways.arrow");
    let Some(table) = crate::dev4::read_table(&path)? else {
        return Ok(0);
    };
    crate::dev4::require_stamp(&table, "railways_contract", "country_baked_v1")?;
    if table.metadata.contains_key(CONTRACT_KEY) {
        return Err(
            "railway finalizer requires the OSM geography input, not finalized counts".into(),
        );
    }
    let schema = table
        .batches
        .first()
        .ok_or("railway input has no schema batch")?
        .schema();
    let merged = concat_batches(&schema, &table.batches).map_err(|e| e.to_string())?;
    let pieces = load_square_pieces(source_dir)?;
    let merged = super::yards::stamp_yard_service(&merged, source_dir, intervals)?;
    let children = expand_rows(model, &merged, intervals, &pieces, square)?;
    let mut batch = encode_children(&merged, &children)?;
    if !crossings.is_empty() {
        batch = super::horns::append(&batch, model, crossings, regions, square, neighbor_horns)?;
    }
    let ipc = super::encode::encode(&batch)?;
    write_atomically(output, &ipc)?;
    Ok(batch.num_rows())
}

pub(crate) fn col_i64<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Int64Array, String> {
    downcast(batch, name)
}
pub(crate) fn col_i16<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Int16Array, String> {
    downcast(batch, name)
}
pub(crate) fn col_i32<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Int32Array, String> {
    downcast(batch, name)
}
pub(crate) fn col_u8<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a UInt8Array, String> {
    downcast(batch, name)
}
pub(crate) fn col_u16<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a UInt16Array, String> {
    downcast(batch, name)
}
pub(crate) fn col_f32<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Float32Array, String> {
    downcast(batch, name)
}
fn col_bool<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a BooleanArray, String> {
    downcast(batch, name)
}
fn col_str<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray, String> {
    downcast(batch, name)
}

fn downcast<'a, T: Array + 'static>(batch: &'a RecordBatch, name: &str) -> Result<&'a T, String> {
    batch
        .column_by_name(name)
        .and_then(|column| column.as_any().downcast_ref::<T>())
        .ok_or_else(|| format!("railways Arrow missing {name}"))
}

fn row_country(batch: &RecordBatch, row: usize) -> Result<[u8; 2], String> {
    Ok(col_u16(batch, "country_iso")?.value(row).to_le_bytes())
}

pub(super) fn expand_rows(
    model: &super::model::RailModel,
    merged: &RecordBatch,
    intervals: &HashMap<(i64, i16), Vec<Interval>>,
    pieces: &HashMap<(i64, i16), super::topology::Piece>,
    square: Square,
) -> Result<Vec<Expanded>, String> {
    let osm_id = col_i64(merged, "osm_id")?;
    let segment_idx = col_i16(merged, "segment_idx")?;
    let start_gx = col_i32(merged, "start_gx")?;
    let start_gy = col_i32(merged, "start_gy")?;
    let end_gx = col_i32(merged, "end_gx")?;
    let end_gy = col_i32(merged, "end_gy")?;
    let length = col_f32(merged, "length_m")?;
    let rail_type = col_u8(merged, "rail_type")?;
    let usage = col_u8(merged, "usage")?;
    let service = col_u8(merged, "service")?;
    // The sources read these too (`sources::rail`, `railways::freight_eligibility`): an extract
    // without them is refused here, before it is finalized into a table they would refuse.
    let traffic_mode = col_u8(merged, "traffic_mode")?;
    let maxspeed = col_u16(merged, "maxspeed")?;
    let highspeed = col_bool(merged, "highspeed")?;
    let names = col_str(merged, "name")?;
    let refs = col_str(merged, "ref")?;
    let mut expanded = Vec::new();
    for row in 0..merged.num_rows() {
        let id = osm_id.value(row);
        let idx = segment_idx.value(row);
        // Horn periods are appended after finalization and cannot be split as daily traffic.
        if rail_type.value(row) == 6 {
            return Err(format!(
                "horn row (osm_id {id}) cannot be refinalized: prepare-railways requires the original OSM geography"
            ));
        }
        let original = ChildGeom {
            start_gx: start_gx.value(row),
            start_gy: start_gy.value(row),
            end_gx: end_gx.value(row),
            end_gy: end_gy.value(row),
            length_m: length.value(row),
        };
        let country = row_country(merged, row)?;
        let raw_intervals = intervals
            .get(&(id, idx))
            .map(Vec::as_slice)
            .unwrap_or_default();
        // Service tracks (including those newly stamped as yard infrastructure) carry no
        // crossing-allocated running-line traffic; only direct timetable claims remain.
        let filtered: Vec<Interval>;
        let row_intervals = if service.value(row) != 0
            && raw_intervals
                .iter()
                .any(|interval| interval.resolved_section.is_some())
        {
            filtered = raw_intervals
                .iter()
                .filter(|interval| interval.resolved_section.is_none())
                .cloned()
                .collect();
            filtered.as_slice()
        } else {
            raw_intervals
        };
        let children = split_parent(
            model,
            super::split::ParentRow {
                osm_id: id,
                segment_idx: idx,
                original,
                rail_type: rail_type.value(row),
                country,
            },
            pieces.get(&(id, idx)),
            row_intervals,
        )?;
        let ref_token = utf8_value(refs, row);
        let name = utf8_value(names, row);
        let corridor = if ref_token.trim().is_empty() {
            name
        } else {
            ref_token
        };
        let mode = traffic_mode.value(row);
        let prior = class_prior(
            model,
            rail_type.value(row),
            usage.value(row),
            service.value(row),
            country,
            mode,
        );
        for child in children {
            expanded.push(Expanded {
                parent: row as u32,
                child,
                prior,
                osm_id: id,
                corridor: corridor.trim().to_owned(),
                rail_type: rail_type.value(row),
                usage: usage.value(row),
                service: service.value(row),
                traffic_mode: mode,
                takes_guessed_freight: true,
                country_iso: country,
            });
        }
    }
    // Compute high-speed line speeds on expanded child chords so preparation, the national
    // train-scale fit and the source emitter agree bit-for-bit on freight eligibility.
    let line_speeds = high_speed_line_speeds_from_chords(expanded.iter().map(|row| {
        let parent = row.parent as usize;
        let geom = row.child.geom;
        HighSpeedChord {
            highspeed: highspeed.value(parent),
            maxspeed: maxspeed.value(parent),
            name: utf8_value(names, parent),
            reference: utf8_value(refs, parent),
            ends: [(geom.start_gx, geom.start_gy), (geom.end_gx, geom.end_gy)],
        }
    }));
    for row in &mut expanded {
        let parent = row.parent as usize;
        row.takes_guessed_freight = row_takes_guessed_freight(
            highspeed.value(parent),
            maxspeed.value(parent),
            row.traffic_mode,
            utf8_value(names, parent),
            utf8_value(refs, parent),
            &line_speeds,
        )
        .0;
    }
    allocate_over_parallel_tracks(model, &mut expanded, square);
    Ok(expanded)
}

pub(crate) fn write_atomically(dir: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = dir.join("railways.arrow.tmp");
    let final_path = dir.join("railways.arrow");
    {
        let mut file = File::create(&tmp).map_err(|e| format!("create {}: {e}", tmp.display()))?;
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, &final_path)
        .map_err(|e| format!("rename {}: {e}", final_path.display()))?;
    File::open(dir)
        .and_then(|directory| directory.sync_all())
        .map_err(|e| format!("sync {}: {e}", dir.display()))
}

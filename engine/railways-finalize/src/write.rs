//! Per-square Arrow rewrite: split, stamp `rail_traffic_contract=1`, z14-rebatch.

use crate::encode::{encode_children, Expanded, CONTRACT_KEY};
use crate::merge::{fill_missing_priors, RowTraffic, STATUS_UNKNOWN};
use crate::sharing::apply_default_sharing;
use crate::split::{split_parent, ChildGeom, ChildRow};
use crate::square_intervals::{load_square_intervals, Interval};
use crate::topology::load_square_pieces;
use arrow::array::{
    Array, Float32Array, Int16Array, Int32Array, Int64Array, UInt16Array, UInt8Array,
};
use arrow::compute::concat_batches;
use arrow::ipc::reader::FileReader;
use arrow::record_batch::RecordBatch;
use grid::Square;
use noise_compute::square_country_city::{Continent, SquareCountryCity};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Cursor, Write};
use std::path::Path;

pub struct SquareReceipt {
    pub rewritten: bool,
    pub rows_in: usize,
    pub rows_out: usize,
}

pub fn finalize_square(
    prepared_year: &Path,
    square: Square,
) -> Result<Option<SquareReceipt>, String> {
    let dir = prepared_year.join(grid::square_name(square));
    let arrow_path = dir.join("railways.arrow");
    if !arrow_path.is_file() {
        return Ok(None);
    }
    let bytes =
        std::fs::read(&arrow_path).map_err(|e| format!("read {}: {e}", arrow_path.display()))?;
    let reader = FileReader::try_new(Cursor::new(&bytes), None)
        .map_err(|e| format!("arrow open {}: {e}", arrow_path.display()))?;
    let schema = reader.schema();
    let finalized = schema.metadata().get(CONTRACT_KEY).map(String::as_str) == Some("1");
    let batches = reader
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("arrow batch {}: {e}", arrow_path.display()))?;
    if finalized {
        crate::rail_traffic::RailTrafficColumns::read(&RecordBatch::new_empty(schema.clone()))?;
        let mut rows = 0;
        let mut missing_priors = false;
        for batch in &batches {
            let traffic = crate::rail_traffic::RailTrafficColumns::read(batch)?;
            let service = col_u8(batch, "service")?;
            for row in 0..batch.num_rows() {
                let current = traffic.row(row);
                missing_priors |= service.value(row) == 0
                    && (current.passenger.status == STATUS_UNKNOWN
                        || current.freight.status == STATUS_UNKNOWN);
            }
            if batch.num_rows() > 0
                && !schema
                    .metadata()
                    .contains_key(arrow_batching::QM_BLOCKS_KEY)
            {
                return Err(format!(
                    "unfinished railway blocks: {}",
                    arrow_path.display()
                ));
            }
            rows += batch.num_rows();
        }
        if !missing_priors {
            return Ok(Some(SquareReceipt {
                rewritten: false,
                rows_in: rows,
                rows_out: rows,
            }));
        }
    }
    let merged =
        concat_batches(&schema, &batches).map_err(|e| format!("{}: {e}", arrow_path.display()))?;
    let retained = finalized
        .then(|| crate::rail_traffic::RailTrafficColumns::read(&merged))
        .transpose()?;
    let intervals = if finalized {
        HashMap::new()
    } else {
        load_square_intervals(&dir)?
    };
    let pieces = if finalized {
        HashMap::new()
    } else {
        load_square_pieces(&dir)?
    };
    let children = expand_rows(&merged, &intervals, &pieces, retained.as_ref())?;
    let ipc = encode_children(&merged, &children)?;
    write_atomically(&dir, &ipc)?;
    Ok(Some(SquareReceipt {
        rewritten: true,
        rows_in: merged.num_rows(),
        rows_out: children.len(),
    }))
}

fn col_i64<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Int64Array, String> {
    downcast(batch, name)
}
fn col_i16<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Int16Array, String> {
    downcast(batch, name)
}
fn col_i32<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Int32Array, String> {
    downcast(batch, name)
}
fn col_u8<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a UInt8Array, String> {
    downcast(batch, name)
}
fn col_u16<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a UInt16Array, String> {
    downcast(batch, name)
}
fn col_f32<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Float32Array, String> {
    downcast(batch, name)
}

fn downcast<'a, T: Array + 'static>(batch: &'a RecordBatch, name: &str) -> Result<&'a T, String> {
    batch
        .column_by_name(name)
        .and_then(|column| column.as_any().downcast_ref::<T>())
        .ok_or_else(|| format!("railways Arrow missing {name}"))
}

fn utf8_at(batch: &RecordBatch, name: &str, row: usize) -> String {
    batch
        .column_by_name(name)
        .and_then(|column| {
            column
                .as_any()
                .downcast_ref::<arrow::array::StringArray>()
                .map(|array| {
                    if array.is_null(row) {
                        String::new()
                    } else {
                        array.value(row).to_owned()
                    }
                })
        })
        .unwrap_or_default()
}

fn row_country(batch: &RecordBatch, row: usize) -> Result<SquareCountryCity, String> {
    let country = col_u16(batch, "country_iso")?.value(row);
    let continent = col_u8(batch, "continent")?.value(row);
    let city = col_u16(batch, "city_id")?.value(row);
    Ok(SquareCountryCity {
        continent: Continent::from_u8(continent),
        country_iso: [(country & 0xff) as u8, (country >> 8) as u8],
        city_id: city,
    })
}

fn expand_rows(
    merged: &RecordBatch,
    intervals: &HashMap<(i64, i16), Vec<Interval>>,
    pieces: &HashMap<(i64, i16), crate::topology::Piece>,
    retained: Option<&crate::rail_traffic::RailTrafficColumns<'_>>,
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
    let mut expanded = Vec::new();
    for row in 0..merged.num_rows() {
        let id = osm_id.value(row);
        let idx = segment_idx.value(row);
        let original = ChildGeom {
            start_gx: start_gx.value(row),
            start_gy: start_gy.value(row),
            end_gx: end_gx.value(row),
            end_gy: end_gy.value(row),
            length_m: length.value(row),
        };
        let children = if let Some(retained) = retained {
            let current = retained.row(row);
            let mut priors = RowTraffic::default();
            fill_missing_priors(
                &mut priors,
                rail_type.value(row),
                usage.value(row),
                service.value(row),
                row_country(merged, row)?,
            );
            if current.passenger.status != STATUS_UNKNOWN {
                priors.passenger = Default::default();
            }
            if current.freight.status != STATUS_UNKNOWN {
                priors.freight = Default::default();
            }
            vec![ChildRow {
                geom: original,
                traffic: priors,
            }]
        } else {
            split_parent(
                id,
                idx,
                original,
                pieces.get(&(id, idx)),
                intervals
                    .get(&(id, idx))
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
                rail_type.value(row),
                usage.value(row),
                service.value(row),
                row_country(merged, row)?,
            )?
        };
        let ref_token = utf8_at(merged, "ref", row);
        let name = utf8_at(merged, "name", row);
        let corridor = if ref_token.trim().is_empty() {
            name
        } else {
            ref_token
        };
        for child in children {
            expanded.push(Expanded {
                parent: row as u32,
                child,
                osm_id: id,
                corridor: corridor.trim().to_owned(),
                rail_type: rail_type.value(row),
                usage: usage.value(row),
            });
        }
    }
    apply_default_sharing(&mut expanded);
    if let Some(retained) = retained {
        for row in &mut expanded {
            let current = retained.row(row.parent as usize);
            if current.passenger.status != STATUS_UNKNOWN {
                row.child.traffic.passenger = current.passenger;
            }
            if current.freight.status != STATUS_UNKNOWN {
                row.child.traffic.freight = current.freight;
            }
        }
    }
    Ok(expanded)
}

fn write_atomically(dir: &Path, bytes: &[u8]) -> Result<(), String> {
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

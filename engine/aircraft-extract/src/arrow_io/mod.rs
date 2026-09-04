//! Read / write Arrow files for the popup aircraft schemas. Atomic
//! write via a sibling `.tmp` rename — concurrent readers (popup,
//! pipeline) never observe a partially-written file.

use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};

use anyhow::Result;
use arrow::datatypes::Schema;
use arrow::ipc::reader::FileReader;
use arrow::ipc::writer::FileWriter;
use arrow::record_batch::RecordBatch;

use crate::arrow_schemas;

mod airborne;
mod airport_summary;
mod airport_traffic;
mod cruise;
mod cruise_spill;
mod flights;
mod segments;

pub use airborne::write_airborne;
pub use airport_summary::{read_airport_summary, write_airport_summary, AirportSummaryRow};
pub(crate) use airport_summary::{read_airport_summary_part, write_airport_summary_part};
pub use airport_traffic::{read_airport_traffic, write_airport_traffic, AirportTrafficRow};
pub use cruise::write_cruise;
pub(crate) use cruise_spill::{read_cruise_spill, write_cruise_spill, CruiseSpillRow};
pub use flights::{write_flights, FlightRow};
pub(crate) use segments::for_each_segment_batch;
pub use segments::{read_segments, write_segments};

pub(crate) fn sibling_tmp_path(p: &Path) -> PathBuf {
    let mut name = p
        .file_name()
        .map(|n| n.to_owned())
        .unwrap_or_else(|| std::ffi::OsString::from("anon"));
    name.push(".tmp");
    p.with_file_name(name)
}

pub(crate) fn write_record_batches(
    path: &Path,
    schema: &Schema,
    batches: &[RecordBatch],
) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = sibling_tmp_path(path);
    {
        let f = File::create(&tmp)?;
        // BufWriter coalesces FileWriter's many small writes (header,
        // schema, per-batch dictionaries, footer) into one syscall
        // per 8 KiB block. Without it, big arrow files on rotational
        // disk burn a measurable chunk of wall-time in write(2)
        // overhead. Mirrors the pattern in `cruise_spill.rs`.
        let mut w = FileWriter::try_new(BufWriter::new(f), schema)?;
        for b in batches {
            if b.num_rows() > 0 {
                w.write(b)?;
            }
        }
        w.finish()?;
        let buffer = w.into_inner()?;
        let file = buffer.into_inner().map_err(|error| error.into_error())?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub(crate) fn read_all_batches(path: &Path) -> Result<(Schema, Vec<RecordBatch>)> {
    let f = File::open(path)?;
    let r = FileReader::try_new(BufReader::new(f), None)?;
    let schema = r.schema();
    arrow_schemas::assert_schema_version(schema.metadata())?;
    let mut batches = Vec::new();
    for b in r {
        batches.push(b?);
    }
    Ok(((*schema).clone(), batches))
}

/// Stream record batches one at a time (schema-checked) instead of
/// collecting them all like [`read_all_batches`]. Peak is one batch: small
/// for chunk-written shards (`WRITE_CHUNK_ROWS`), but a legacy single-batch
/// shard is one big batch — so a caller bounding a 100M-row legacy day
/// (Stage 2B) also slices the decode (`for_each_segment_batch`) and caps
/// concurrency (the arrow batch itself still resides per worker).
pub(crate) fn for_each_batch(
    path: &Path,
    mut f: impl FnMut(RecordBatch) -> Result<()>,
) -> Result<()> {
    let file = File::open(path)?;
    let r = FileReader::try_new(BufReader::new(file), None)?;
    arrow_schemas::assert_schema_version(r.schema().metadata())?;
    let mut had_batch = false;
    let schema = r.schema();
    for b in r {
        had_batch = true;
        f(b?)?;
    }
    if !had_batch {
        f(RecordBatch::new_empty(schema))?;
    }
    Ok(())
}

pub fn read_record_batches(path: &Path) -> Result<(Schema, Vec<RecordBatch>)> {
    read_all_batches(path)
}

/// Integer metre storage cannot silently saturate corrupt elevations.
pub(crate) fn height_meters(value: f32) -> Result<i16> {
    anyhow::ensure!(
        value.is_finite() && (i16::MIN as f32..=i16::MAX as f32).contains(&value.round()),
        "aircraft elevation is not representable in integer metres: {value}"
    );
    Ok(value.round() as i16)
}

pub(crate) fn required_column<'a, T: arrow::array::Array + 'static>(
    batch: &'a RecordBatch,
    name: &str,
) -> Result<&'a T> {
    let array = batch
        .column_by_name(name)
        .and_then(|array| array.as_any().downcast_ref::<T>())
        .ok_or_else(|| anyhow::anyhow!("missing or wrongly typed aircraft column {name}"))?;
    anyhow::ensure!(
        array.null_count() == 0,
        "aircraft column {name} contains nulls"
    );
    Ok(array)
}

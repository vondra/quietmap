//! Observed aircraft data processing on the canonical square grid.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use arrow::array::{
    Array, Float32Array, Float32Builder, Int32Array, Int32Builder, StringArray, StringBuilder,
    UInt16Array, UInt16Builder, UInt64Array, UInt64Builder, UInt8Array, UInt8Builder,
};
use arrow::record_batch::RecordBatch;

use crate::arrow_io::{read_all_batches, write_record_batch_stream};
use crate::arrow_schemas::synth_airport_lines_schema;

/// True iff `osm_id` carries the [`SYNTHETIC_OSM_ID_BIT`] marker —
/// emitted by Stage 1.5 DBSCAN, not by real OSM. Cheaper to read at
/// call sites than the inline bitmask `osm_id & SYNTHETIC_OSM_ID_BIT
/// != 0` and keeps the bit definition encapsulated.
pub(crate) fn is_synthetic_osm_id(osm_id: u64) -> bool {
    osm_id & SYNTHETIC_OSM_ID_BIT != 0
}

pub(crate) const SYNTHETIC_OSM_ID_BIT: u64 = 1u64 << 63;

/// Filename under `<prepared_year>/<z9>/` holding the Stage 1.5 synthetic
/// airstrip lines for that z9 (empty arrow when nothing clustered).
pub(crate) const SYNTH_LINES_FILE: &str = "synth_airport_lines.arrow";

/// Aeroway-type sentinel for synthetic airstrip lines. Mirrors the
/// real OSM convention (`osm-extract/classify.rs::aeroway_type` —
/// 0=runway, 1=taxiway, 6=stopway, 7=airstrip).
pub(crate) const AIRSTRIP_AEROWAY_TYPE: u8 = 7;

/// z20 location bins retain a stable airport key at approximately 38 m resolution.
fn synth_cell(lat: f64, lon: f64) -> u64 {
    let (gx, gy) = grid::lonlat_to_grid(grid::geo::normalize_longitude(lon), lat);
    let (gx, gy) = (
        gx.clamp(0, (1 << 30) - 1) as u64,
        gy.clamp(0, (1 << 30) - 1) as u64,
    );
    ((gx >> 10) << 20) | (gy >> 10)
}

pub(crate) fn synth_osm_id_for(lat: f64, lon: f64) -> u64 {
    SYNTHETIC_OSM_ID_BIT | synth_cell(lat, lon)
}

pub(crate) fn synth_airport_key_for(lat: f64, lon: f64) -> String {
    let (lon, lat) = synth_cell_center(synth_cell(lat, lon));
    format!(
        "auto-{}-{}",
        (lon * 1e5).round() as i32,
        (lat * 1e5).round() as i32
    )
}

fn synth_cell_center(cell: u64) -> (f64, f64) {
    square_store::grid_cols::grid_cell_lonlat(
        (((cell >> 20) << 10) + 512) as i32,
        (((cell & ((1 << 20) - 1)) << 10) + 512) as i32,
    )
}

pub(crate) fn synth_owner_for_id(id: u64) -> Result<u64> {
    anyhow::ensure!(
        id >> 63 == 1 && id & !(SYNTHETIC_OSM_ID_BIT | ((1u64 << 40) - 1)) == 0,
        "invalid discovered location identity"
    );
    let (lon, lat) = synth_cell_center(id & ((1u64 << 40) - 1));
    Ok(grid::square_id(grid::square_of(lat, lon)) as u64)
}

/// Discovery identifies geometry; candidate vertices are not observed flight counts.
pub const DISCOVERED_AIRSTRIP_NAME: &str = "Discovered airstrip";

/// One row of `synth_airport_lines.arrow`. Carries an explicit
/// `airport_key` because synthetic clusters have no icao/iata/name
/// to derive identity from (unlike real OSM aerodromes).
#[derive(Debug, Clone)]
pub struct SynthAirportLineRow {
    pub osm_id: u64,
    pub segment_idx: u16,
    pub airport_key: String,
    pub start_gx: i32,
    pub start_gy: i32,
    pub end_gx: i32,
    pub end_gy: i32,
    pub length_m: f32,
    pub heading_deg: f32,
    pub aeroway_type: u8,
    pub name: String,
}

// Bound decoded rows, string builders and IPC buffers independently of airport count.
pub(crate) const SYNTH_WRITE_BATCH_ROWS: usize = 4096;

pub(crate) fn writer_allocation_allowance(max_airport_key_bytes: usize) -> u64 {
    // Real names are not copied, only their airport keys.
    let key = max_airport_key_bytes.max(synth_airport_key_for(-90.0, -180.0).len());
    let row = std::mem::size_of::<SynthAirportLineRow>();
    // Rows + string growth, Arrow builders and serialized IPC coexist. Four
    // row-sized fixed parts overbound every field/offset/validity buffer.
    (4 * SYNTH_WRITE_BATCH_ROWS * (row + 2 * key.max(24) + 2 * DISCOVERED_AIRSTRIP_NAME.len()))
        as u64
}

fn write_synth_rows<T>(
    path: &Path,
    schema: Arc<arrow::datatypes::Schema>,
    rows: impl IntoIterator<Item = T>,
    batch: impl Fn(&Arc<arrow::datatypes::Schema>, &[T]) -> Result<RecordBatch>,
) -> Result<()> {
    let mut rows = rows.into_iter();
    let batches = std::iter::from_fn(|| {
        let chunk: Vec<_> = rows.by_ref().take(SYNTH_WRITE_BATCH_ROWS).collect();
        (!chunk.is_empty()).then(|| batch(&schema, &chunk))
    });
    write_record_batch_stream(path, &schema, batches)
}

/// Atomically replace synthetic lines in source order, including an empty stream.
pub(crate) fn write_synth_airport_lines(
    path: &Path,
    rows: impl IntoIterator<Item = SynthAirportLineRow>,
) -> Result<()> {
    write_synth_rows(path, synth_airport_lines_schema(), rows, lines_batch)
}

fn lines_batch(
    schema: &Arc<arrow::datatypes::Schema>,
    rows: &[SynthAirportLineRow],
) -> Result<RecordBatch> {
    let n = rows.len();

    let mut osm_id = UInt64Builder::with_capacity(n);
    let mut seg_idx = UInt16Builder::with_capacity(n);
    let mut airport_key = StringBuilder::with_capacity(n, n * 24);
    let mut slat = Int32Builder::with_capacity(n);
    let mut slon = Int32Builder::with_capacity(n);
    let mut elat = Int32Builder::with_capacity(n);
    let mut elon = Int32Builder::with_capacity(n);
    let mut len = Float32Builder::with_capacity(n);
    let mut heading = Float32Builder::with_capacity(n);
    let mut atype = UInt8Builder::with_capacity(n);
    let mut name = StringBuilder::with_capacity(n, n * DISCOVERED_AIRSTRIP_NAME.len());

    for r in rows {
        osm_id.append_value(r.osm_id);
        seg_idx.append_value(r.segment_idx);
        airport_key.append_value(&r.airport_key);
        let (gx, gy) = (r.start_gx, r.start_gy);
        slat.append_value(gx);
        slon.append_value(gy);
        let (gx, gy) = (r.end_gx, r.end_gy);
        elat.append_value(gx);
        elon.append_value(gy);
        len.append_value(r.length_m);
        heading.append_value(r.heading_deg);
        atype.append_value(r.aeroway_type);
        name.append_value(&r.name);
    }

    Ok(RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(osm_id.finish()),
            Arc::new(seg_idx.finish()),
            Arc::new(airport_key.finish()),
            Arc::new(slat.finish()),
            Arc::new(slon.finish()),
            Arc::new(elat.finish()),
            Arc::new(elon.finish()),
            Arc::new(len.finish()),
            Arc::new(heading.finish()),
            Arc::new(atype.finish()),
            Arc::new(name.finish()),
        ],
    )?)
}

fn col_u64<'a>(b: &'a RecordBatch, n: &str) -> Option<&'a UInt64Array> {
    b.column_by_name(n)?.as_any().downcast_ref()
}
fn col_u16<'a>(b: &'a RecordBatch, n: &str) -> Option<&'a UInt16Array> {
    b.column_by_name(n)?.as_any().downcast_ref()
}
fn col_u8<'a>(b: &'a RecordBatch, n: &str) -> Option<&'a UInt8Array> {
    b.column_by_name(n)?.as_any().downcast_ref()
}
fn col_str<'a>(b: &'a RecordBatch, n: &str) -> Option<&'a StringArray> {
    b.column_by_name(n)?.as_any().downcast_ref()
}
fn col_i32<'a>(b: &'a RecordBatch, n: &str) -> Option<&'a Int32Array> {
    b.column_by_name(n)?.as_any().downcast_ref()
}
fn col_f32<'a>(b: &'a RecordBatch, n: &str) -> Option<&'a Float32Array> {
    b.column_by_name(n)?.as_any().downcast_ref()
}

/// Read `synth_airport_lines.arrow`. Missing file → empty vec (the
/// per-z9 sidecar is absent when Stage 1.5 found no clusters there).
/// Routes through [`crate::arrow_io::read_all_batches`] for the
/// `schema_version` guard — stale files raise loudly instead of
/// silently decoding as zero rows.
pub fn read_synth_airport_lines(path: &Path) -> Result<Vec<SynthAirportLineRow>> {
    match std::fs::metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    let mut out = Vec::new();
    let (schema, batches) = read_all_batches(path)?;
    let expected = synth_airport_lines_schema();
    anyhow::ensure!(
        schema.fields() == expected.fields()
            && schema.metadata().get("synth_airport_lines_contract")
                == expected.metadata().get("synth_airport_lines_contract"),
        "invalid synth_airport_lines contract at {}",
        path.display()
    );
    for batch in batches {
        anyhow::ensure!(
            batch.columns().iter().all(|array| array.null_count() == 0),
            "null synthetic airport column"
        );
        let n = batch.num_rows();
        let (
            Some(osm_id),
            Some(seg),
            Some(key),
            Some(sla),
            Some(slo),
            Some(ela),
            Some(elo),
            Some(len),
            Some(hd),
            Some(at),
            Some(name),
        ) = (
            col_u64(&batch, "osm_id"),
            col_u16(&batch, "segment_idx"),
            col_str(&batch, "airport_key"),
            col_i32(&batch, "start_gx"),
            col_i32(&batch, "start_gy"),
            col_i32(&batch, "end_gx"),
            col_i32(&batch, "end_gy"),
            col_f32(&batch, "length_m"),
            col_f32(&batch, "heading_deg"),
            col_u8(&batch, "aeroway_type"),
            col_str(&batch, "name"),
        )
        else {
            anyhow::bail!(
                "synth_airport_lines.arrow at {} is missing required columns; \
                 re-extract the aircraft pipeline",
                path.display()
            );
        };
        for i in 0..n {
            out.push(SynthAirportLineRow {
                osm_id: osm_id.value(i),
                segment_idx: seg.value(i),
                airport_key: key.value(i).to_string(),
                start_gx: sla.value(i),
                start_gy: slo.value(i),
                end_gx: ela.value(i),
                end_gy: elo.value(i),
                length_m: len.value(i),
                heading_deg: hd.value(i),
                aeroway_type: at.value(i),
                name: name.value(i).to_string(),
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
#[path = "synth_airport_io_tests.rs"]
mod tests;

//! Stage 1 segments writer + reader.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use arrow::array::{
    ArrayRef, FixedSizeBinaryArray, FixedSizeBinaryBuilder, Float32Array, Float32Builder,
    Int16Array, Int16Builder, StringArray, StringBuilder, UInt64Array, UInt64Builder, UInt8Array,
    UInt8Builder,
};
use arrow::datatypes::Schema;
use arrow::record_batch::RecordBatch;

use crate::arrow_schemas;
use crate::flight::FlightSegment;

use super::{for_each_batch, required_column, write_record_batches};

/// Rows per record batch. A whole-day Stage 1 shard is ~100M segments; a
/// single batch would be ~8 GB resident on read, which Stage 2B's
/// batch-streaming reader can't bound (it streams whole batches). ~1M rows
/// ≈ 80 MB decoded keeps the per-batch read small. See [`for_each_batch`].
const WRITE_CHUNK_ROWS: usize = 1_000_000;

pub fn write_segments(path: &Path, rows: &[FlightSegment]) -> Result<()> {
    write_segments_chunked(path, rows, WRITE_CHUNK_ROWS)
}

fn write_segments_chunked(path: &Path, rows: &[FlightSegment], chunk_rows: usize) -> Result<()> {
    let schema = arrow_schemas::segments_schema();
    let batches = rows
        .chunks(chunk_rows.max(1))
        .map(|chunk| build_segments_batch(chunk, &schema))
        .collect::<Result<Vec<_>>>()?;
    write_record_batches(path, &schema, &batches)
}

/// Build one segments record batch from up to `WRITE_CHUNK_ROWS` rows.
fn build_segments_batch(rows: &[FlightSegment], schema: &Arc<Schema>) -> Result<RecordBatch> {
    let n = rows.len();
    let mut flight_id = UInt64Builder::with_capacity(n);
    let mut callsign = StringBuilder::with_capacity(n, 8 * n);
    let mut aircraft_type = FixedSizeBinaryBuilder::with_capacity(n, 4);
    let mut profile_idx = UInt8Builder::with_capacity(n);
    let mut source_id = UInt8Builder::with_capacity(n);
    let mut origin = UInt8Builder::with_capacity(n);
    let mut veh_kind = UInt8Builder::with_capacity(n);
    let mut gse_class = UInt8Builder::with_capacity(n);
    let mut period = UInt8Builder::with_capacity(n);
    let mut date_id = Int16Builder::with_capacity(n);
    let mut phase = UInt8Builder::with_capacity(n);
    let mut flags = UInt8Builder::with_capacity(n);
    let mut sla = Float32Builder::with_capacity(n);
    let mut slo = Float32Builder::with_capacity(n);
    let mut sal = Float32Builder::with_capacity(n);
    let mut ela = Float32Builder::with_capacity(n);
    let mut elo = Float32Builder::with_capacity(n);
    let mut eal = Float32Builder::with_capacity(n);
    let mut speed = Float32Builder::with_capacity(n);
    let mut length = Float32Builder::with_capacity(n);
    let mut agl = Float32Builder::with_capacity(n);
    let mut s_elev = Float32Builder::with_capacity(n);
    let mut e_elev = Float32Builder::with_capacity(n);
    for r in rows {
        flight_id.append_value(r.flight_id);
        callsign.append_value(&r.callsign);
        aircraft_type.append_value(r.aircraft_type)?;
        profile_idx.append_value(r.profile_idx);
        source_id.append_value(r.source_id);
        origin.append_value(r.origin);
        veh_kind.append_value(r.veh_kind);
        gse_class.append_value(r.gse_class);
        period.append_value(r.period);
        date_id.append_value(r.date_id);
        phase.append_value(r.phase.as_u8());
        flags.append_value(r.flags);
        sla.append_value(r.start_lat);
        slo.append_value(r.start_lon);
        sal.append_value(r.start_alt_m);
        ela.append_value(r.end_lat);
        elo.append_value(r.end_lon);
        eal.append_value(r.end_alt_m);
        speed.append_value(r.speed_kt);
        length.append_value(r.length_m);
        agl.append_value(r.agl_avg_m);
        s_elev.append_value(r.start_elev_m);
        e_elev.append_value(r.end_elev_m);
    }
    let columns: Vec<ArrayRef> = vec![
        Arc::new(flight_id.finish()),
        Arc::new(callsign.finish()),
        Arc::new(aircraft_type.finish()),
        Arc::new(profile_idx.finish()),
        Arc::new(source_id.finish()),
        Arc::new(origin.finish()),
        Arc::new(veh_kind.finish()),
        Arc::new(gse_class.finish()),
        Arc::new(period.finish()),
        Arc::new(date_id.finish()),
        Arc::new(phase.finish()),
        Arc::new(flags.finish()),
        Arc::new(sla.finish()),
        Arc::new(slo.finish()),
        Arc::new(sal.finish()),
        Arc::new(ela.finish()),
        Arc::new(elo.finish()),
        Arc::new(eal.finish()),
        Arc::new(speed.finish()),
        Arc::new(length.finish()),
        Arc::new(agl.finish()),
        Arc::new(s_elev.finish()),
        Arc::new(e_elev.finish()),
    ];
    Ok(RecordBatch::try_new(schema.clone(), columns)?)
}

/// Decode one segments record batch — the per-batch half of
/// [`read_segments`], reused by the streaming [`for_each_segment_batch`].
fn segments_from_batch(b: &RecordBatch) -> Result<Vec<FlightSegment>> {
    anyhow::ensure!(
        b.schema().fields() == arrow_schemas::segments_schema().fields(),
        "segments Arrow fields mismatch"
    );
    let mut out = Vec::with_capacity(b.num_rows());
    // Scoped so the per-column array borrows of `b` drop before returning.
    {
        let flight_id = required_column::<UInt64Array>(b, "flight_id")?;
        let callsign = required_column::<StringArray>(b, "callsign")?;
        let aircraft_type = required_column::<FixedSizeBinaryArray>(b, "aircraft_type")?;
        let profile_idx = required_column::<UInt8Array>(b, "profile_idx")?;
        let source_id = required_column::<UInt8Array>(b, "source_id")?;
        let origin = required_column::<UInt8Array>(b, "origin")?;
        let veh_kind = required_column::<UInt8Array>(b, "veh_kind")?;
        let gse_class = required_column::<UInt8Array>(b, "gse_class")?;
        let period = required_column::<UInt8Array>(b, "period")?;
        let date_id = required_column::<Int16Array>(b, "date_id")?;
        let phase = required_column::<UInt8Array>(b, "phase")?;
        let flags = required_column::<UInt8Array>(b, "flags")?;
        let sla = required_column::<Float32Array>(b, "start_lat")?;
        let slo = required_column::<Float32Array>(b, "start_lon")?;
        let sal = required_column::<Float32Array>(b, "start_alt_m")?;
        let ela = required_column::<Float32Array>(b, "end_lat")?;
        let elo = required_column::<Float32Array>(b, "end_lon")?;
        let eal = required_column::<Float32Array>(b, "end_alt_m")?;
        let speed = required_column::<Float32Array>(b, "speed_kt")?;
        let length = required_column::<Float32Array>(b, "length_m")?;
        let agl = required_column::<Float32Array>(b, "agl_avg_m")?;
        let s_elev = required_column::<Float32Array>(b, "start_elev_m")?;
        let e_elev = required_column::<Float32Array>(b, "end_elev_m")?;
        for i in 0..b.num_rows() {
            anyhow::ensure!(
                [
                    sla.value(i),
                    slo.value(i),
                    sal.value(i),
                    ela.value(i),
                    elo.value(i),
                    eal.value(i),
                    speed.value(i),
                    length.value(i),
                    agl.value(i),
                    s_elev.value(i),
                    e_elev.value(i)
                ]
                .iter()
                .all(|value| value.is_finite()),
                "segment {i} contains non-finite values"
            );
            anyhow::ensure!(
                (-90.0..=90.0).contains(&sla.value(i))
                    && (-90.0..=90.0).contains(&ela.value(i))
                    && (-180.0..=180.0).contains(&slo.value(i))
                    && (-180.0..=180.0).contains(&elo.value(i))
                    && period.value(i) < 3
                    && phase.value(i) <= 3,
                "segment {i} has invalid coordinates, period or phase"
            );
            let mut typecode = [0u8; 4];
            typecode.copy_from_slice(aircraft_type.value(i));
            out.push(FlightSegment {
                flight_id: flight_id.value(i),
                callsign: callsign.value(i).to_string(),
                aircraft_type: typecode,
                profile_idx: profile_idx.value(i),
                source_id: source_id.value(i),
                origin: origin.value(i),
                veh_kind: veh_kind.value(i),
                gse_class: gse_class.value(i),
                period: period.value(i),
                date_id: date_id.value(i),
                phase: crate::flight::Phase::from_u8(phase.value(i)),
                flags: flags.value(i),
                start_lat: sla.value(i),
                start_lon: slo.value(i),
                start_alt_m: sal.value(i),
                end_lat: ela.value(i),
                end_lon: elo.value(i),
                end_alt_m: eal.value(i),
                speed_kt: speed.value(i),
                length_m: length.value(i),
                agl_avg_m: agl.value(i),
                start_elev_m: s_elev.value(i),
                end_elev_m: e_elev.value(i),
            });
        }
    }
    Ok(out)
}

/// Stream a segments shard batch-by-batch, handing each batch's decoded
/// segments to `f` and dropping them before the next — peak RAM is one
/// batch, not the whole file. Stage 2B scans multi-GB day shards this way
/// instead of [`read_segments`] (which holds the entire file in RAM).
/// Decode a record batch at most this many rows at a time. A legacy
/// single-batch day shard holds ~100M rows in ONE batch; decoding it whole
/// would materialise ~8 GB of `FlightSegment`s at once (on top of the ~8 GB
/// arrow batch), so slice it. Newer shards are written in `WRITE_CHUNK_ROWS`
/// batches and decode in one slice. ~256k rows ≈ 20 MB decoded.
const READ_CHUNK_ROWS: usize = 262_144;

pub(crate) fn for_each_segment_batch(
    path: &Path,
    f: impl FnMut(Vec<FlightSegment>) -> Result<()>,
) -> Result<()> {
    for_each_segment_slice(path, READ_CHUNK_ROWS, f)
}

fn for_each_segment_slice(
    path: &Path,
    slice_rows: usize,
    mut f: impl FnMut(Vec<FlightSegment>) -> Result<()>,
) -> Result<()> {
    for_each_batch(path, |b| {
        if b.num_rows() == 0 {
            f(segments_from_batch(&b)?)?;
        }
        let n = b.num_rows();
        let mut off = 0;
        while off < n {
            let len = slice_rows.min(n - off);
            f(segments_from_batch(&b.slice(off, len))?)?;
            off += len;
        }
        Ok(())
    })
}

pub fn read_segments(path: &Path) -> Result<Vec<FlightSegment>> {
    let mut out = Vec::new();
    for_each_segment_batch(path, |mut segs| {
        out.append(&mut segs);
        Ok(())
    })?;
    Ok(out)
}

#[cfg(test)]
#[path = "segments_tests.rs"]
mod tests;

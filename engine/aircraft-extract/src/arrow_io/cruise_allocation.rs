//! Bound cruise IPC storage using the actual schema/writer and counted destination rows.

use super::cruise::cruise_record_batch;
use crate::flight::{CruiseBucket, CruiseTopCandidate};
use anyhow::Result;
use arrow::ipc::writer::FileWriter;

pub(crate) fn cruise_buffers_bound(rows: u64, candidates: u64, callsigns: u64) -> u64 {
    // Scalars/list offsets: 46 B/row, candidate children/offsets: 24 B/candidate.
    // Round 14 row and six candidate validity bitmaps upwards separately; each
    // batch adds at most 20 rounding bytes and eight terminal offset bytes.
    48 * rows + 25 * candidates + callsigns
}

pub(crate) fn cruise_file_overhead_bound() -> Result<u64> {
    let row = CruiseBucket {
        cruise_cell_id: grid::cruise::cruise_cell_id(0.0, 0.0),
        class: 1,
        rep_profile_idx: 1,
        fl_bin: 1,
        period: 1,
        sum_length_m: 1.0,
        rep_len_m: 1.0,
        rep_alt_m: 1.0,
        rep_speed_kt: 1.0,
        unique_count: 1,
        source_id: 1,
        origin: 1,
        top_candidates: vec![CruiseTopCandidate {
            flight_id: 1,
            callsign: "A".into(),
            aircraft_type: *b"A320",
            peak_lmax_25m_db: 1.0,
            altitude_m: 1.0,
        }],
    };
    let batch = cruise_record_batch(&[row], u16::MAX)?;
    fn buffers(data: &arrow::array::ArrayData) -> usize {
        // A validity buffer is represented even when the array has no nulls.
        1 + data.buffers().len() + data.child_data().iter().map(buffers).sum::<usize>()
    }
    let count: usize = batch.columns().iter().map(|a| buffers(&a.to_data())).sum();
    let mut writer = FileWriter::try_new(Vec::new(), batch.schema().as_ref())?;
    writer.write(&batch)?;
    writer.finish()?;
    // Charge a complete one-row file per batch, plus every possible 64-byte
    // alignment pad and bitmap/offset rounding. This also bounds final files
    // containing multiple 4096-row gather batches without a layout constant.
    Ok(writer.into_inner()?.len() as u64 + (count as u64 + 2) * 63 + 28)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arrow_io::{write_cruise, write_record_batch_stream};

    #[test]
    fn counted_bound_covers_variable_candidates_and_multibatch_gather() {
        let directory = tempfile::tempdir().unwrap();
        for count in [1, 7, 8, 9, 4096, 4097] {
            let rows: Vec<_> = (0..count)
                .map(|i| CruiseBucket {
                    cruise_cell_id: grid::cruise::cruise_cell_id(0.0, 0.0),
                    class: 1,
                    rep_profile_idx: 1,
                    fl_bin: 1,
                    period: 1,
                    sum_length_m: 1.0,
                    rep_len_m: 1.0,
                    rep_alt_m: 1.0,
                    rep_speed_kt: 1.0,
                    unique_count: 50,
                    source_id: 1,
                    origin: 1,
                    top_candidates: (0..i % 51)
                        .map(|j| CruiseTopCandidate {
                            flight_id: j as u64,
                            callsign: "X".repeat(j % 17),
                            aircraft_type: *b"A320",
                            peak_lmax_25m_db: 1.0,
                            altitude_m: 1.0,
                        })
                        .collect(),
                })
                .collect();
            let candidates = rows.iter().map(|r| r.top_candidates.len() as u64).sum();
            let strings = rows
                .iter()
                .flat_map(|r| &r.top_candidates)
                .map(|c| c.callsign.len() as u64)
                .sum();
            let body = cruise_buffers_bound(count as u64, candidates, strings);
            let overhead = cruise_file_overhead_bound().unwrap();
            let path = directory.path().join("single.arrow");
            write_cruise(&path, &rows, u16::MAX).unwrap();
            assert!(path.metadata().unwrap().len() <= body + overhead);
            let batches = rows.chunks(4096).map(|r| cruise_record_batch(r, u16::MAX));
            let path = directory.path().join("gather.arrow");
            let schema =
                crate::arrow_schemas::with_n_days(crate::arrow_schemas::cruise_schema(), u16::MAX);
            write_record_batch_stream(&path, schema.as_ref(), batches).unwrap();
            assert!(
                path.metadata().unwrap().len() <= body + overhead * (count as u64).div_ceil(4096)
            );
        }
    }
}

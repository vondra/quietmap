//! Cruise row accumulator (v14). Snapshots `cruise.arrow` columns into
//! owned per-row buffers; views borrow into them so noise-compute's
//! `CruiseRowView<'_>` never holds a reference into mmap'd arrow.
//!
//! v14 swap: drops the per-fid lists (`cruise_flight_ids` /
//! `cruise_aircraft_types` / `cruise_callsigns`) and stores the bounded
//! top-K `top_candidates` struct list + scalar `unique_count` instead.
//!
//! The row carries its
//! explicit `lon`/`lat` centroid (Float64 degrees), which is what
//! noise-compute's `CruiseRowView` consumes.

use arrow::array::*;
use noise_compute::compute::aircraft_v6::{CruiseRowView, CruiseTopCandidateView};

fn required_column<'a, T: Array + 'static>(
    column: Option<&'a ArrayRef>,
    batch_index: usize,
    name: &str,
) -> Result<&'a T, String> {
    super::columns::required_array::<T>(column, name)
        .map_err(|error| format!("cruise.arrow[batch {batch_index}] {error}"))
}

pub struct CruiseRowAccum {
    rows: Vec<OwnedCruiseRow>,
}

struct OwnedCruiseRow {
    lon: f64,
    lat: f64,
    class: u8,
    rep_profile_idx: u8,
    fl_bin: u8,
    period: u8,
    sum_length_m: f32,
    rep_len_m: f32,
    rep_alt_m: f32,
    rep_speed_kt: f32,
    source_id: u8,
    origin: u8,
    unique_count: u32,
    /// Owned per-candidate identity fields. Candidate views borrow
    /// from these — same Vec-of-String lifetime trick as the v13
    /// per-fid arrays.
    cand_fid: Vec<u64>,
    cand_callsign: Vec<String>,
    cand_typecode: Vec<[u8; 4]>,
    cand_lmax: Vec<f32>,
    cand_alt: Vec<f32>,
}

impl CruiseRowAccum {
    pub fn new(batches: &[arrow::record_batch::RecordBatch]) -> Result<Self, String> {
        let mut rows = Vec::new();
        for (batch_index, batch) in batches.iter().enumerate() {
            let n = batch.num_rows();
            let lon =
                required_column::<Float64Array>(batch.column_by_name("lon"), batch_index, "lon")?;
            let lat =
                required_column::<Float64Array>(batch.column_by_name("lat"), batch_index, "lat")?;
            let class =
                required_column::<UInt8Array>(batch.column_by_name("class"), batch_index, "class")?;
            let rep_pi = required_column::<UInt8Array>(
                batch.column_by_name("rep_profile_idx"),
                batch_index,
                "rep_profile_idx",
            )?;
            let fl_bin = required_column::<UInt8Array>(
                batch.column_by_name("fl_bin"),
                batch_index,
                "fl_bin",
            )?;
            let period = required_column::<UInt8Array>(
                batch.column_by_name("period"),
                batch_index,
                "period",
            )?;
            let sum_len = required_column::<Float32Array>(
                batch.column_by_name("sum_length_m"),
                batch_index,
                "sum_length_m",
            )?;
            let rep_len = required_column::<Float32Array>(
                batch.column_by_name("rep_len_m"),
                batch_index,
                "rep_len_m",
            )?;
            let rep_alt = required_column::<Float32Array>(
                batch.column_by_name("rep_alt_m"),
                batch_index,
                "rep_alt_m",
            )?;
            let rep_speed = required_column::<Float32Array>(
                batch.column_by_name("rep_speed_kt"),
                batch_index,
                "rep_speed_kt",
            )?;
            let unique_count = required_column::<UInt32Array>(
                batch.column_by_name("unique_count"),
                batch_index,
                "unique_count",
            )?;
            let source_id = required_column::<UInt8Array>(
                batch.column_by_name("source_id"),
                batch_index,
                "source_id",
            )?;
            let origin = required_column::<UInt8Array>(
                batch.column_by_name("origin"),
                batch_index,
                "origin",
            )?;
            let cand_list = required_column::<ListArray>(
                batch.column_by_name("top_candidates"),
                batch_index,
                "top_candidates",
            )?;
            let cand_struct = required_column::<StructArray>(
                Some(cand_list.values()),
                batch_index,
                "top_candidates.item",
            )?;
            let cand_fid_arr = required_column::<UInt64Array>(
                cand_struct.column_by_name("flight_id"),
                batch_index,
                "top_candidates.flight_id",
            )?;
            let cand_callsign_arr = required_column::<StringArray>(
                cand_struct.column_by_name("callsign"),
                batch_index,
                "top_candidates.callsign",
            )?;
            let cand_tc_arr = required_column::<FixedSizeBinaryArray>(
                cand_struct.column_by_name("aircraft_type"),
                batch_index,
                "top_candidates.aircraft_type",
            )?;
            if cand_tc_arr.value_length() != 4 {
                return Err(format!(
                    "cruise.arrow[batch {batch_index}] `top_candidates.aircraft_type` \
                     must be FixedSizeBinary(4) — rebuild the cruise z9 data"
                ));
            }
            let cand_lmax_arr = required_column::<Float32Array>(
                cand_struct.column_by_name("peak_lmax_25m_db"),
                batch_index,
                "top_candidates.peak_lmax_25m_db",
            )?;
            let cand_alt_arr = required_column::<Float32Array>(
                cand_struct.column_by_name("altitude_m"),
                batch_index,
                "top_candidates.altitude_m",
            )?;
            let cand_offsets = cand_list.value_offsets();
            for i in 0..n {
                let lo = cand_offsets[i] as usize;
                let hi = cand_offsets[i + 1] as usize;
                let len = hi - lo;
                let mut cand_fid = Vec::with_capacity(len);
                let mut cand_callsign = Vec::with_capacity(len);
                let mut cand_typecode = Vec::with_capacity(len);
                let mut cand_lmax = Vec::with_capacity(len);
                let mut cand_alt = Vec::with_capacity(len);
                for j in lo..hi {
                    cand_fid.push(cand_fid_arr.value(j));
                    cand_callsign.push(cand_callsign_arr.value(j).to_string());
                    let mut tc = [0u8; 4];
                    tc.copy_from_slice(cand_tc_arr.value(j));
                    cand_typecode.push(tc);
                    cand_lmax.push(cand_lmax_arr.value(j));
                    cand_alt.push(cand_alt_arr.value(j));
                }
                rows.push(OwnedCruiseRow {
                    lon: lon.value(i),
                    lat: lat.value(i),
                    class: class.value(i),
                    rep_profile_idx: rep_pi.value(i),
                    fl_bin: fl_bin.value(i),
                    period: period.value(i),
                    sum_length_m: sum_len.value(i),
                    rep_len_m: rep_len.value(i),
                    rep_alt_m: rep_alt.value(i),
                    rep_speed_kt: rep_speed.value(i),
                    source_id: source_id.value(i),
                    origin: origin.value(i),
                    unique_count: unique_count.value(i),
                    cand_fid,
                    cand_callsign,
                    cand_typecode,
                    cand_lmax,
                    cand_alt,
                });
            }
        }
        Ok(Self { rows })
    }

    /// Returns the owned candidate slice per row. Caller borrows
    /// into these to build `CruiseRowView<'_>` instances — the
    /// `CruiseTopCandidateView` slice must live as long as the
    /// `CruiseRowAccum`, hence the materialisation here instead of
    /// per-call construction.
    fn build_candidate_views<'a>(row: &'a OwnedCruiseRow) -> Vec<CruiseTopCandidateView<'a>> {
        (0..row.cand_fid.len())
            .map(|j| CruiseTopCandidateView {
                flight_id: row.cand_fid[j],
                callsign: row.cand_callsign[j].as_str(),
                aircraft_type: &row.cand_typecode[j],
                peak_lmax_25m_db: row.cand_lmax[j],
                altitude_m: row.cand_alt[j],
            })
            .collect()
    }

    /// Materialise per-row candidate views into a parallel
    /// `Vec<Vec<...>>` so the per-row borrow is contiguous in memory
    /// and noise-compute's per-row `&[CruiseTopCandidateView]` doesn't
    /// need to be reconstructed on each access.
    pub fn views(&self) -> CruiseViewSlices<'_> {
        let cand_views: Vec<Vec<CruiseTopCandidateView<'_>>> =
            self.rows.iter().map(Self::build_candidate_views).collect();
        CruiseViewSlices {
            rows: &self.rows,
            cand_views,
        }
    }
}

/// Pair of owned candidate-view vectors + a slice into the rows.
/// `as_row_views` borrows from both to hand noise-compute the
/// `Vec<CruiseRowView<'a>>` it expects.
pub struct CruiseViewSlices<'a> {
    rows: &'a [OwnedCruiseRow],
    cand_views: Vec<Vec<CruiseTopCandidateView<'a>>>,
}

impl<'a> CruiseViewSlices<'a> {
    pub fn as_row_views(&'a self) -> Vec<CruiseRowView<'a>> {
        self.rows
            .iter()
            .enumerate()
            .map(|(i, r)| CruiseRowView {
                lon: r.lon,
                lat: r.lat,
                class: r.class,
                rep_profile_idx: r.rep_profile_idx,
                fl_bin: r.fl_bin,
                period: r.period,
                sum_length_m: r.sum_length_m,
                rep_len_m: r.rep_len_m,
                rep_alt_m: r.rep_alt_m,
                rep_speed_kt: r.rep_speed_kt,
                source_id: r.source_id,
                origin: r.origin,
                unique_count: r.unique_count,
                top_candidates: self.cand_views[i].as_slice(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc};

    use arrow::{datatypes::Schema, record_batch::RecordBatch};

    use super::super::{assert_cruise_contract, CRUISE_CONTRACT, SCHEMA_VERSION};
    use super::*;

    fn empty_batch(cruise_contract: &str) -> RecordBatch {
        let metadata = HashMap::from([
            ("schema_version".to_string(), SCHEMA_VERSION.to_string()),
            ("cruise_contract".to_string(), cruise_contract.to_string()),
        ]);
        RecordBatch::new_empty(Arc::new(Schema::new_with_metadata(
            Vec::<arrow::datatypes::Field>::new(),
            metadata,
        )))
    }

    #[test]
    fn dev1_cruise_contract_is_not_accepted_as_z9() {
        let error = assert_cruise_contract("cruise.arrow", &[empty_batch("cruise_v17")])
            .expect_err("dev1 legacy schema must not pass the z9 contract gate");
        assert!(error.contains(CRUISE_CONTRACT));
    }

    #[test]
    fn current_contract_with_missing_columns_fails_loudly() {
        let batch = empty_batch(CRUISE_CONTRACT);
        assert_cruise_contract("cruise.arrow", std::slice::from_ref(&batch)).unwrap();
        let error = CruiseRowAccum::new(&[batch])
            .err()
            .expect("missing lon must fail");
        assert!(error.contains("`lon`"));
        assert!(error.contains("Float64"));
    }
}

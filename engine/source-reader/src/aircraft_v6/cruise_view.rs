//! Cruise row accumulator (v14). Snapshots `cruise.arrow` columns into
//! owned per-row buffers; views borrow into them so noise-compute's
//! `CruiseRowView<'_>` never holds a reference into mmap'd arrow.
//!
//! v14 swap: drops the per-fid lists (`cruise_flight_ids` /
//! `cruise_aircraft_types` / `cruise_callsigns`) and stores the bounded
//! top-K `top_candidates` struct list + scalar `unique_count` instead.
//!
//! Grid transfer: the old `r7_hex` bucket id is gone — the row carries its
//! explicit `lon`/`lat` centroid (Float64 degrees), which is what
//! noise-compute's `CruiseRowView` consumes.

use arrow::array::*;
use noise_compute::compute::aircraft_v6::{CruiseRowView, CruiseTopCandidateView};

use super::columns::{col_f32, col_f64, col_list, col_u32, col_u8};

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
    pub fn new(batches: &[arrow::record_batch::RecordBatch]) -> Self {
        let mut rows = Vec::new();
        for batch in batches {
            let n = batch.num_rows();
            if n == 0 {
                continue;
            }
            let (Some(lon), Some(lat)) = (col_f64(batch, "lon"), col_f64(batch, "lat")) else {
                continue;
            };
            let Some(class) = col_u8(batch, "class") else {
                continue;
            };
            let Some(rep_pi) = col_u8(batch, "rep_profile_idx") else {
                continue;
            };
            let Some(fl_bin) = col_u8(batch, "fl_bin") else {
                continue;
            };
            let Some(period) = col_u8(batch, "period") else {
                continue;
            };
            let Some(sum_len) = col_f32(batch, "sum_length_m") else {
                continue;
            };
            let Some(rep_len) = col_f32(batch, "rep_len_m") else {
                continue;
            };
            let Some(rep_alt) = col_f32(batch, "rep_alt_m") else {
                continue;
            };
            let Some(rep_speed) = col_f32(batch, "rep_speed_kt") else {
                continue;
            };
            let Some(unique_count) = col_u32(batch, "unique_count") else {
                continue;
            };
            let source_id = col_u8(batch, "source_id");
            let origin = col_u8(batch, "origin");
            let Some(cand_list) = col_list(batch, "top_candidates") else {
                continue;
            };
            let Some(cand_struct) = cand_list.values().as_any().downcast_ref::<StructArray>()
            else {
                continue;
            };
            let Some(cand_fid_arr) = cand_struct
                .column_by_name("flight_id")
                .and_then(|a| a.as_any().downcast_ref::<UInt64Array>())
            else {
                continue;
            };
            let Some(cand_callsign_arr) = cand_struct
                .column_by_name("callsign")
                .and_then(|a| a.as_any().downcast_ref::<StringArray>())
            else {
                continue;
            };
            let Some(cand_tc_arr) = cand_struct
                .column_by_name("aircraft_type")
                .and_then(|a| a.as_any().downcast_ref::<FixedSizeBinaryArray>())
            else {
                continue;
            };
            let Some(cand_lmax_arr) = cand_struct
                .column_by_name("peak_lmax_25m_db")
                .and_then(|a| a.as_any().downcast_ref::<Float32Array>())
            else {
                continue;
            };
            let Some(cand_alt_arr) = cand_struct
                .column_by_name("altitude_m")
                .and_then(|a| a.as_any().downcast_ref::<Float32Array>())
            else {
                continue;
            };
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
                    source_id: source_id.map(|a| a.value(i)).unwrap_or(0),
                    origin: origin.map(|a| a.value(i)).unwrap_or(0),
                    unique_count: unique_count.value(i),
                    cand_fid,
                    cand_callsign,
                    cand_typecode,
                    cand_lmax,
                    cand_alt,
                });
            }
        }
        Self { rows }
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

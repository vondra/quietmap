//! Strict cruise rows with per-batch borrowed views.
//!
//! One batch decodes to typed arrow slices plus ONE materialized candidate
//! vector (identity structs borrowing the arrow strings/bytes); row views
//! borrow per-row slices of it. No per-row Vecs, no per-row Strings — the old
//! owned copy cost ~0.3 s per popup on cruise-heavy squares.

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

/// One decoded `cruise.arrow` batch: typed scalar slices borrowed from the
/// arrow batch plus the row-major candidate structs (which borrow the arrow
/// callsign/typecode buffers). Row `i`'s candidates are `cands[lo..hi]` with
/// `lo/hi` from the arrow list offsets — no per-row allocation.
pub struct CruiseBatchViews<'a> {
    lon: &'a Float64Array,
    lat: &'a Float64Array,
    class: &'a UInt8Array,
    rep_profile_idx: &'a UInt8Array,
    fl_bin: &'a UInt8Array,
    period: &'a UInt8Array,
    sum_length_m: &'a Float32Array,
    heading_bin: &'a UInt8Array,
    rep_alt_m: &'a Float32Array,
    rep_speed_kt: &'a Float32Array,
    unique_count: &'a UInt32Array,
    source_id: &'a UInt8Array,
    origin: &'a UInt8Array,
    secondary_only: &'a UInt8Array,
    cand_offsets: &'a [i32],
    cands: Vec<CruiseTopCandidateView<'a>>,
    n: usize,
}

impl<'a> CruiseBatchViews<'a> {
    pub fn len(&self) -> usize {
        self.n
    }

    fn row_view(&'a self, i: usize) -> CruiseRowView<'a> {
        let lo = self.cand_offsets[i] as usize;
        let hi = self.cand_offsets[i + 1] as usize;
        CruiseRowView {
            lon: self.lon.value(i),
            lat: self.lat.value(i),
            class: self.class.value(i),
            rep_profile_idx: self.rep_profile_idx.value(i),
            fl_bin: self.fl_bin.value(i),
            period: self.period.value(i),
            sum_length_m: self.sum_length_m.value(i),
            heading_bin: self.heading_bin.value(i),
            rep_alt_m: self.rep_alt_m.value(i),
            rep_speed_kt: self.rep_speed_kt.value(i),
            source_id: self.source_id.value(i),
            origin: self.origin.value(i),
            secondary_only: self.secondary_only.value(i) != 0,
            unique_count: self.unique_count.value(i),
            top_candidates: &self.cands[lo..hi],
        }
    }
}

pub struct CruiseRowAccum<'a> {
    batches: Vec<CruiseBatchViews<'a>>,
}

impl<'a> CruiseRowAccum<'a> {
    pub fn new(batches: &'a [arrow::record_batch::RecordBatch]) -> Result<Self, String> {
        batches
            .iter()
            .enumerate()
            .map(|(batch_index, batch)| decode_batch(batch, batch_index))
            .collect::<Result<_, _>>()
            .map(|batches| Self { batches })
    }

    pub fn len(&self) -> usize {
        self.batches.iter().map(CruiseBatchViews::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn views(&self) -> CruiseViewSlices<'_> {
        CruiseViewSlices {
            batches: &self.batches,
        }
    }
}

fn decode_batch(
    batch: &arrow::record_batch::RecordBatch,
    batch_index: usize,
) -> Result<CruiseBatchViews<'_>, String> {
    let n = batch.num_rows();
    let lon = required_column::<Float64Array>(batch.column_by_name("lon"), batch_index, "lon")?;
    let lat = required_column::<Float64Array>(batch.column_by_name("lat"), batch_index, "lat")?;
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
    let heading = required_column::<UInt8Array>(
        batch.column_by_name("heading_bin"),
        batch_index,
        "heading_bin",
    )?;
    if heading.values().iter().any(|&value| {
        value >= noise_compute::compute::aircraft_v6::cruise::CRUISE_HEADING_BINS
    }) {
        return Err(format!(
            "cruise.arrow[batch {batch_index}] heading_bin must be in 0..8"
        ));
    }
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
    let secondary_only = required_column::<UInt8Array>(
        batch.column_by_name("secondary_only"),
        batch_index,
        "secondary_only",
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
    let typecodes = cand_tc_arr.value_data();
    let n_cands = cand_offsets[n] as usize;
    let mut cands = Vec::with_capacity(n_cands);
    for j in 0..n_cands {
        cands.push(CruiseTopCandidateView {
            flight_id: cand_fid_arr.value(j),
            callsign: cand_callsign_arr.value(j),
            aircraft_type: typecodes[j * 4..j * 4 + 4].try_into().expect(
                "FixedSizeBinary(4) verified above carries four typecode bytes per candidate",
            ),
            peak_lmax_25m_db: cand_lmax_arr.value(j),
            altitude_m: cand_alt_arr.value(j),
        });
    }
    Ok(CruiseBatchViews {
        lon,
        lat,
        class,
        rep_profile_idx: rep_pi,
        fl_bin,
        period,
        sum_length_m: sum_len,
        heading_bin: heading,
        rep_alt_m: rep_alt,
        rep_speed_kt: rep_speed,
        unique_count,
        source_id,
        origin,
        secondary_only,
        cand_offsets,
        cands,
        n,
    })
}

/// Borrowed row views over the accumulator's batches.
/// `as_row_views` hands noise-compute the `Vec<CruiseRowView<'a>>` it expects.
pub struct CruiseViewSlices<'a> {
    batches: &'a [CruiseBatchViews<'a>],
}

impl<'a> CruiseViewSlices<'a> {
    pub fn as_row_views(&'a self) -> Vec<CruiseRowView<'a>> {
        let n: usize = self.batches.iter().map(CruiseBatchViews::len).sum();
        let mut rows = Vec::with_capacity(n);
        for batch in self.batches {
            rows.extend((0..batch.len()).map(|i| batch.row_view(i)));
        }
        rows
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

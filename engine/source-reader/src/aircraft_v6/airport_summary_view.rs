//! Strict local airport summaries: preserve global unions without counting replicated rows twice.

use arrow::array::*;
use arrow::record_batch::RecordBatch;
use noise_compute::compute::aircraft_v6::{
    airport_traffic::{AirportSummaryEntry, AirportSummaryLookup},
    NUM_GSE_CLASSES,
};

use super::columns::required_array;
use square_store::aircraft_contract::AIRPORT_SUMMARY_CONTRACT;

#[derive(Debug, Default)]
pub struct AirportSummaryAccum {
    lookup: AirportSummaryLookup,
}

impl AirportSummaryAccum {
    pub fn new(batches: &[RecordBatch]) -> Result<Self, String> {
        super::assert_schema_version("airport_summary.arrow", batches)?;
        super::assert_metadata_value(
            "airport_summary.arrow",
            batches,
            "airport_summary_contract",
            AIRPORT_SUMMARY_CONTRACT,
            "re-run Stage 2C",
        )?;
        let mut accum = Self::default();
        for batch in batches {
            accum.absorb(batch)?;
        }
        Ok(accum)
    }

    pub fn require_traffic(&self, traffic: &[RecordBatch]) -> Result<(), String> {
        for batch in traffic {
            let keys =
                required_array::<StringArray>(batch.column_by_name("airport_key"), "airport_key")?;
            for i in 0..keys.len() {
                if !self.lookup.contains_key(keys.value(i)) {
                    return Err(format!(
                        "airport_summary.arrow missing airport {} required by this cell's traffic",
                        keys.value(i)
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn merge_square(
        &mut self,
        summary: &square_store::store::LazyArrow,
        traffic: &[RecordBatch],
    ) -> Result<(), String> {
        let local = Self::new(&summary.batches_all()?)?;
        local.require_traffic(traffic)?;
        for (key, value) in local.lookup {
            if self
                .lookup
                .get(&key)
                .is_some_and(|existing| *existing != value)
            {
                return Err(format!(
                    "airport_summary.arrow disagrees across cells for airport {key}"
                ));
            }
            self.lookup.insert(key, value);
        }
        Ok(())
    }

    fn absorb(&mut self, batch: &RecordBatch) -> Result<(), String> {
        let n = batch.num_rows();
        let airport_key =
            required_array::<StringArray>(batch.column_by_name("airport_key"), "airport_key")?;
        let arr = required_array::<UInt32Array>(
            batch.column_by_name("airport_unique_arr_count"),
            "airport_unique_arr_count",
        )?;
        let dep = required_array::<UInt32Array>(
            batch.column_by_name("airport_unique_dep_count"),
            "airport_unique_dep_count",
        )?;
        let gse_list = required_array::<FixedSizeListArray>(
            batch.column_by_name("airport_unique_gse_count_per_class"),
            "airport_unique_gse_count_per_class",
        )?;
        let ops_list = required_array::<FixedSizeListArray>(
            batch.column_by_name("airport_unique_ops_count_per_kind"),
            "airport_unique_ops_count_per_kind",
        )?;
        let ga_arr = required_array::<UInt32Array>(
            batch.column_by_name("airport_unique_ga_arr_count"),
            "airport_unique_ga_arr_count",
        )?;
        let ga_dep = required_array::<UInt32Array>(
            batch.column_by_name("airport_unique_ga_dep_count"),
            "airport_unique_ga_dep_count",
        )?;
        let ga_ops_list = required_array::<FixedSizeListArray>(
            batch.column_by_name("airport_unique_ga_ops_count_per_kind"),
            "airport_unique_ga_ops_count_per_kind",
        )?;
        if gse_list.value_length() != NUM_GSE_CLASSES as i32
            || ops_list.value_length() != 3
            || ga_ops_list.value_length() != 3
        {
            return Err("airport_summary fixed-size list width mismatch".into());
        }
        let gse_buf = required_array::<UInt32Array>(
            Some(gse_list.values()),
            "airport_unique_gse_count_per_class.item",
        )?
        .values();
        let ops_buf = required_array::<UInt32Array>(
            Some(ops_list.values()),
            "airport_unique_ops_count_per_kind.item",
        )?
        .values();
        let ga_ops_buf = required_array::<UInt32Array>(
            Some(ga_ops_list.values()),
            "airport_unique_ga_ops_count_per_kind.item",
        )?
        .values();
        self.lookup.reserve(n);
        for i in 0..n {
            let lo_g = i * NUM_GSE_CLASSES;
            let mut gse = [0u32; NUM_GSE_CLASSES];
            gse.copy_from_slice(&gse_buf[lo_g..lo_g + NUM_GSE_CLASSES]);
            let lo_o = i * 3;
            let mut ops = [0u32; 3];
            ops.copy_from_slice(&ops_buf[lo_o..lo_o + 3]);
            let mut ga_ops = [0u32; 3];
            ga_ops.copy_from_slice(&ga_ops_buf[lo_o..lo_o + 3]);
            if self
                .lookup
                .insert(
                    airport_key.value(i).to_string(),
                    AirportSummaryEntry {
                        arr_count: arr.value(i),
                        dep_count: dep.value(i),
                        gse_count_per_class: gse,
                        ops_count_per_kind: ops,
                        ga_arr_count: ga_arr.value(i),
                        ga_dep_count: ga_dep.value(i),
                        ga_ops_count_per_kind: ga_ops,
                    },
                )
                .is_some()
            {
                return Err(format!(
                    "airport_summary duplicates key {}",
                    airport_key.value(i)
                ));
            }
        }
        Ok(())
    }

    /// Counts are global unions, not sums of the loaded cells' copies.
    pub fn lookup(&self) -> &AirportSummaryLookup {
        &self.lookup
    }
}

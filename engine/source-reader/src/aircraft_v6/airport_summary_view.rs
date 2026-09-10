//! Global airport unions from the `qm_airport_summaries` footer of every loaded traffic file.

use arrow::array::*;
use arrow::datatypes::Schema;
use arrow::record_batch::RecordBatch;
use noise_compute::compute::aircraft_v6::airport_traffic::{
    decode_airport_summaries, AirportSummaryLookup,
};

use super::columns::required_array;
use square_store::aircraft_contract::{AIRPORT_SUMMARIES_KEY, AIRPORT_TRAFFIC_CONTRACT};

#[derive(Debug, Default)]
pub struct AirportSummaryAccum {
    lookup: AirportSummaryLookup,
}

impl AirportSummaryAccum {
    /// Parse one traffic file's footer at square open. An old contract names
    /// the re-extract; a current file without the key is an incomplete Stage 2C.
    pub fn parse_footer(schema: &Schema) -> Result<AirportSummaryLookup, String> {
        let metadata = schema.metadata();
        let contract = metadata.get("airport_traffic_contract").map(String::as_str);
        if contract != Some(AIRPORT_TRAFFIC_CONTRACT) {
            return Err(format!(
                "airport_traffic.arrow airport_traffic_contract mismatch (expected {AIRPORT_TRAFFIC_CONTRACT}, got {contract:?}) — re-extract aircraft pipeline"
            ));
        }
        let json = metadata.get(AIRPORT_SUMMARIES_KEY).ok_or_else(|| {
            format!(
                "airport_traffic.arrow has no {AIRPORT_SUMMARIES_KEY}: the Stage 2C reduce did not stamp it; re-run Stage 2C"
            )
        })?;
        decode_airport_summaries(json).map_err(|error| format!("airport_traffic.arrow: {error}"))
    }

    pub fn require_traffic(&self, traffic: &[RecordBatch]) -> Result<(), String> {
        for batch in traffic {
            let keys =
                required_array::<StringArray>(batch.column_by_name("airport_key"), "airport_key")?;
            for i in 0..keys.len() {
                if !self.lookup.contains_key(keys.value(i)) {
                    return Err(format!(
                        "{AIRPORT_SUMMARIES_KEY} missing airport {} required by this cell's traffic",
                        keys.value(i)
                    ));
                }
            }
        }
        Ok(())
    }

    /// Merge one square's footer; the same airport must carry identical
    /// counts in every cell that owns part of its traffic.
    pub fn merge_square(&mut self, schema: &Schema, traffic: &[RecordBatch]) -> Result<(), String> {
        let local = Self {
            lookup: Self::parse_footer(schema)?,
        };
        local.require_traffic(traffic)?;
        for (key, value) in local.lookup {
            if self
                .lookup
                .get(&key)
                .is_some_and(|existing| *existing != value)
            {
                return Err(format!(
                    "{AIRPORT_SUMMARIES_KEY} disagrees across cells for airport {key}"
                ));
            }
            self.lookup.insert(key, value);
        }
        Ok(())
    }

    /// Counts are global unions, not sums of the loaded cells' copies.
    pub fn lookup(&self) -> &AirportSummaryLookup {
        &self.lookup
    }
}

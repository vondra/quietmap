//! Producer-stamped spill row, flight and candidate counts for allocation admission.

use super::cruise_spill::CruiseSpillRow;
use anyhow::{Context, Result};
use arrow::ipc::reader::FileReader;
use std::{fs::File, io::BufReader, path::Path};

#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct CruiseSpillCounts {
    pub rows: usize,
    pub fids: usize,
    pub candidates: usize,
    pub callsign_bytes: usize,
}

impl CruiseSpillCounts {
    pub fn from_rows(rows: &[CruiseSpillRow]) -> Self {
        Self {
            rows: rows.len(),
            fids: rows.iter().map(|row| row.fid_set.len()).sum(),
            candidates: rows.iter().map(|row| row.top_candidates.len()).sum(),
            callsign_bytes: rows
                .iter()
                .flat_map(|row| &row.top_candidates)
                .map(|candidate| candidate.callsign.len())
                .sum(),
        }
    }

    pub fn encoded_buffers_bytes(self) -> usize {
        // Arrow IPC writes validity bitmaps even for these non-null arrays:
        // 16 row arrays, the flight-id child, and six candidate struct/children.
        54 * self.rows
            + 8 * self.fids
            + 24 * self.candidates
            + self.callsign_bytes
            + 12
            + 16 * self.rows.div_ceil(8)
            + self.fids.div_ceil(8)
            + 6 * self.candidates.div_ceil(8)
    }

    pub(super) fn fields(self) -> [(&'static str, usize); 4] {
        [
            ("spill_rows", self.rows),
            ("spill_fids", self.fids),
            ("spill_candidates", self.candidates),
            ("spill_callsign_bytes", self.callsign_bytes),
        ]
    }

    pub fn read(path: &Path) -> Result<Self> {
        let reader = FileReader::try_new(BufReader::new(File::open(path)?), None)?;
        let schema = reader.schema();
        let get = |name| -> Result<usize> {
            Ok(schema
                .metadata()
                .get(name)
                .with_context(|| format!("missing spill count {name}"))?
                .parse()?)
        };
        Ok(Self {
            rows: get("spill_rows")?,
            fids: get("spill_fids")?,
            candidates: get("spill_candidates")?,
            callsign_bytes: get("spill_callsign_bytes")?,
        })
    }

    pub fn merge(&mut self, other: Self) {
        self.rows += other.rows;
        self.fids += other.fids;
        self.candidates += other.candidates;
        self.callsign_bytes += other.callsign_bytes;
    }
}

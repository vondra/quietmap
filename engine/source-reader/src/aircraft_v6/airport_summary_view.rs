//! `airport_summary.arrow` sidecar loader (v5). Reads the global
//! single-row-per-airport file produced by Stage 2C v5 reduce phase
//! and builds the `AirportSummaryLookup` HashMap noise-compute's
//! `airport_traffic::run` consumes.
//!
//! Missing file → caller MUST propagate `None` to the popup so the
//! popup refuses to display airport-level arr/dep counts (Codex C4 and
//! Claude W1); no silent fallback to per-row sum, which would over-count
//! rotations crossing N microsegments by ~N×.

use std::path::Path;

use arrow::array::Array;
use arrow::record_batch::RecordBatch;
use noise_compute::compute::aircraft_v6::{
    airport_traffic::{AirportSummaryEntry, AirportSummaryLookup},
    NUM_GSE_CLASSES,
};

use super::columns::{col_fixed_size_list, col_str, col_u32};

/// The parsed sidecar, already in the shape the popup compute consumes.
///
/// This used to hold two parallel `Vec`s and build the lookup map from
/// them on demand; because the map borrowed the key slices it could not
/// outlive one call, so every click near an airport re-hashed all ~50 k
/// airports (measured 2026-08-05). The map is now built once, at parse
/// time, and shared through the same `Arc` as the rest of the sidecar.
pub struct AirportSummaryAccum {
    lookup: AirportSummaryLookup,
}

impl AirportSummaryAccum {
    pub fn new(batches: &[RecordBatch]) -> Self {
        let mut accum = AirportSummaryAccum {
            lookup: AirportSummaryLookup::new(),
        };
        for batch in batches {
            accum.absorb(batch);
        }
        accum
    }

    fn absorb(&mut self, batch: &RecordBatch) {
        let n = batch.num_rows();
        let (
            Some(airport_key),
            Some(arr),
            Some(dep),
            Some(gse_list),
            Some(ops_list),
            Some(ga_arr),
            Some(ga_dep),
            Some(ga_ops_list),
        ) = (
            col_str(batch, "airport_key"),
            col_u32(batch, "airport_unique_arr_count"),
            col_u32(batch, "airport_unique_dep_count"),
            col_fixed_size_list(batch, "airport_unique_gse_count_per_class"),
            col_fixed_size_list(batch, "airport_unique_ops_count_per_kind"),
            col_u32(batch, "airport_unique_ga_arr_count"),
            col_u32(batch, "airport_unique_ga_dep_count"),
            col_fixed_size_list(batch, "airport_unique_ga_ops_count_per_kind"),
        )
        else {
            return;
        };
        if gse_list.value_length() != NUM_GSE_CLASSES as i32
            || ops_list.value_length() != 3
            || ga_ops_list.value_length() != 3
        {
            return;
        }
        let u32_buf = |list: &arrow::array::FixedSizeListArray| {
            list.values()
                .as_any()
                .downcast_ref::<arrow::array::UInt32Array>()
                .map(|a| a.values().to_vec())
        };
        let (Some(gse_buf), Some(ops_buf), Some(ga_ops_buf)) =
            (u32_buf(gse_list), u32_buf(ops_list), u32_buf(ga_ops_list))
        else {
            return;
        };
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
            self.lookup.insert(
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
            );
        }
    }

    /// The lookup the popup compute consumes. Free — the map was built
    /// when the sidecar was parsed, and the parse is process-cached.
    pub fn lookup(&self) -> &AirportSummaryLookup {
        &self.lookup
    }
}

/// Process-wide cache for the parsed sidecar, keyed by (path, mtime, len).
/// The popup previously re-read and re-parsed the ~11 MB global file on
/// EVERY query near an airport (owner report 2026-07-10). On an idle box the
/// parse is a small share of aircraft_ground_ms (compute dominates); what the
/// cache removes is the per-query disk I/O and its jitter under load — the
/// file only changes on an aircraft re-extract, at which point the mtime/len
/// key rolls the cache naturally. One instance per worker (each worker loads
/// its own addon copy).
type SummaryCacheEntry = (
    std::path::PathBuf,
    std::time::SystemTime,
    u64,
    std::sync::Arc<AirportSummaryAccum>,
);
static SUMMARY_CACHE: std::sync::RwLock<Option<SummaryCacheEntry>> = std::sync::RwLock::new(None);

/// Cached wrapper around [`load_airport_summary`] — same absent/`Err`
/// semantics; hits clone an `Arc` instead of touching the disk.
pub fn load_airport_summary_cached(
    path: &Path,
) -> Result<Option<std::sync::Arc<AirportSummaryAccum>>, String> {
    let Ok(meta) = std::fs::metadata(path) else {
        // Missing/unstattable: defer to the uncached path (which maps
        // absent → Ok(None)) and drop any stale entry for this path.
        let mut w = SUMMARY_CACHE.write().unwrap();
        if w.as_ref().is_some_and(|(p, ..)| p == path) {
            *w = None;
        }
        return load_airport_summary(path).map(|o| o.map(std::sync::Arc::new));
    };
    let mtime = meta
        .modified()
        .map_err(|e| format!("mtime {}: {e}", path.display()))?;
    let len = meta.len();
    if let Some((p, m, l, accum)) = SUMMARY_CACHE.read().unwrap().as_ref() {
        if p == path && *m == mtime && *l == len {
            return Ok(Some(accum.clone()));
        }
    }
    let loaded = load_airport_summary(path)?.map(std::sync::Arc::new);
    if let Some(accum) = &loaded {
        *SUMMARY_CACHE.write().unwrap() = Some((path.to_path_buf(), mtime, len, accum.clone()));
    }
    Ok(loaded)
}

/// Load `airport_summary.arrow` from disk. Returns `Ok(None)` when the
/// file is absent (popup MUST then refuse to populate airport-level
/// counts), `Err` only on actual read failure.
pub fn load_airport_summary(path: &Path) -> Result<Option<AirportSummaryAccum>, String> {
    if !path.exists() {
        return Ok(None);
    }
    use arrow::ipc::reader::FileReader;
    use std::fs::File;
    use std::io::BufReader;
    let f = File::open(path)
        .map_err(|e| format!("open airport_summary.arrow at {}: {e}", path.display()))?;
    let r = FileReader::try_new(BufReader::new(f), None).map_err(|e| {
        format!(
            "arrow ipc {} (re-extract aircraft pipeline?): {e}",
            path.display()
        )
    })?;
    let schema = r.schema();
    // Defence-in-depth: check both `schema_version` and the dimensional
    // `airport_summary_contract` stamps. Per /gg Codex audit, a corrupt
    // sidecar could carry one stamp but not the other; reject either
    // mismatch loud so the popup gets `Err` rather than zero counts.
    let sv = schema.metadata().get("schema_version").map(String::as_str);
    if sv != Some(super::EXPECTED_SCHEMA_VERSION) {
        return Err(format!(
            "{} schema_version mismatch (expected {}, got {:?}) \
             — re-extract aircraft pipeline",
            path.display(),
            super::EXPECTED_SCHEMA_VERSION,
            sv
        ));
    }
    let v = schema.metadata().get("airport_summary_contract");
    if v.map(String::as_str) != Some("airport_summary_v2") {
        return Err(format!(
            "{} airport_summary_contract mismatch (expected airport_summary_v2, got {:?}) \
             — re-extract aircraft pipeline (GA 365-day hybrid split)",
            path.display(),
            v
        ));
    }
    let mut batches = Vec::new();
    for b in r {
        let batch = b.map_err(|e| format!("read batch from {}: {e}", path.display()))?;
        batches.push(batch);
    }
    Ok(Some(AirportSummaryAccum::new(&batches)))
}

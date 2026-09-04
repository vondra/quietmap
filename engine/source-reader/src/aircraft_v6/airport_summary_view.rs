//! Strict global airport movement union loader with a per-file cache.

use std::path::Path;

use arrow::array::*;
use arrow::record_batch::RecordBatch;
use noise_compute::compute::aircraft_v6::{
    airport_traffic::{AirportSummaryEntry, AirportSummaryLookup},
    NUM_GSE_CLASSES,
};

use super::columns::required_array;
use square_store::aircraft_contract::AIRPORT_SUMMARY_CONTRACT;

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
    pub fn new(batches: &[RecordBatch]) -> Result<Self, String> {
        let mut accum = AirportSummaryAccum {
            lookup: AirportSummaryLookup::new(),
        };
        for batch in batches {
            accum.absorb(batch)?;
        }
        Ok(accum)
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
    use arrow::ipc::reader::FileReader;
    use std::fs::File;
    use std::io::BufReader;
    let f = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "open airport_summary.arrow at {}: {error}",
                path.display()
            ))
        }
    };
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
    if sv != Some(super::SCHEMA_VERSION) {
        return Err(format!(
            "{} schema_version mismatch (expected {}, got {:?}) \
             — re-extract aircraft pipeline",
            path.display(),
            super::SCHEMA_VERSION,
            sv
        ));
    }
    let v = schema.metadata().get("airport_summary_contract");
    if v.map(String::as_str) != Some(AIRPORT_SUMMARY_CONTRACT) {
        return Err(format!(
            "{} airport_summary_contract mismatch (expected {AIRPORT_SUMMARY_CONTRACT}, got {:?}) \
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
    if batches.is_empty() {
        batches.push(RecordBatch::new_empty(schema));
    }
    Ok(Some(AirportSummaryAccum::new(&batches)?))
}

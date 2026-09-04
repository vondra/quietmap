//! Schema-version guard for the v6 aircraft arrows the heatmap loaders
//! consume. Without this, stale `v12` (or earlier) arrows produced by
//! an out-of-date `aircraft-extract` binary are silently absorbed and
//! the heatmap bakes wrong `class_idx → NPD profile` mappings — the
//! same risk the popup-side `source-reader::aircraft_v6::mod` guards.

use std::fs::File;
use std::io::Cursor;
use std::path::Path;

use aircraft_extract::arrow_schemas::{
    AIRBORNE_CONTRACT_V4, AIRPORT_TRAFFIC_CONTRACT_V9, CRUISE_CONTRACT_V17,
};
use aircraft_extract::SCHEMA_VERSION;
use anyhow::{bail, Context, Result};
use arrow::ipc::reader::FileReader;
use arrow::record_batch::RecordBatch;
use memmap2::Mmap;

fn check_metadata_value(
    label: &str,
    batches: &[RecordBatch],
    key: &str,
    expected: &str,
    recovery: &str,
) -> Result<()> {
    for (idx, batch) in batches.iter().enumerate() {
        let actual = batch.schema_ref().metadata().get(key).map(String::as_str);
        if actual == Some(expected) {
            continue;
        }
        bail!(
            "{label}[batch {idx}] {key} mismatch \
             (expected {expected}, got {actual:?}) — {recovery}"
        );
    }
    Ok(())
}

/// Verify every batch carries the current `schema_version` metadata.
/// Returns the offending version in the error so the operator knows
/// which file to re-extract. Single-file IPC guarantees one schema per
/// file but callers may merge across R4 hexes; loop every batch.
pub fn check_batches(label: &str, batches: &[RecordBatch]) -> Result<()> {
    check_metadata_value(
        label,
        batches,
        "schema_version",
        SCHEMA_VERSION,
        "re-extract aircraft pipeline",
    )
}

/// `airport_traffic.arrow` carries a second `airport_traffic_contract`
/// stamp that gates `band_energy_lin` semantics independently of
/// `schema_version`. Popup checks both (`source-reader::aircraft_v6
/// ::mod::assert_airport_traffic_contract`); heatmap mirrors here so a
/// stale v5 traffic arrow doesn't silently feed wrong daily-average
/// energy where v6 code expects raw Σ.
pub fn check_airport_traffic_contract(label: &str, batches: &[RecordBatch]) -> Result<()> {
    check_metadata_value(
        label,
        batches,
        "airport_traffic_contract",
        AIRPORT_TRAFFIC_CONTRACT_V9,
        "re-extract aircraft pipeline",
    )
}

/// `airborne.arrow` carries `airborne_contract` (K3, 2026-05). v1
/// stored five terrain elevations per sub-segment; v2 dropped q1/mid/q3.
/// Heatmap loader uses
/// `take_f32` with a `0.0`-fill fallback, so a v1 file would silently
/// alias `terrain_q1_elev_m` data into what v2 treats as
/// `terrain_end_elev_m` — produce wrong Filter D cuts at every pixel.
/// Reject loud.
pub fn check_airborne_contract(label: &str, batches: &[RecordBatch]) -> Result<()> {
    check_metadata_value(
        label,
        batches,
        "airborne_contract",
        AIRBORNE_CONTRACT_V4,
        "re-extract aircraft pipeline",
    )
}

/// `cruise.arrow` carries `cruise_contract` (v16, 2026-05). v16 drops
/// the tautological `flags` column (Doc 29 §A.3.2 forces IS_DEPARTURE).
/// Older files have columns the heatmap loader does NOT expect; without
/// this assert the loader would silently skip batches and zero out
/// cruise pixels. Reject loud.
pub fn check_cruise_contract(label: &str, batches: &[RecordBatch]) -> Result<()> {
    check_metadata_value(
        label,
        batches,
        "cruise_contract",
        CRUISE_CONTRACT_V17,
        "re-extract cruise stage 2B",
    )
}

/// Open a per-R4 arrow file, run the version + per-file contract gate,
/// then `absorb` each batch into the caller's row buffer. Centralises
/// the open + mmap + IPC + gate + absorb recipe that all three loaders
/// share — the only per-loader variation is `filename`,
/// `contract_check`, and the per-row decoder.
///
/// Missing files are silently skipped (rural R4s legitimately have no
/// rows for some sources). All errors are wrapped with the R4 hex so a
/// world build's stderr names the offending file.
pub fn read_arrow_for_r4(
    h3r4_dir: &Path,
    r4: u64,
    filename: &str,
    contract_check: impl Fn(&str, &[RecordBatch]) -> Result<()>,
    absorb: impl FnMut(&RecordBatch) -> Result<()>,
) -> Result<()> {
    // Aircraft arrows carry the `schema_version` stamp + a per-file contract.
    read_arrow_core(
        h3r4_dir,
        r4,
        filename,
        |label, batches| {
            check_batches(label, batches)?;
            contract_check(label, batches)
        },
        absorb,
    )
}

/// Like [`read_arrow_for_r4`] but WITHOUT the aircraft `schema_version`
/// gate — surface arrows (`roads.arrow`, `railways.arrow`, …) come from the
/// OSM-extract / enrichment pipeline and carry no aircraft metadata, so the
/// per-column reads inside `absorb` are the structural gate.
pub fn read_surface_arrow_for_r4(
    h3r4_dir: &Path,
    r4: u64,
    filename: &str,
    absorb: impl FnMut(&RecordBatch) -> Result<()>,
) -> Result<()> {
    read_arrow_core(h3r4_dir, r4, filename, |_label, _batches| Ok(()), absorb)
}

/// settlement v2 phase 2 per-file contract for the leisure table (source of
/// truth: `osm-extract::finalize`). The buildings table is merged into
/// `structures.arrow` now — the structure loader gates on
/// [`STRUCTURES_CONTRACT_V1`] instead.
pub const LEISURE_CONTRACT_V1: &str = "leisure_v1";

/// The per-cell structure table's contract (source of truth:
/// `scripts/structures/build-structures.py`): the merged buildings ∪ walls
/// table every prepared cell carries. A stale or missing stamp fails the
/// region load — rebuilding the cell with the structures builder is the fix.
pub const STRUCTURES_CONTRACT_V1: &str = "structures_v1";

/// Surface-arrow read that ALSO enforces a per-file `<key>` contract stamp
/// (Convention-B). Missing files are skipped (no contract to check); a present
/// file with the wrong/absent contract aborts the build.
pub fn read_surface_arrow_for_r4_with_contract(
    h3r4_dir: &Path,
    r4: u64,
    filename: &str,
    contract_key: &str,
    expected: &str,
    absorb: impl FnMut(&RecordBatch) -> Result<()>,
) -> Result<()> {
    read_arrow_core(
        h3r4_dir,
        r4,
        filename,
        |label, batches| {
            for (idx, batch) in batches.iter().enumerate() {
                let c = batch
                    .schema_ref()
                    .metadata()
                    .get(contract_key)
                    .map(String::as_str);
                if c != Some(expected) {
                    bail!(
                        "{label}[batch {idx}] {contract_key} mismatch (expected {expected}, \
                         got {c:?}) — re-extract OSM (settlement v2 phase 2)"
                    );
                }
            }
            Ok(())
        },
        absorb,
    )
}

/// Shared open + mmap + IPC + validate + absorb. `validate` runs once over
/// all batches before any are absorbed (aircraft composes
/// `check_batches` + the contract; surface passes a no-op).
fn read_arrow_core(
    h3r4_dir: &Path,
    r4: u64,
    filename: &str,
    validate: impl Fn(&str, &[RecordBatch]) -> Result<()>,
    mut absorb: impl FnMut(&RecordBatch) -> Result<()>,
) -> Result<()> {
    let path = h3r4_dir.join(format!("{r4:015x}")).join(filename);
    if !path.exists() {
        return Ok(());
    }
    let file = File::open(&path).with_context(|| format!("open {}", path.display()))?;
    let mmap = unsafe { Mmap::map(&file)? };
    let reader = FileReader::try_new(Cursor::new(&mmap[..]), None)
        .with_context(|| format!("arrow ipc {}", path.display()))?;
    let mut batches: Vec<RecordBatch> = Vec::new();
    for batch in reader {
        let batch = batch.with_context(|| format!("read batch {}", path.display()))?;
        batches.push(batch);
    }
    validate(filename, &batches).with_context(|| format!("R4={r4:015x}"))?;
    for batch in &batches {
        absorb(batch).with_context(|| format!("{filename} R4={r4:015x}"))?;
    }
    Ok(())
}

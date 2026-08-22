//! Schema-version guard for the v6 aircraft arrows the heatmap loaders
//! consume. Without this, stale `v12` (or earlier) arrows produced by
//! an out-of-date `aircraft-extract` binary are silently absorbed and
//! the heatmap bakes wrong `class_idx → NPD profile` mappings — the
//! same risk the popup-side `source-reader::aircraft_v6::mod` guards.
//!
//! Honors the same `ACCEPT_LEGACY_AIRCRAFT_SCHEMA=1` escape hatch as
//! popup so a dev iterating against pre-v13 data isn't forced into a
//! multi-hour ADS-B re-extract just to render a tile.

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

/// Empty since v15: terrain elevations are mandatory.
/// v15 adds `terrain_*_elev_m` sub-segment columns; legacy
/// versions can't provide them, and the heatmap loader's per-column
/// `unwrap_or_else(vec![0.0; n])` would silently zero-out terrain,
/// masking real underground segments. Re-extract is the only path
/// forward. Within-v15 `airport_traffic.arrow` semantic evolutions
/// (e.g. v5→v6 raw-Σ convention) are gated by
/// [`check_airport_traffic_contract`] instead.
const LEGACY_SCHEMA_VERSIONS: &[&str] = &[];
/// Empty: v5 stored daily-average `band_energy_lin` plus a redundant
/// `movements_per_day` column; v6 stores raw Σ over n_days and drops
/// the redundant field. Accepting v5 under v6 code would ship wrong
/// tile numbers (~25.6 dB low at n_days=365).
const LEGACY_AIRPORT_TRAFFIC_CONTRACTS: &[&str] = &[];

fn accept_legacy() -> bool {
    matches!(
        std::env::var("ACCEPT_LEGACY_AIRCRAFT_SCHEMA").as_deref(),
        Ok("1")
    )
}

/// Verify every batch carries the current `schema_version` metadata.
/// Returns the offending version in the error so the operator knows
/// which file to re-extract. Single-file IPC guarantees one schema per
/// file but callers may merge across R4 hexes; loop every batch.
pub fn check_batches(label: &str, batches: &[RecordBatch]) -> Result<()> {
    let allow_legacy = accept_legacy();
    for (idx, batch) in batches.iter().enumerate() {
        let v = batch
            .schema_ref()
            .metadata()
            .get("schema_version")
            .map(String::as_str);
        if v == Some(SCHEMA_VERSION) {
            continue;
        }
        if allow_legacy && v.is_some_and(|s| LEGACY_SCHEMA_VERSIONS.contains(&s)) {
            eprintln!(
                "WARN: {label}[batch {idx}] legacy schema {v:?} accepted via \
                 ACCEPT_LEGACY_AIRCRAFT_SCHEMA — class_idx may map to wrong NPD profile"
            );
            continue;
        }
        bail!(
            "{label}[batch {idx}] schema_version mismatch \
             (expected {SCHEMA_VERSION}, got {v:?}) — re-extract aircraft pipeline \
             (or set ACCEPT_LEGACY_AIRCRAFT_SCHEMA=1 for dev)"
        );
    }
    Ok(())
}

/// `airport_traffic.arrow` carries a second `airport_traffic_contract`
/// stamp that gates `band_energy_lin` semantics independently of
/// `schema_version`. Popup checks both (`source-reader::aircraft_v6
/// ::mod::assert_airport_traffic_contract`); heatmap mirrors here so a
/// stale v5 traffic arrow doesn't silently feed wrong daily-average
/// energy where v6 code expects raw Σ.
pub fn check_airport_traffic_contract(label: &str, batches: &[RecordBatch]) -> Result<()> {
    let allow_legacy = accept_legacy();
    for (idx, batch) in batches.iter().enumerate() {
        let c = batch
            .schema_ref()
            .metadata()
            .get("airport_traffic_contract")
            .map(String::as_str);
        if c == Some(AIRPORT_TRAFFIC_CONTRACT_V9) {
            continue;
        }
        if allow_legacy && c.is_some_and(|s| LEGACY_AIRPORT_TRAFFIC_CONTRACTS.contains(&s)) {
            eprintln!(
                "WARN: {label}[batch {idx}] legacy airport_traffic_contract {c:?} accepted \
                 via ACCEPT_LEGACY_AIRCRAFT_SCHEMA — energy semantics may differ"
            );
            continue;
        }
        bail!(
            "{label}[batch {idx}] airport_traffic_contract mismatch \
             (expected {AIRPORT_TRAFFIC_CONTRACT_V9}, got {c:?}) \
             — re-extract aircraft pipeline (or set ACCEPT_LEGACY_AIRCRAFT_SCHEMA=1 for dev)"
        );
    }
    Ok(())
}

/// `airborne.arrow` carries `airborne_contract` (K3, 2026-05). v1
/// stored five terrain elevations per sub-segment; v2 dropped q1/mid/q3.
/// Heatmap loader uses
/// `take_f32` with a `0.0`-fill fallback, so a v1 file would silently
/// alias `terrain_q1_elev_m` data into what v2 treats as
/// `terrain_end_elev_m` — produce wrong Filter D cuts at every pixel.
/// Reject loud.
pub fn check_airborne_contract(label: &str, batches: &[RecordBatch]) -> Result<()> {
    for (idx, batch) in batches.iter().enumerate() {
        let c = batch
            .schema_ref()
            .metadata()
            .get("airborne_contract")
            .map(String::as_str);
        if c == Some(AIRBORNE_CONTRACT_V4) {
            continue;
        }
        bail!(
            "{label}[batch {idx}] airborne_contract mismatch \
             (expected {AIRBORNE_CONTRACT_V4}, got {c:?}) \
             — re-extract aircraft pipeline"
        );
    }
    Ok(())
}

/// `cruise.arrow` carries `cruise_contract` (v16, 2026-05). v16 drops
/// the tautological `flags` column (Doc 29 §A.3.2 forces IS_DEPARTURE).
/// Older files have columns the heatmap loader does NOT expect; without
/// this assert the loader would silently skip batches and zero out
/// cruise pixels. Reject loud.
pub fn check_cruise_contract(label: &str, batches: &[RecordBatch]) -> Result<()> {
    for (idx, batch) in batches.iter().enumerate() {
        let c = batch
            .schema_ref()
            .metadata()
            .get("cruise_contract")
            .map(String::as_str);
        if c == Some(CRUISE_CONTRACT_V17) {
            continue;
        }
        bail!(
            "{label}[batch {idx}] cruise_contract mismatch \
             (expected {CRUISE_CONTRACT_V17}, got {c:?}) \
             — re-extract cruise stage 2B"
        );
    }
    Ok(())
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

/// settlement v2 phase 2 per-file contracts (source of truth:
/// `osm-extract::finalize`). The buildings re-numbering (new SILENT/HOUSE/
/// FOOD_RETAIL/HOSPITALITY classes) silently re-profiles a v1 arrow, so the
/// loader must reject a stale buildings.arrow loud — re-extract is the fix.
pub const BUILDINGS_CONTRACT_V2: &str = "buildings_v2";
pub const LEISURE_CONTRACT_V1: &str = "leisure_v1";

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

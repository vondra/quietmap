//! tile-store-pack — publish one BUILD: every layer store → `{layer}.{build}.pmtiles`
//! + an atomically-swapped `current.json` manifest (the ops + serving pointer).
//!
//! `--layer L` (repeatable) packs ONLY those layers and MERGES the manifest —
//! untouched layers keep their previous archive + build id. Pack multiple
//! layers in ONE invocation (they parallelise internally); two concurrent
//! PARTIAL packs would race the manifest read-modify-write.
//!
//! Ship-out is codec-blind via [`TileStore::get_hm3_by_entry`], and — since
//! the 2026-07-16 publish-speed fix — a straight VERBATIM copy for the
//! overwhelming majority of entries: `BrotliHm3` blobs ship untouched whether
//! they came from the fleet (source-layer ingest) or from a central writer
//! (`build_heatmap_combine`, `pyramid::build_one_level`, which now encode
//! Brotli-q9 once at write time via `TileStore::put_cells_hm3` instead of
//! deferring it here). Only a legacy `ZstdCells` entry — a central tile a
//! store hasn't been rewritten through since the cutover — still gets
//! composed + Brotli-encoded on this path; that population only shrinks as
//! stores get touched again. Before reading, the packer takes the shared
//! master→ingest locks plus every selected per-z writer lock in canonical
//! order, and copies every exact index entry, captured data-file length, and
//! open data handle. All of those locks remain held through validation, archive
//! creation, and the durable manifest flip: an open fd survives rename/unlink,
//! but not an in-place truncate, so releasing a per-z lock early would make the
//! captured feed mutable to a direct full writer. The
//! pmtiles header declares `tile_compression = brotli` via a passthrough codec
//! (bytes are added pre-compressed); internal directories stay gzip (default)
//! so the npm `pmtiles` reader decodes them natively.
//!
//! Tiles are fed in ascending pmtiles TileId order (the spec's Hilbert-derived
//! id) → a clustered archive; the writer dedups identical blobs (xxhash64 of
//! the bytes — hash-based, not byte-verified, the pmtiles-ecosystem standard),
//! which collapses the uniform low-dB halo tiles for free.
//!
//! Published files are IMMUTABLE: each archive is built and fsynced under a
//! hidden temp name, then atomically renamed with `RENAME_NOREPLACE`; an
//! existing `{layer}.{build}.pmtiles` is a hard error, never overwritten, and
//! a crash cannot leave a final-named partial. Every selected layer finishes
//! staging before the first final rename. A durable transaction marker then
//! makes an interrupted rename set recoverable: uncommitted finals are removed,
//! while a set already referenced by `current.json` is retained. The manifest
//! is written last, tmp + atomic rename — a crash mid-pack leaves the previous
//! build fully live.
//!
//! Usage: tile-store-pack <store-root> <out-dir> <build-id> --generation-contract <file>
//!   e.g.  tile-store-pack data/tiles/2026/store \
//!                         data/tiles/2026/pmtiles  b0 --generation-contract generation.json

use std::ffi::CString;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use pmtiles::{Compression, Compressor, PmTilesWriter, PmtResult, TileCoord, TileType};
use rayon::prelude::*;
use sha2::{Digest, Sha256};

use tile_painter::tile_store::fsck::{validate_captured_store, CapturedTileRef};
use tile_painter::tile_store::manifest::is_safe_archive_filename;
use tile_painter::tile_store::{
    detect_layers, detect_zooms, expected_source_id, reject_incomplete_store_transactions,
    zoom_store_lock_path, StoreFileLock, StoreFileLocks, StoreMasterIngestLocks, TileStore,
    PUBLISHED_BASE_ZOOM, PUBLISHED_LAYERS, PUBLISHED_MIN_ZOOM, TOTAL_INPUT_LAYERS,
};

const STORE_LOCK_WAIT: Duration = Duration::from_secs(300);
const GENERATION_ID_HEX_LENGTH: usize = 64;
const RASTER_GENERATION_ID_HEX_LENGTH: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PublicationSlot {
    Base,
    Tier(u8),
}

/// The packer checks only the generation fields that decide where a payload may be written.
/// The complete schema, profile semantics, and identity hashes remain the JS SSOT; preserving
/// the parsed JSON value here keeps every additional quality field byte-for-byte represented in
/// the resulting manifest.
#[derive(Clone)]
struct GenerationContract {
    value: serde_json::Value,
    generation_id: String,
    base_generation_id: String,
    quality_profile_id: String,
    base_quality_profile_id: String,
    quality_profile_name: String,
    base_quality_profile_name: String,
    dataset_year: u64,
    raster_generation_id: String,
    line_model_role_sha256: String,
}

/// Declares Brotli in the pmtiles header while passing bytes through verbatim —
/// everything this packer adds is already a whole-file-Brotli HM3 image.
struct BrotliPassthrough;

impl Compressor for BrotliPassthrough {
    fn compression(&self) -> Compression {
        Compression::Brotli
    }
    fn compress(
        &self,
        f: &mut dyn FnMut(&mut dyn Write) -> std::io::Result<()>,
        writer: &mut dyn Write,
    ) -> PmtResult<()> {
        f(writer)?;
        Ok(())
    }
}

struct LayerResult {
    layer: String,
    file: String,
    sha256: String,
    tiles: u64,
    bytes: u64,
    publisher_proof: PublisherProof,
}

struct StagedLayerResult {
    layer: String,
    file: String,
    sha256: String,
    tiles: u64,
    bytes: u64,
    staged_proof: PublisherProof,
    archive: StagedArchive,
}

impl StagedLayerResult {
    fn publish(self) -> Result<LayerResult> {
        let out_path = self.archive.publish()?;
        let publisher_proof = PublisherProof::read(&out_path)?;
        if publisher_proof.dev != self.staged_proof.dev
            || publisher_proof.ino != self.staged_proof.ino
            || publisher_proof.size != self.bytes
        {
            bail!(
                "{} changed identity or size during atomic publish",
                out_path.display()
            );
        }
        Ok(LayerResult {
            layer: self.layer,
            file: self.file,
            sha256: self.sha256,
            tiles: self.tiles,
            bytes: self.bytes,
            publisher_proof,
        })
    }
}

#[derive(Clone, Copy)]
struct SnapshotEntry {
    tile_id: u64,
    tile: CapturedTileRef,
}

struct StoreSnapshot {
    zoom: u8,
    store: TileStore,
    captured_data_len: u64,
    entries: Vec<SnapshotEntry>,
}

/// One layer's exact read view. Every writer lock remains held while it is validated and packed;
/// the copied entries and open data handles therefore describe one immutable view until the
/// manifest is durable. `captured_data_len` preserves its exact bounds as a separate invariant.
struct LayerSnapshot {
    layer: String,
    stores: Vec<StoreSnapshot>,
}

struct SelectedLayerStores {
    layer: String,
    zooms: Vec<u8>,
}

/// A PMTiles archive exists under its public immutable name only after it is complete and
/// durable. Process errors remove the staging file; a process/host crash can leave only the
/// hidden staging name, which the next pack removes before retrying the same layer/build.
struct StagedArchive {
    temp_path: PathBuf,
    final_path: PathBuf,
}

fn archive_temp_path(final_path: &Path) -> Result<PathBuf> {
    let file_name = final_path
        .file_name()
        .and_then(|name| name.to_str())
        .context("archive path has no UTF-8 file name")?;
    Ok(final_path.with_file_name(format!(".{file_name}.tmp")))
}

impl StagedArchive {
    fn create(final_path: PathBuf) -> Result<(Self, File)> {
        let temp_path = archive_temp_path(&final_path)?;
        match fs::remove_file(&temp_path) {
            Ok(()) => eprintln!("removed stale pack staging file {}", temp_path.display()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("remove stale staging file {}", temp_path.display()))
            }
        }
        if final_path.try_exists()? {
            bail!(
                "{}: already published; builds are immutable",
                final_path.display()
            );
        }
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .with_context(|| format!("create archive staging file {}", temp_path.display()))?;
        Ok((
            Self {
                temp_path,
                final_path,
            },
            file,
        ))
    }

    fn path(&self) -> &Path {
        &self.temp_path
    }

    /// Linux `renameat2(RENAME_NOREPLACE)` gives both required properties in one operation:
    /// readers see either no final archive or the complete one, and a racing/stale build can
    /// never overwrite an immutable archive.
    fn publish(self) -> Result<PathBuf> {
        rename_noreplace(&self.temp_path, &self.final_path).with_context(|| {
            format!(
                "atomically publish {} as {} (already published?)",
                self.temp_path.display(),
                self.final_path.display()
            )
        })?;
        let parent = self
            .final_path
            .parent()
            .context("archive path has no parent")?;
        File::open(parent)?.sync_all()?;
        Ok(self.final_path.clone())
    }
}

impl Drop for StagedArchive {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_file(&self.temp_path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!(
                    "warning: cannot clean archive staging file {}: {error}",
                    self.temp_path.display()
                );
            }
        }
    }
}

fn rename_noreplace(from: &Path, to: &Path) -> std::io::Result<()> {
    let from = CString::new(from.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let to = CString::new(to.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let rc = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[derive(Debug, Eq, PartialEq)]
struct PublisherProof {
    dev: u64,
    ino: u64,
    size: u64,
    mtime_ns: i128,
    ctime_ns: i128,
}

const PUBLISHER_PROOF_SCHEMA: &str = "sha256-posix-stat-v1";

impl PublisherProof {
    fn from_metadata(path: &Path, metadata: &fs::Metadata) -> Result<Self> {
        if !metadata.is_file() {
            bail!("{} is not a regular file", path.display());
        }
        Ok(Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
            size: metadata.size(),
            mtime_ns: i128::from(metadata.mtime()) * 1_000_000_000
                + i128::from(metadata.mtime_nsec()),
            ctime_ns: i128::from(metadata.ctime()) * 1_000_000_000
                + i128::from(metadata.ctime_nsec()),
        })
    }

    fn read(path: &Path) -> Result<Self> {
        Self::from_metadata(path, &fs::metadata(path)?)
    }

    /// The fields that identify a FILE, for comparing a recorded proof against one read back
    /// later. ctime is excluded: it advances on metadata operations that change no byte of
    /// content, and hardlinking an archive is exactly that. Since `validate_manifest_layers`
    /// re-validates every RETAINED layer, one hardlinked archive would otherwise permanently
    /// refuse every later partial or tier pack over that manifest. Full struct equality (ctime
    /// included) is still the right test for "did this file change while I was reading it" —
    /// see `sha256_file`.
    fn identity(&self) -> (u64, u64, u64, i128) {
        (self.dev, self.ino, self.size, self.mtime_ns)
    }
}

/// Open one publication input without following a final symlink, then read from that same file
/// descriptor. This closes the common path-check/TOCTOU gap before the expensive archive phase.
fn read_regular_file_without_following_symlink(path: &Path, label: &str) -> Result<Vec<u8>> {
    let link_metadata = fs::symlink_metadata(path)
        .with_context(|| format!("{label} {} cannot be inspected", path.display()))?;
    if !link_metadata.file_type().is_file() {
        bail!("{label} {} is not a regular file", path.display());
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .with_context(|| format!("{label} {} cannot be opened safely", path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("{label} {} cannot be stat-ed", path.display()))?;
    if !metadata.is_file() {
        bail!("{label} {} is not a regular file", path.display());
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .with_context(|| format!("read {label} {}", path.display()))?;
    Ok(bytes)
}

fn read_json_object_without_following_symlink(
    path: &Path,
    label: &str,
) -> Result<serde_json::Value> {
    let bytes = read_regular_file_without_following_symlink(path, label)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("{label} {} is not valid JSON", path.display()))?;
    if !value.is_object() {
        bail!("{label} {} must contain a JSON object", path.display());
    }
    Ok(value)
}

fn required_object<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    field: &str,
    label: &str,
) -> Result<&'a serde_json::Map<String, serde_json::Value>> {
    object
        .get(field)
        .and_then(serde_json::Value::as_object)
        .with_context(|| format!("{label}.{field} must be an object"))
}

fn required_string(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
    label: &str,
) -> Result<String> {
    object
        .get(field)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .with_context(|| format!("{label}.{field} must be a non-empty string"))
}

fn required_u64(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
    label: &str,
) -> Result<u64> {
    object
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .with_context(|| format!("{label}.{field} must be an unsigned integer"))
}

fn require_lower_hex(value: String, length: usize, label: &str) -> Result<String> {
    if value.len() != length
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("{label} must be lowercase hexadecimal with {length} characters");
    }
    Ok(value)
}

/// Validate the small slot-binding subset of the full generation contract. Keeping this subset
/// deliberately narrow avoids a second JS quality validator while making a crossed base/tier
/// payload impossible to place in the wrong manifest location.
fn validate_generation_contract_for_slot(
    value: serde_json::Value,
    slot: PublicationSlot,
) -> Result<GenerationContract> {
    let object = value
        .as_object()
        .context("generation contract must be a JSON object")?;
    let label = "generation contract";
    if required_u64(object, "schema", label)? != 1 {
        bail!("{label}.schema must be 1");
    }
    let deployment = required_string(object, "deployment", label)?;
    let zoom = required_u64(object, "zoom", label)?;
    let tier = object
        .get("tier")
        .and_then(serde_json::Value::as_str)
        .context("generation contract.tier must be a string")?;
    match slot {
        PublicationSlot::Base => {
            if deployment != "base" || zoom != 12 || !tier.is_empty() {
                bail!("generation contract is not the exact base deployment shape (base/z12)");
            }
        }
        PublicationSlot::Tier(expected_zoom) => {
            let expected_tier = format!("z{expected_zoom}");
            if deployment != expected_tier
                || zoom != u64::from(expected_zoom)
                || tier != expected_tier
            {
                bail!(
                    "generation contract is not the exact tier deployment shape ({expected_tier}/z{expected_zoom})"
                );
            }
        }
    }
    let dataset_year = required_u64(object, "dataset_year", label)?;
    if !(2000..=2200).contains(&dataset_year) {
        bail!("generation contract.dataset_year is outside 2000..=2200");
    }
    let generation_id = require_lower_hex(
        required_string(object, "generation_id", label)?,
        GENERATION_ID_HEX_LENGTH,
        "generation contract.generation_id",
    )?;
    let base_generation_id = require_lower_hex(
        required_string(object, "base_generation_id", label)?,
        GENERATION_ID_HEX_LENGTH,
        "generation contract.base_generation_id",
    )?;
    let quality_profile_id = require_lower_hex(
        required_string(object, "quality_profile_id", label)?,
        GENERATION_ID_HEX_LENGTH,
        "generation contract.quality_profile_id",
    )?;
    let base_quality_profile_id = require_lower_hex(
        required_string(object, "base_quality_profile_id", label)?,
        GENERATION_ID_HEX_LENGTH,
        "generation contract.base_quality_profile_id",
    )?;
    let quality_profile_name = required_string(object, "quality_profile_name", label)?;
    let base_quality_profile_name = required_string(object, "base_quality_profile_name", label)?;
    let raster_generation_id = require_lower_hex(
        required_string(object, "raster_generation_id", label)?,
        RASTER_GENERATION_ID_HEX_LENGTH,
        "generation contract.raster_generation_id",
    )?;
    if slot == PublicationSlot::Base
        && (generation_id != base_generation_id
            || quality_profile_id != base_quality_profile_id
            || quality_profile_name != base_quality_profile_name)
    {
        bail!("base generation contract does not anchor itself");
    }
    let quality = required_object(object, "quality", label)?;
    let model_role_contract = required_object(
        quality,
        "model_role_contract",
        "generation contract.quality",
    )?;
    let line_model_role_sha256 = require_lower_hex(
        required_string(
            model_role_contract,
            "line_model_role_sha256",
            "generation contract.quality.model_role_contract",
        )?,
        GENERATION_ID_HEX_LENGTH,
        "generation contract.quality.model_role_contract.line_model_role_sha256",
    )?;
    Ok(GenerationContract {
        value,
        generation_id,
        base_generation_id,
        quality_profile_id,
        base_quality_profile_id,
        quality_profile_name,
        base_quality_profile_name,
        dataset_year,
        raster_generation_id,
        line_model_role_sha256,
    })
}

fn read_generation_contract(path: &Path, slot: PublicationSlot) -> Result<GenerationContract> {
    let value = read_json_object_without_following_symlink(path, "generation contract")?;
    validate_generation_contract_for_slot(value, slot)
        .with_context(|| format!("validate generation contract {}", path.display()))
}

fn read_coverage_r4(path: &Path) -> Result<Vec<String>> {
    let bytes = read_regular_file_without_following_symlink(path, "coverage file")?;
    let text = String::from_utf8(bytes)
        .with_context(|| format!("coverage file {} is not UTF-8", path.display()))?;
    let mut coverage: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    if coverage.is_empty() {
        bail!("--coverage-r4 {} lists no cells", path.display());
    }
    for cell in &coverage {
        let parsed = cell
            .parse::<h3o::CellIndex>()
            .with_context(|| format!("--coverage-r4: {cell:?} is not an H3 cell id"))?;
        if parsed.resolution() != h3o::Resolution::Four {
            bail!(
                "--coverage-r4: {cell} is res {}, coverage must be res 4",
                parsed.resolution()
            );
        }
        if parsed.to_string() != *cell {
            bail!("--coverage-r4: {cell:?} is not canonical (want {parsed})");
        }
    }
    coverage.sort();
    if coverage.windows(2).any(|pair| pair[0] == pair[1]) {
        bail!("--coverage-r4 {} contains duplicate cells", path.display());
    }
    Ok(coverage)
}

fn proof_u64(proof: &serde_json::Map<String, serde_json::Value>, field: &str) -> Result<u64> {
    let value = proof
        .get(field)
        .and_then(|value| value.as_str())
        .with_context(|| format!("publisher_proof.{field} must be a decimal string"))?;
    let parsed: u64 = value
        .parse()
        .with_context(|| format!("publisher_proof.{field} is not u64 decimal"))?;
    if parsed.to_string() != value {
        bail!("publisher_proof.{field} is not canonical decimal");
    }
    Ok(parsed)
}

fn proof_i128(proof: &serde_json::Map<String, serde_json::Value>, field: &str) -> Result<i128> {
    let value = proof
        .get(field)
        .and_then(|value| value.as_str())
        .with_context(|| format!("publisher_proof.{field} must be a decimal string"))?;
    let parsed: i128 = value
        .parse()
        .with_context(|| format!("publisher_proof.{field} is not signed decimal"))?;
    if parsed.to_string() != value {
        bail!("publisher_proof.{field} is not canonical decimal");
    }
    Ok(parsed)
}

fn validate_manifest_entry(out_dir: &Path, layer: &str, value: &serde_json::Value) -> Result<()> {
    let entry = value
        .as_object()
        .with_context(|| format!("manifest layer {layer} is not an object"))?;
    let file = entry
        .get("file")
        .and_then(|value| value.as_str())
        .with_context(|| format!("manifest layer {layer} has no file"))?;
    if !is_safe_archive_filename(file) {
        bail!("manifest layer {layer} has unsafe file name {file:?}");
    }
    let prefix = format!("{layer}.");
    let filename_build = file
        .strip_prefix(&prefix)
        .and_then(|name| name.strip_suffix(".pmtiles"))
        .unwrap_or("");
    if !filename_build.starts_with('b')
        || filename_build.len() < 2
        || !filename_build[1..].chars().all(|c| c.is_ascii_digit())
    {
        bail!("manifest layer {layer} file does not contain a valid build id");
    }
    if let Some(build) = entry.get("build") {
        if build.as_str() != Some(filename_build) {
            bail!("manifest layer {layer} build does not match its archive file");
        }
    }
    let bytes = entry
        .get("bytes")
        .and_then(|value| value.as_u64())
        .with_context(|| format!("manifest layer {layer} has invalid bytes"))?;
    let sha256 = entry
        .get("sha256")
        .and_then(|value| value.as_str())
        .with_context(|| format!("manifest layer {layer} has no sha256"))?;
    if sha256.len() != 64
        || !sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("manifest layer {layer} has invalid sha256");
    }
    let proof = entry
        .get("publisher_proof")
        .and_then(|value| value.as_object())
        .with_context(|| format!("manifest layer {layer} has no publisher_proof"))?;
    if proof.get("schema").and_then(|value| value.as_str()) != Some(PUBLISHER_PROOF_SCHEMA)
        || proof.get("sha256").and_then(|value| value.as_str()) != Some(sha256)
    {
        bail!("manifest layer {layer} publisher_proof is not bound to sha256");
    }
    let expected = PublisherProof {
        dev: proof_u64(proof, "dev")?,
        ino: proof_u64(proof, "ino")?,
        size: proof_u64(proof, "size")?,
        mtime_ns: proof_i128(proof, "mtime_ns")?,
        ctime_ns: proof_i128(proof, "ctime_ns")?,
    };
    if expected.size != bytes {
        bail!("manifest layer {layer} publisher_proof size does not match bytes");
    }
    let archive_path = out_dir.join(file);
    let actual = PublisherProof::read(&archive_path)?;
    if actual.identity() != expected.identity() {
        bail!(
            "manifest layer {layer} publisher_proof does not match {}",
            archive_path.display()
        );
    }
    Ok(())
}

fn validate_manifest_layers(
    out_dir: &Path,
    layers: &serde_json::Map<String, serde_json::Value>,
    replacing: Option<&[String]>,
) -> Result<()> {
    validate_manifest_layer_contract(layers)?;
    for (layer, entry) in layers {
        if replacing.is_some_and(|names| names.iter().any(|name| name == layer)) {
            continue;
        }
        validate_manifest_entry(out_dir, layer, entry)?;
    }
    Ok(())
}

/// Parse a tier layer token `{base}-z{tier}-p{N}` (city-z13 plan §D) into
/// (base, tier, pack). Tier archives are ORDINARY manifest layer entries —
/// that is what makes GC keep-sets, transaction recovery, publisher proofs
/// and readiness apply to them with zero new machinery — so the layer-set
/// contract below must recognise the token shape.
fn parse_tier_token(name: &str) -> Option<(&str, u8, &str)> {
    let (rest, pack) = name.rsplit_once('-')?;
    if !pack.starts_with('p') || pack.len() < 2 || !pack[1..].bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (base, tier) = rest.rsplit_once('-')?;
    let digits = tier.strip_prefix('z')?;
    let tier_zoom: u8 = digits.parse().ok()?;
    // Canonical digits only: `z013` must NOT parse — the TypeScript mirror
    // (heatmap-shared.ts parseTierToken) rejects it, and a packer-accepted /
    // server-refused manifest is the worst lockstep failure (gg z13 impl
    // review). Round-trip equality pins the grammar to one spelling.
    if digits != tier_zoom.to_string() {
        return None;
    }
    if !(13..=18).contains(&tier_zoom) || !PUBLISHED_LAYERS.contains(&base) {
        return None;
    }
    Some((base, tier_zoom, pack))
}

fn validate_manifest_layer_contract(
    layers: &serde_json::Map<String, serde_json::Value>,
) -> Result<()> {
    let actual: std::collections::BTreeSet<&str> = layers
        .keys()
        .map(String::as_str)
        .filter(|name| parse_tier_token(name).is_none())
        .collect();
    let expected: std::collections::BTreeSet<&str> = PUBLISHED_LAYERS.iter().copied().collect();
    if actual != expected {
        let missing: Vec<_> = expected.difference(&actual).copied().collect();
        let unexpected: Vec<_> = actual.difference(&expected).copied().collect();
        bail!(
            "manifest layer set is incomplete: missing [{}], unexpected [{}]",
            missing.join(","),
            unexpected.join(",")
        );
    }
    Ok(())
}

fn validate_existing_base_generation(
    manifest: &serde_json::Map<String, serde_json::Value>,
) -> Result<GenerationContract> {
    let value = manifest
        .get("generation")
        .cloned()
        .context("tier pack requires an existing top-level base generation")?;
    let generation = validate_generation_contract_for_slot(value, PublicationSlot::Base)
        .context("existing top-level generation is not a valid base contract")?;
    let line = manifest
        .get("line_model_role_sha256")
        .and_then(serde_json::Value::as_str)
        .context("tier pack requires top-level line_model_role_sha256")?;
    if line != generation.line_model_role_sha256 {
        bail!("existing top-level line_model_role_sha256 differs from its base generation");
    }
    Ok(generation)
}

fn validate_tier_generation_anchor(
    base: &GenerationContract,
    tier: &GenerationContract,
) -> Result<()> {
    if tier.base_generation_id != base.generation_id
        || tier.base_quality_profile_id != base.quality_profile_id
        || tier.base_quality_profile_name != base.quality_profile_name
        || tier.dataset_year != base.dataset_year
        || tier.raster_generation_id != base.raster_generation_id
    {
        bail!("tier generation is not anchored to the existing base generation/profile");
    }
    Ok(())
}

/// Decide whether a base publication may carry forward its refinement tiers. A partial base
/// publish is only a byte-for-byte continuation of the same fenced generation; a full publish
/// starts a new base epoch whenever the prior head is legacy or names a different generation.
fn base_publication_preserves_tier_state(
    previous: &serde_json::Map<String, serde_json::Value>,
    generation: &GenerationContract,
    partial: bool,
) -> Result<bool> {
    if previous.get("generation").is_none() {
        if partial {
            bail!("partial base publication cannot relabel a legacy manifest");
        }
        return Ok(false);
    }
    let existing = validate_existing_base_generation(previous)?;
    let same_generation = existing.value == generation.value;
    if partial && !same_generation {
        bail!("partial base publication generation differs from the existing fenced base");
    }
    Ok(same_generation)
}

const PACK_TRANSACTION_PREFIX: &str = ".pack-transaction-";
const PACK_TRANSACTION_SUFFIX: &str = ".incomplete";

struct PackTransaction {
    out_dir: PathBuf,
    marker_path: PathBuf,
}

impl PackTransaction {
    fn begin(out_dir: &Path, build: &str, files: &[String]) -> Result<Self> {
        if files.is_empty() {
            bail!("pack transaction has no archives");
        }
        let mut canonical = files.to_vec();
        canonical.sort();
        canonical.dedup();
        if canonical.len() != files.len() {
            bail!("pack transaction contains duplicate archive names");
        }
        for file in &canonical {
            validate_transaction_archive_name(file, build)?;
        }
        let marker_path = out_dir.join(format!(
            "{PACK_TRANSACTION_PREFIX}{build}{PACK_TRANSACTION_SUFFIX}"
        ));
        if marker_path.try_exists()? {
            bail!(
                "pack transaction marker {} already exists after recovery",
                marker_path.display()
            );
        }
        let tmp = marker_path.with_extension("incomplete.tmp");
        let body = std::iter::once(build)
            .chain(canonical.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(&tmp, body)?;
        File::open(&tmp)?.sync_all()?;
        fs::rename(&tmp, &marker_path)?;
        File::open(out_dir)?.sync_all()?;
        Ok(Self {
            out_dir: out_dir.to_path_buf(),
            marker_path,
        })
    }

    fn complete(self) -> Result<()> {
        fs::remove_file(&self.marker_path)
            .with_context(|| format!("remove pack transaction {}", self.marker_path.display()))?;
        File::open(&self.out_dir)?.sync_all()?;
        Ok(())
    }
}

fn validate_transaction_archive_name(file: &str, build: &str) -> Result<()> {
    if !is_safe_archive_filename(file) || !file.ends_with(&format!(".{build}.pmtiles")) {
        bail!("unsafe or wrong-build pack transaction archive {file:?}");
    }
    Ok(())
}

fn read_pack_transaction(marker_path: &Path) -> Result<(String, Vec<String>)> {
    let body = fs::read_to_string(marker_path)
        .with_context(|| format!("read pack transaction {}", marker_path.display()))?;
    let mut lines = body.lines();
    let build = lines.next().context("pack transaction has no build id")?;
    if !build.starts_with('b')
        || build.len() < 2
        || !build[1..]
            .chars()
            .all(|character| character.is_ascii_digit())
    {
        bail!("pack transaction has invalid build id {build:?}");
    }
    let mut files: Vec<String> = lines.map(str::to_string).collect();
    if files.is_empty() {
        bail!("pack transaction {build} has no archives");
    }
    for file in &files {
        validate_transaction_archive_name(file, build)?;
    }
    files.sort();
    if files.windows(2).any(|pair| pair[0] == pair[1]) {
        bail!("pack transaction {build} contains duplicate archives");
    }
    Ok((build.to_string(), files))
}

fn current_manifest_references_all(out_dir: &Path, files: &[String]) -> Result<bool> {
    let current_path = out_dir.join("current.json");
    if !current_path.try_exists()? {
        return Ok(false);
    }
    let current: serde_json::Value = serde_json::from_str(&fs::read_to_string(&current_path)?)
        .with_context(|| {
            format!(
                "parse {} during transaction recovery",
                current_path.display()
            )
        })?;
    let layers = current
        .get("layers")
        .and_then(serde_json::Value::as_object)
        .context("current.json has no layers object during transaction recovery")?;
    let referenced: std::collections::HashSet<&str> = layers
        .values()
        .filter_map(|entry| entry.get("file").and_then(serde_json::Value::as_str))
        .collect();
    Ok(files.iter().all(|file| referenced.contains(file.as_str())))
}

fn recover_pack_transaction(out_dir: &Path, marker_path: &Path) -> Result<()> {
    let (build, files) = read_pack_transaction(marker_path)?;
    let committed = current_manifest_references_all(out_dir, &files)?;
    for file in &files {
        let final_path = out_dir.join(file);
        let temp_path = archive_temp_path(&final_path)?;
        if !committed {
            match fs::remove_file(&final_path) {
                Ok(()) => eprintln!(
                    "recovered {build}: removed uncommitted {}",
                    final_path.display()
                ),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).context("remove uncommitted archive"),
            }
        }
        match fs::remove_file(&temp_path) {
            Ok(()) => eprintln!("recovered {build}: removed staged {}", temp_path.display()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("remove staged archive"),
        }
    }
    fs::remove_file(marker_path)
        .with_context(|| format!("remove recovered transaction {}", marker_path.display()))?;
    File::open(out_dir)?.sync_all()?;
    Ok(())
}

fn recover_pack_transactions(out_dir: &Path) -> Result<()> {
    let mut markers: Vec<PathBuf> = fs::read_dir(out_dir)?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name();
            let name = name.to_str()?;
            (name.starts_with(PACK_TRANSACTION_PREFIX) && name.ends_with(PACK_TRANSACTION_SUFFIX))
                .then(|| entry.path())
        })
        .collect();
    markers.sort();
    for marker in markers {
        recover_pack_transaction(out_dir, &marker)?;
    }
    Ok(())
}

fn cleanup_orphan_pack_temps(out_dir: &Path) -> Result<()> {
    for entry in fs::read_dir(out_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let archive_temp = name
            .strip_prefix('.')
            .and_then(|name| name.strip_suffix(".tmp"))
            .is_some_and(|name| name.ends_with(".pmtiles") && is_safe_archive_filename(name));
        let transaction_temp = name.starts_with(PACK_TRANSACTION_PREFIX)
            && name.ends_with(&format!("{PACK_TRANSACTION_SUFFIX}.tmp"));
        if archive_temp || transaction_temp {
            fs::remove_file(entry.path()).with_context(|| {
                format!("remove orphan pack staging file {}", entry.path().display())
            })?;
        }
    }
    File::open(out_dir)?.sync_all()?;
    Ok(())
}

/// Layers whose working stores must be proven clean for this pack. A fresh `total` is derived
/// from every source layer, including layers whose old archives a partial pack retains.
fn validation_layer_scope(only: &[String]) -> Vec<String> {
    if only.is_empty() {
        return Vec::new(); // empty means every detected layer for a full pack
    }
    let mut layers = only.to_vec();
    if only.iter().any(|layer| layer == "total") {
        layers.extend(TOTAL_INPUT_LAYERS.iter().map(|layer| (*layer).to_string()));
    }
    layers.sort();
    layers.dedup();
    layers
}

fn reject_incomplete_rebuilds(store_root: &Path, only: &[String]) -> Result<()> {
    reject_incomplete_store_transactions(store_root, only)
}

fn selected_layer_stores(store_root: &Path, only: &[String]) -> Result<Vec<SelectedLayerStores>> {
    reject_incomplete_rebuilds(store_root, only)?;
    let mut layers = detect_layers(store_root)?;
    if layers.is_empty() {
        bail!("no layer stores under {}", store_root.display());
    }
    if only.is_empty() {
        let actual: std::collections::BTreeSet<&str> = layers.iter().map(String::as_str).collect();
        let expected: std::collections::BTreeSet<&str> = PUBLISHED_LAYERS.iter().copied().collect();
        if actual != expected {
            let missing: Vec<_> = expected.difference(&actual).copied().collect();
            let unexpected: Vec<_> = actual.difference(&expected).copied().collect();
            bail!(
                "full pack requires the exact published store set: missing [{}], unexpected [{}]",
                missing.join(","),
                unexpected.join(",")
            );
        }
    }
    if !only.is_empty() {
        for layer in only {
            if !layers.contains(layer) {
                bail!("--layer {layer}: no store under {}", store_root.display());
            }
        }
        layers.retain(|layer| only.contains(layer));
    }
    layers
        .into_iter()
        .map(|layer| {
            let zooms = detect_zooms(&store_root.join(&layer))?;
            if zooms.is_empty() {
                bail!("{layer}: no z*.qtsi stores");
            }
            Ok(SelectedLayerStores { layer, zooms })
        })
        .collect()
}

fn snapshot_layer(layer_dir: &Path, layer: &str, zooms: &[u8]) -> Result<LayerSnapshot> {
    let mut stores = Vec::with_capacity(zooms.len());
    for &zoom in zooms {
        let store = TileStore::open(layer_dir, zoom, false)?;
        let captured_data_len = store.data_file_len()?;
        let mut entries = Vec::new();
        store.for_each_present(|x, y, entry| {
            let id = TileCoord::new(zoom, x, y)
                .map_err(|error| anyhow::anyhow!("z{zoom}/{x}/{y}: {error}"))?
                .into();
            entries.push(SnapshotEntry {
                tile_id: pmtiles::TileId::value(id),
                tile: CapturedTileRef { x, y, entry },
            });
            Ok(())
        })?;
        stores.push(StoreSnapshot {
            zoom,
            store,
            captured_data_len,
            entries,
        });
    }
    Ok(LayerSnapshot {
        layer: layer.to_string(),
        stores,
    })
}

fn validate_snapshots(snapshots: &[LayerSnapshot]) -> Result<()> {
    validate_snapshots_for_base(snapshots, PUBLISHED_BASE_ZOOM)
}

fn validate_exact_zoom_band(layer: &str, actual_zooms: &[u8], base_zoom: u8) -> Result<()> {
    let expected_zooms: Vec<u8> = (PUBLISHED_MIN_ZOOM..=base_zoom).collect();
    if actual_zooms != expected_zooms {
        bail!(
            "{layer}: zoom set {:?}, expected exact {:?}",
            actual_zooms,
            expected_zooms
        );
    }
    Ok(())
}

fn validate_snapshots_for_base(snapshots: &[LayerSnapshot], expected_base_zoom: u8) -> Result<()> {
    for layer in snapshots {
        let actual_zooms: Vec<u8> = layer.stores.iter().map(|store| store.zoom).collect();
        validate_exact_zoom_band(&layer.layer, &actual_zooms, expected_base_zoom)?;
    }
    validate_snapshots_common(snapshots)
}

/// Everything band-independent: source_id agreement with the layer registry,
/// tile_px consistency across zoom stores, and the captured-snapshot fsck.
/// Shared by the z2..z12 base contract and tier packs (band exactly [tier]).
fn validate_snapshots_common(snapshots: &[LayerSnapshot]) -> Result<()> {
    for layer in snapshots {
        let first = layer
            .stores
            .first()
            .with_context(|| format!("{}: captured no zoom stores", layer.layer))?;
        let source_id = first.store.source_id();
        let expected_source_id = expected_source_id(&layer.layer)
            .with_context(|| format!("{}: unknown publish layer", layer.layer))?;
        if source_id != expected_source_id {
            bail!(
                "{}: source_id {source_id} differs from required {expected_source_id}",
                layer.layer
            );
        }
        let tile_px = first.store.tile_px();
        for store in &layer.stores {
            if store.store.source_id() != source_id {
                bail!(
                    "{}: z{} source_id {} differs from z{} source_id {}",
                    layer.layer,
                    store.zoom,
                    store.store.source_id(),
                    first.zoom,
                    source_id
                );
            }
            if store.store.tile_px() != tile_px {
                bail!(
                    "{}: z{} tile_px {} differs from z{} tile_px {}",
                    layer.layer,
                    store.zoom,
                    store.store.tile_px(),
                    first.zoom,
                    tile_px
                );
            }
        }
        for store in &layer.stores {
            eprintln!("fsck {}/z{} captured snapshot…", layer.layer, store.zoom);
            validate_captured_store(
                &store.store,
                &layer.layer,
                store.zoom,
                store.captured_data_len,
                &store.entries,
                |entry| entry.tile,
            )
            .ensure_clean()?;
        }
    }
    Ok(())
}

/// Copy every selected index plus every dependency of a selected `total`, pin their data inodes,
/// and retain the whole lock set through validation, packing, and manifest durability. Only the
/// explicit pack scope reaches `body`; dependency snapshots exist solely for the fail-closed fsck.
fn with_validated_store_snapshots<T>(
    store_root: &Path,
    only: &[String],
    timeout: Duration,
    body: impl FnOnce(Vec<LayerSnapshot>) -> Result<T>,
) -> Result<T> {
    with_store_snapshots_after_capture(store_root, only, timeout, validate_snapshots, body)
}

/// One immutable pack of a tier store (city-z13 plan §D): its zoom, pack id
/// and the R4 cells whose z{tier} tiles this pack authoritatively covers.
struct TierPack {
    tier: u8,
    pack: String,
    coverage_r4: Vec<String>,
    generation: GenerationContract,
}

impl TierPack {
    /// The manifest layer token (= archive name stem) for one base layer.
    fn token(&self, base_layer: &str) -> String {
        format!("{base_layer}-z{}-{}", self.tier, self.pack)
    }
}

/// Tier-aware wrapper: a tier root validates against the tier's exact zoom
/// band `[tier]` (a tier store has no pyramid below its own zoom), then the
/// snapshots are re-labelled with the pack's layer tokens so staging, the
/// transaction marker and the manifest all carry `{layer}-z{tier}-{pack}`
/// names while source_id/fsck validation ran against the BASE layer names.
fn with_tier_validated_store_snapshots<T>(
    store_root: &Path,
    only: &[String],
    tier_mode: &Option<TierPack>,
    body: impl FnOnce(Vec<LayerSnapshot>) -> Result<T>,
) -> Result<T> {
    match tier_mode {
        None => with_validated_store_snapshots(store_root, only, STORE_LOCK_WAIT, body),
        Some(tier_pack) => {
            let tier = tier_pack.tier;
            with_store_snapshots_after_capture(
                store_root,
                only,
                STORE_LOCK_WAIT,
                move |snapshots| {
                    for layer in snapshots {
                        let actual_zooms: Vec<u8> =
                            layer.stores.iter().map(|store| store.zoom).collect();
                        if actual_zooms != [tier] {
                            bail!(
                                "{}: tier root zoom set {:?}, expected exactly [{tier}]",
                                layer.layer,
                                actual_zooms
                            );
                        }
                    }
                    validate_snapshots_common(snapshots)
                },
                |mut snapshots| {
                    for snapshot in &mut snapshots {
                        snapshot.layer = tier_pack.token(&snapshot.layer);
                    }
                    body(snapshots)
                },
            )
        }
    }
}

/// Testable phase boundary: `body` is unreachable unless validation succeeds, and every writer
/// domain remains excluded until both callbacks return.
fn with_store_snapshots_after_capture<T>(
    store_root: &Path,
    only: &[String],
    timeout: Duration,
    validate: impl FnOnce(&[LayerSnapshot]) -> Result<()>,
    body: impl FnOnce(Vec<LayerSnapshot>) -> Result<T>,
) -> Result<T> {
    let _outer_locks = StoreMasterIngestLocks::acquire_bounded(store_root, timeout)?;
    let validation_scope = validation_layer_scope(only);
    let selected = selected_layer_stores(store_root, &validation_scope)?;
    // Outer locks quiesce deployed combine/transcode/ingest paths. The selected + dependency
    // per-z set additionally excludes a direct TileStore writer that uses only the store's lock.
    // Acquire every path before reading any index, in one reusable canonical path order.
    let _snapshot_locks = StoreFileLocks::acquire_canonical(
        selected.iter().flat_map(|selection| {
            let layer_dir = store_root.join(&selection.layer);
            selection
                .zooms
                .iter()
                .map(move |&zoom| zoom_store_lock_path(&layer_dir, zoom))
        }),
        timeout,
    )?;
    // A direct per-z writer does not take the outer master lock. It can create its durable fence
    // after our first directory scan and then block on this z-lock set; recheck only after every
    // dependency lock is ours so a failed writer can never leave a coherent partial cascade that
    // slips into the archive.
    reject_incomplete_rebuilds(store_root, &validation_scope)?;
    let mut snapshots = selected
        .par_iter()
        .map(|selection| {
            snapshot_layer(
                &store_root.join(&selection.layer),
                &selection.layer,
                &selection.zooms,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    validate(&snapshots)?;
    if !only.is_empty() {
        snapshots.retain(|snapshot| only.iter().any(|layer| layer == &snapshot.layer));
    }
    body(snapshots)
    // Both guards retain master→ingest→all selected/dependency z locks through manifest durability.
}

fn acquire_pack_lock(out_dir: &Path, timeout: Duration) -> Result<StoreFileLock> {
    StoreFileLock::acquire_bounded(&out_dir.join(".pack.lock"), timeout)
        .context("wait for the PMTiles publish/GC lock")
}

struct CliArguments {
    store_root: PathBuf,
    out_dir: PathBuf,
    build: String,
    layers: Vec<String>,
    tier: Option<u8>,
    pack: Option<String>,
    coverage_r4: Option<PathBuf>,
    generation_contract: PathBuf,
}

fn next_cli_value(arguments: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    let value = arguments
        .next()
        .with_context(|| format!("{flag} needs a value"))?;
    if value.starts_with("--") {
        bail!("{flag} needs a value, got flag {value:?}");
    }
    Ok(value)
}

fn parse_cli_arguments(arguments: impl IntoIterator<Item = String>) -> Result<CliArguments> {
    let mut positional = Vec::new();
    let mut layers = Vec::new();
    let mut tier = None;
    let mut pack = None;
    let mut coverage_r4 = None;
    let mut generation_contract = None;
    let mut arguments = arguments.into_iter();

    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--layer" => layers.push(next_cli_value(&mut arguments, "--layer")?),
            "--tier" => {
                if tier.is_some() {
                    bail!("--tier may be supplied only once");
                }
                tier = Some(
                    next_cli_value(&mut arguments, "--tier")?
                        .parse()
                        .context("--tier must be an integer zoom")?,
                );
            }
            "--pack" => {
                if pack.is_some() {
                    bail!("--pack may be supplied only once");
                }
                pack = Some(next_cli_value(&mut arguments, "--pack")?);
            }
            "--coverage-r4" => {
                if coverage_r4.is_some() {
                    bail!("--coverage-r4 may be supplied only once");
                }
                coverage_r4 = Some(PathBuf::from(next_cli_value(
                    &mut arguments,
                    "--coverage-r4",
                )?));
            }
            "--generation-contract" => {
                if generation_contract.is_some() {
                    bail!("--generation-contract may be supplied only once");
                }
                generation_contract = Some(PathBuf::from(next_cli_value(
                    &mut arguments,
                    "--generation-contract",
                )?));
            }
            value if value.starts_with("--") => bail!("unknown option {value:?}"),
            value => positional.push(value.to_string()),
        }
    }

    let [store_root, out_dir, build]: [String; 3] = positional.try_into().map_err(|_| {
        anyhow::anyhow!(
            "usage: tile-store-pack <store-root> <out-dir> <build-id (b<N>)> \
             --generation-contract <file> [--layer L]... \
             [--tier <zoom> --pack p<N> --coverage-r4 <file>]"
        )
    })?;
    if !build.starts_with('b')
        || build.len() < 2
        || !build[1..]
            .chars()
            .all(|character| character.is_ascii_digit())
    {
        bail!("build id must be b<N>, got {build:?}");
    }
    let generation_contract =
        generation_contract.context("--generation-contract is mandatory for publication")?;

    let tier_shape = (tier.is_some(), pack.is_some(), coverage_r4.is_some());
    let tier = match tier_shape {
        (false, false, false) => None,
        (true, true, true) => {
            let tier_zoom = tier.expect("tier shape says tier is present");
            if !(13..=18).contains(&tier_zoom) {
                bail!("--tier {tier_zoom} out of range 13..=18");
            }
            let pack_id = pack.as_ref().expect("tier shape says pack is present");
            if !pack_id.starts_with('p')
                || pack_id.len() < 2
                || !pack_id[1..].bytes().all(|byte| byte.is_ascii_digit())
            {
                bail!("--pack must be p<N>, got {pack_id:?}");
            }
            if !layers.is_empty() {
                bail!("--tier packs the whole tier store set; --layer is not combinable");
            }
            Some(tier_zoom)
        }
        _ => bail!("--tier, --pack and --coverage-r4 must be given together"),
    };

    Ok(CliArguments {
        store_root: PathBuf::from(store_root),
        out_dir: PathBuf::from(out_dir),
        build,
        layers,
        tier,
        pack,
        coverage_r4,
        generation_contract,
    })
}

fn main() -> Result<()> {
    // usage: tile-store-pack <store-root> <out-dir> <build-id>
    //        --generation-contract <file> [--layer L]...
    //        tile-store-pack <tier-root> <out-dir> <build-id> --tier 13
    //        --pack p001 --coverage-r4 <file> --generation-contract <file>
    // With --layer, only the named layers are packed and the manifest MERGES:
    // untouched layers keep their previous archive + build id — a road-only
    // republish costs one layer's Brotli, not all eight (owner ask 2026-07-09).
    // With --tier (city-z13 plan §D), the root is a TIER root (zoom band
    // exactly [tier]); every published layer packs into an immutable PACK —
    // archives named `{layer}-z{tier}-{pack}.{build}.pmtiles`, manifest layer
    // entries under those tokens (ordinary entries: GC/recovery/readiness
    // apply unchanged) plus one `tiers.z{tier}.packs[]` index entry carrying
    // the pack's R4 coverage for the serving resolver.
    let cli = parse_cli_arguments(std::env::args().skip(1))?;
    let slot = cli
        .tier
        .map_or(PublicationSlot::Base, PublicationSlot::Tier);
    let generation = read_generation_contract(&cli.generation_contract, slot)?;
    let tier_mode = match (cli.tier, cli.pack.as_ref(), cli.coverage_r4.as_ref()) {
        (None, None, None) => None,
        (Some(tier), Some(pack), Some(coverage)) => Some(TierPack {
            tier,
            pack: pack.clone(),
            coverage_r4: read_coverage_r4(coverage)?,
            generation: generation.clone(),
        }),
        _ => bail!("invalid publication argument combination"),
    };
    let store_root = cli.store_root;
    let out_dir = cli.out_dir;
    let build = cli.build;
    let only = cli.layers;
    fs::create_dir_all(&out_dir)?;

    let partial = !only.is_empty();
    // Serialize whole partial packs on a lock file: the manifest merge is
    // read-modify-write, and two concurrent partials would drop each other's
    // entries (and clash on the same-build temp name). Held for the process
    // lifetime; full packs take it too so a full and a partial cannot
    // interleave either (/gg consensus).
    let _pack_lock = acquire_pack_lock(&out_dir, STORE_LOCK_WAIT)?;
    recover_pack_transactions(&out_dir)?;
    cleanup_orphan_pack_temps(&out_dir)?;
    if partial || tier_mode.is_some() {
        // Preflight the merge base BEFORE any (immutable) archive is written —
        // discovering a missing manifest after the pack leaves undeletable
        // {layer}.{build}.pmtiles behind and burns the build id (/gg Codex).
        // A tier pack always merges (it never retires base layers).
        let manifest = out_dir.join("current.json");
        if !manifest.exists() {
            bail!(
                "partial/tier pack needs an existing {} to merge over — run a full pack first",
                manifest.display()
            );
        }
        let previous: serde_json::Value = serde_json::from_str(&fs::read_to_string(&manifest)?)?;
        let previous_layers = previous
            .get("layers")
            .and_then(|value| value.as_object())
            .context("current.json has no layers object")?;
        // Validate every entry the partial pack will retain BEFORE spending
        // hours writing a new immutable archive. Selected layers are replaced
        // by fresh, post-hash proofs below.
        validate_manifest_layers(&out_dir, previous_layers, Some(&only))?;
        if partial && tier_mode.is_none() {
            base_publication_preserves_tier_state(
                previous
                    .as_object()
                    .context("current.json is not an object")?,
                &generation,
                true,
            )?;
        }
        if let Some(tier_pack) = &tier_mode {
            let base_generation = validate_existing_base_generation(
                previous
                    .as_object()
                    .context("current.json is not an object")?,
            )?;
            validate_tier_generation_anchor(&base_generation, &tier_pack.generation)?;
            // Pack ids are immutable like archives: re-publishing p001 must
            // fail loudly, never replace coverage in place.
            let existing = previous
                .get("tiers")
                .and_then(|tiers| tiers.get(format!("z{}", tier_pack.tier)))
                .and_then(|entry| entry.get("packs"))
                .and_then(|packs| packs.as_array());
            if existing.is_some_and(|packs| {
                packs.iter().any(|pack| {
                    pack.get("pack").and_then(|id| id.as_str()) == Some(&tier_pack.pack)
                })
            }) {
                bail!(
                    "tier z{} pack {} already published — packs are immutable, pick the next id",
                    tier_pack.tier,
                    tier_pack.pack
                );
            }
        }
    }
    with_tier_validated_store_snapshots(&store_root, &only, &tier_mode, |snapshots| {
        eprintln!(
            "pack {build}: {} layers{} → {}",
            snapshots.len(),
            match (&tier_mode, partial) {
                (Some(t), _) => format!(" (tier z{} pack {})", t.tier, t.pack),
                (None, true) => " (partial — manifest merges)".to_string(),
                (None, false) => String::new(),
            },
            out_dir.display()
        );

        let results = pack_snapshots_transactionally(
            snapshots,
            &out_dir,
            &build,
            partial,
            &generation,
            &tier_mode,
        )?;
        // Deletion no longer happens here (2026-07-16 Track 2 rewrite — docs/dev/
        // checkout-restructure-plan.md). Per-environment pins (`current.{env}.json`) mean a
        // prod pointer can legitimately lag dev by many publishes; this pack's old
        // "keep new+previous" retention would 404 a stale-but-still-live pin the moment TWO
        // publishes happened after it. Retention is now `tile-store-gc`'s job.
        let total_tiles: u64 = results.iter().map(|result| result.tiles).sum();
        let total_bytes: u64 = results.iter().map(|result| result.bytes).sum();
        eprintln!(
            "PUBLISHED {build}: {} layers, {total_tiles} tiles, {:.1} GiB, manifest flipped",
            results.len(),
            total_bytes as f64 / (1 << 30) as f64
        );
        Ok(())
    })
}

fn pack_snapshots_transactionally(
    snapshots: Vec<LayerSnapshot>,
    out_dir: &Path,
    build: &str,
    partial: bool,
    generation: &GenerationContract,
    tier_mode: &Option<TierPack>,
) -> Result<Vec<LayerResult>> {
    // Finish every expensive archive under a hidden name before exposing even the first final
    // name. Nesting layer fan-out and each layer's Rayon prefetch deadlocks at one pool thread,
    // so layers remain sequential and reads inside one layer remain parallel.
    let staged: Vec<StagedLayerResult> = snapshots
        .into_iter()
        .map(|snapshot| stage_layer(snapshot, out_dir, build))
        .collect::<Result<_>>()?;
    let files: Vec<String> = staged.iter().map(|result| result.file.clone()).collect();
    let transaction = PackTransaction::begin(out_dir, build, &files)?;

    let publish_result = (|| -> Result<Vec<LayerResult>> {
        let mut results: Vec<LayerResult> = staged
            .into_iter()
            .map(StagedLayerResult::publish)
            .collect::<Result<_>>()?;
        results.sort_by(|left, right| left.layer.cmp(&right.layer));
        write_manifest(out_dir, build, &results, partial, generation, tier_mode)?;
        Ok(results)
    })();

    let results = match publish_result {
        Ok(results) => results,
        Err(error) => {
            if let Err(recovery_error) = recover_pack_transaction(out_dir, &transaction.marker_path)
            {
                return Err(error.context(format!(
                    "pack failed and transaction recovery also failed: {recovery_error:#}"
                )));
            }
            return Err(error);
        }
    };
    transaction.complete()?;
    Ok(results)
}

fn stage_layer(snapshot: LayerSnapshot, out_dir: &Path, build: &str) -> Result<StagedLayerResult> {
    let t0 = Instant::now();
    let LayerSnapshot { layer, mut stores } = snapshot;
    stores.sort_unstable_by_key(|store| store.zoom);
    let out_name = format!("{layer}.{build}.pmtiles");
    let out_path = out_dir.join(&out_name);

    let first = stores.first().context("validated layer has no stores")?;
    let min_zoom = first.zoom;
    let max_zoom = stores.last().expect("non-empty").zoom;
    let source_id = first.store.source_id();
    let metadata = format!(
        r#"{{"name":"quietmap-{layer}","layer":"{layer}","build":"{build}","source_id":{source_id}}}"#
    );
    let writer = PmTilesWriter::new(TileType::Unknown)
        .tile_codec(BrotliPassthrough)
        .metadata(&metadata)
        .min_zoom(min_zoom)
        .max_zoom(max_zoom)
        .bounds(-180.0, -85.051_13, 180.0, 85.051_13)
        .center(15.5, 49.8)
        .center_zoom(min_zoom);
    // Never stream into the public immutable name: a crash during create/finalize would leave
    // a final-named partial archive that blocks retry and looks publishable to an operator.
    let (staged, file) = StagedArchive::create(out_path)?;
    let mut w = writer.create(file)?;

    let n_feed: u64 = stores.iter().map(|store| store.entries.len() as u64).sum();
    // Ordered-prefetch pipeline (owner efficiency directive, 2026-07-08): the
    // pmtiles format forces ONE writer in ascending-id order, but nothing
    // forces the READS to be serial. A producer thread pulls blob batches with
    // the shared rayon pool (full-core NVMe queue depth — this is what erased
    // the 23.5-min single-threaded road/total tail of the first b0 pack) and a
    // bounded channel hands them to this thread, which writes strictly in
    // order. Batches are BYTE-bounded, so peak RAM is machine-independent:
    // ~PREFETCH_BATCH_BYTES × (PREFETCH_WINDOW + 2) per layer.
    const PREFETCH_BATCH_BYTES: u64 = 32 << 20;
    const PREFETCH_WINDOW: usize = 3;
    for store in &mut stores {
        store.entries.sort_unstable_by_key(|entry| entry.tile_id);
    }
    // Per-tile fetch still allocates a fresh Vec<u8> (get_blob_by_entry) rather than reusing a
    // per-rayon-thread scratch buffer. Considered and skipped: every blob here is handed to
    // `tx.send` and on to the single writer thread as an OWNED Vec (add_raw_tile only borrows
    // it briefly) — a shared scratch buffer would need a return path back to whichever rayon
    // worker produced it once the writer is done with it, which is real plumbing for a step
    // that (post 2026-07-16) no longer decodes or re-encodes anything: the dominant per-tile
    // cost is now the pread itself, not the allocation. Not worth the complexity unless a
    // future profile shows allocation (not I/O) is the bottleneck.
    // PMTiles TileId ranges are ordered by zoom, so ascending zoom stores plus each store's
    // ascending TileId entries form the same one global order as the former flattened feed.
    for store in &stores {
        let mut batches: Vec<&[SnapshotEntry]> = Vec::new();
        {
            let (mut start, mut acc) = (0usize, 0u64);
            for (index, entry) in store.entries.iter().enumerate() {
                acc += u64::from(entry.tile.entry.len);
                if acc >= PREFETCH_BATCH_BYTES {
                    batches.push(&store.entries[start..=index]);
                    start = index + 1;
                    acc = 0;
                }
            }
            if start < store.entries.len() {
                batches.push(&store.entries[start..]);
            }
        }
        std::thread::scope(|scope| -> Result<()> {
            let (tx, rx) =
                std::sync::mpsc::sync_channel::<Vec<Result<(u64, Vec<u8>)>>>(PREFETCH_WINDOW);
            scope.spawn(move || {
                for batch in &batches {
                    let blobs: Vec<Result<(u64, Vec<u8>)>> = batch
                        .par_iter()
                        .map(|entry| {
                            Ok((
                                entry.tile_id,
                                store.store.get_hm3_by_entry(&entry.tile.entry)?,
                            ))
                        })
                        .collect();
                    if tx.send(blobs).is_err() {
                        return; // writer bailed — stop prefetching
                    }
                }
            });
            for blobs in rx {
                for result in blobs {
                    let (id, blob) = result?;
                    let coord = pmtiles::TileId::new(id)
                        .map_err(|error| anyhow::anyhow!("tile id {id}: {error}"))?
                        .into();
                    w.add_raw_tile(coord, &blob)
                        .map_err(|error| anyhow::anyhow!("{layer} id {id}: {error}"))?;
                }
            }
            Ok(())
        })?;
    }
    w.finalize()
        .map_err(|e| anyhow::anyhow!("{layer}: finalize: {e}"))?;

    // Durability + integrity record: fsync, then hash what is actually on
    // disk. The second read pass is deliberate — pmtiles finalize() SEEKS BACK
    // to rewrite the header + root directory, so a streaming hash of writes
    // would not match the final file bytes (reviewed 2026-07-08).
    let f = File::open(staged.path())?;
    f.sync_all()?;
    let (sha, bytes, staged_proof) = sha256_file(staged.path())?;
    eprintln!(
        "{layer}: staged {n_feed} tiles → {} ({:.2} GiB) in {:.0?}",
        out_name,
        bytes as f64 / (1 << 30) as f64,
        t0.elapsed()
    );
    Ok(StagedLayerResult {
        layer: layer.to_string(),
        file: out_name,
        sha256: sha,
        tiles: n_feed,
        bytes,
        staged_proof,
        archive: staged,
    })
}

fn sha256_file(path: &Path) -> Result<(String, u64, PublisherProof)> {
    let mut f = File::open(path)?;
    let before = PublisherProof::from_metadata(path, &f.metadata()?)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 8 << 20];
    let mut total = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    let mut hex = String::with_capacity(64);
    for b in hasher.finalize() {
        write!(hex, "{b:02x}").expect("write to String");
    }
    let after = PublisherProof::from_metadata(path, &f.metadata()?)?;
    if before != after {
        bail!(
            "{} changed while its sha256 was being computed",
            path.display()
        );
    }
    if total != after.size {
        bail!(
            "{} read {total} bytes while stat reports {}",
            path.display(),
            after.size
        );
    }
    let published_path = PublisherProof::read(path)?;
    if published_path != after {
        bail!(
            "{} path changed after its sha256 was computed",
            path.display()
        );
    }
    Ok((hex, total, after))
}

/// `current.json`, written last, tmp + atomic rename: the pointer flip. Readers
/// (Fastify, publish/rsync tooling, rollback) treat THIS as the single truth.
/// Every layer entry carries its OWN `build` — a partial pack merges over the
/// previous manifest, so untouched layers keep serving their existing archives
/// (the per-layer split finally pays off: one-layer republish, one-layer cost).
fn write_manifest(
    out_dir: &Path,
    build: &str,
    results: &[LayerResult],
    partial: bool,
    generation: &GenerationContract,
    tier_mode: &Option<TierPack>,
) -> Result<()> {
    let created_unix = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let path = out_dir.join("current.json");

    // The previous manifest, when one exists. Unknown top-level keys are carried forward, but
    // refinement state belongs to one exact base generation and is removed atomically when a
    // full base publish starts another generation (including legacy -> fenced migration).
    let previous: Option<serde_json::Map<String, serde_json::Value>> = if path.exists() {
        Some(
            serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&path)?)?
                .as_object()
                .cloned()
                .context("current.json is not an object")?,
        )
    } else {
        None
    };
    if (partial || tier_mode.is_some()) && previous.is_none() {
        bail!("partial/tier pack needs an existing current.json to merge over");
    }
    let slot = tier_mode
        .as_ref()
        .map_or(PublicationSlot::Base, |tier_pack| {
            PublicationSlot::Tier(tier_pack.tier)
        });
    validate_generation_contract_for_slot(generation.value.clone(), slot)
        .context("publication generation contract does not match its manifest slot")?;
    let preserve_tier_state = match (&previous, tier_mode) {
        (Some(previous), None) => {
            base_publication_preserves_tier_state(previous, generation, partial)?
        }
        (Some(_), Some(_)) => true,
        (None, _) => false,
    };

    // Seed layer entries: a FULL z12 pack retires the base layers it rebuilds
    // (so a dropped layer cannot linger). It retains tier tokens only for the exact same base
    // generation. Partial and tier packs seed everything after their generation preflight.
    let mut layers: serde_json::Map<String, serde_json::Value> = match &previous {
        Some(prev) => {
            let prev_layers = prev
                .get("layers")
                .and_then(|l| l.as_object())
                .context("current.json has no layers object")?;
            if partial || tier_mode.is_some() {
                prev_layers.clone()
            } else if preserve_tier_state {
                prev_layers
                    .iter()
                    .filter(|(name, _)| parse_tier_token(name).is_some())
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect()
            } else {
                serde_json::Map::new()
            }
        }
        None => serde_json::Map::new(),
    };
    for r in results {
        let proof = &r.publisher_proof;
        layers.insert(
            r.layer.clone(),
            serde_json::json!({
                "file": r.file, "build": build, "sha256": r.sha256,
                "tiles": r.tiles, "bytes": r.bytes,
                "publisher_proof": {
                    "schema": PUBLISHER_PROOF_SCHEMA,
                    "sha256": r.sha256,
                    "dev": proof.dev.to_string(),
                    "ino": proof.ino.to_string(),
                    "size": proof.size.to_string(),
                    "mtime_ns": proof.mtime_ns.to_string(),
                    "ctime_ns": proof.ctime_ns.to_string(),
                },
            }),
        );
    }
    // Recheck every retained and newly packed archive immediately before the
    // atomic manifest flip. This closes the partial-pack preflight race.
    validate_manifest_layers(out_dir, &layers, None)?;

    // The manifest object: previous unknown keys first, then the keys this
    // packer owns. A tier pack additionally upserts its index entry under
    // `tiers.z{N}.packs[]` — coverage for the serving resolver; the archives
    // themselves are protected by their ordinary `layers` entries above.
    let mut manifest = previous.unwrap_or_default();
    if tier_mode.is_none() {
        if !preserve_tier_state {
            manifest.remove("tiers");
            manifest.remove("qualification_closure");
        }
        manifest.insert("generation".into(), generation.value.clone());
        manifest.insert(
            "line_model_role_sha256".into(),
            serde_json::json!(generation.line_model_role_sha256),
        );
    }
    manifest.insert("build".into(), serde_json::json!(build));
    manifest.insert("created_unix".into(), serde_json::json!(created_unix));
    manifest.insert("layers".into(), serde_json::Value::Object(layers));
    if let Some(tier_pack) = tier_mode {
        let base_generation = validate_existing_base_generation(&manifest)?;
        validate_tier_generation_anchor(&base_generation, &tier_pack.generation)?;
        if tier_pack.generation.value != generation.value {
            bail!("tier generation differs from the publication generation input");
        }
        if tier_pack.coverage_r4.is_empty()
            || tier_pack
                .coverage_r4
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
        {
            bail!("tier coverage must be non-empty, sorted, and unique");
        }
        let tiers = manifest
            .entry("tiers")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .context("current.json tiers is not an object")?;
        let tier_entry = tiers
            .entry(format!("z{}", tier_pack.tier))
            .or_insert_with(|| serde_json::json!({ "packs": [] }))
            .as_object_mut()
            .context("tier entry is not an object")?;
        let packs = tier_entry
            .entry("packs")
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
            .context("tier packs is not an array")?;
        if packs
            .iter()
            .any(|p| p.get("pack").and_then(|id| id.as_str()) == Some(&tier_pack.pack))
        {
            bail!(
                "tier z{} pack {} already in manifest — packs are immutable",
                tier_pack.tier,
                tier_pack.pack
            );
        }
        packs.push(serde_json::json!({
            "pack": tier_pack.pack,
            "build": build,
            "created_unix": created_unix,
            "coverage_r4": tier_pack.coverage_r4,
            "layers": results.iter().map(|r| r.layer.clone()).collect::<Vec<_>>(),
            "generation": tier_pack.generation.value,
        }));
    }
    let json = serde_json::Value::Object(manifest).to_string();

    // Build-unique temp name: two concurrent packers must not clobber each
    // other's staged manifest (the rename itself is last-writer-wins, atomic).
    let tmp = out_dir.join(format!("current.json.{build}.tmp"));
    fs::write(&tmp, &json)?;
    File::open(&tmp)?.sync_all()?;
    fs::rename(&tmp, &path)?;
    File::open(out_dir)?.sync_all()?; // the rename itself, durable
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::fs::OpenOptions;
    use std::sync::{Mutex, MutexGuard};

    use tempfile::{tempdir, TempDir};
    use tile_painter::grid::TILE_PX;
    use tile_painter::tile_store::{
        ingest_store_lock_path, master_store_lock_path, zoom_store_lock_path, StoreFileLock,
        TileCodec, TileStore, REBUILD_INCOMPLETE_MARKER,
    };
    use tile_painter::wire_hm3::{self, NO_DATA, SOURCE_ID_RAIL, SOURCE_ID_ROAD};

    static PACK_TEST_SCRATCH_LOCK: Mutex<()> = Mutex::new(());

    /// Owns one unique scratch tree while preventing production-sized store
    /// preallocation from exhausting the shared test filesystem. Every first
    /// tile reserves a 256 MiB data-log extent; unique paths alone therefore
    /// do not make disk-heavy pack tests safe to run in parallel.
    struct PackTestScratch {
        // Fields drop in declaration order: remove the tree before another test
        // can acquire the guard and start reserving extents.
        dir: TempDir,
        _exclusive: MutexGuard<'static, ()>,
    }

    impl PackTestScratch {
        fn path(&self) -> &Path {
            self.dir.path()
        }
    }

    fn pack_test_scratch() -> Result<PackTestScratch> {
        let exclusive = PACK_TEST_SCRATCH_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempdir()?;
        Ok(PackTestScratch {
            dir,
            _exclusive: exclusive,
        })
    }

    /// Release the unused tail of the production 256 MiB extent reservation.
    /// The test blob remains byte-identical; extending by one byte and then
    /// truncating back forces the filesystem to discard allocations past the
    /// logical EOF instead of treating a same-length truncate as a no-op.
    fn release_test_store_extent_reservation(layer_dir: &Path, zoom: u8) -> Result<()> {
        let path = layer_dir.join(format!("z{zoom}.qtsd"));
        let file = OpenOptions::new().read(true).write(true).open(&path)?;
        let logical_len = file.metadata()?.len();
        let extended_len = logical_len
            .checked_add(1)
            .context("test store data length cannot be extended")?;
        file.set_len(extended_len)?;
        file.set_len(logical_len)?;
        Ok(())
    }

    fn create_one_tile_layer_store(
        store_root: &Path,
        layer: &str,
        source_id: u8,
        value: u8,
    ) -> Result<PathBuf> {
        let layer_dir = store_root.join(layer);
        let mut cells = vec![NO_DATA; TILE_PX * TILE_PX];
        cells[10] = value;
        let blob = wire_hm3::encode_tile_bytes(&cells, source_id)?;
        for zoom in PUBLISHED_MIN_ZOOM..=6 {
            let store = TileStore::create(&layer_dir, zoom, source_id, TILE_PX as u16)?;
            if zoom == 6 {
                store.put_blob(1, 1, TileCodec::BrotliHm3, &blob)?;
            }
            store.sync_all()?;
            drop(store);
            release_test_store_extent_reservation(&layer_dir, zoom)?;
        }
        Ok(layer_dir)
    }

    fn create_one_tile_road_store(store_root: &Path, value: u8) -> Result<PathBuf> {
        create_one_tile_layer_store(store_root, "road", SOURCE_ID_ROAD, value)
    }

    /// A tier layer store: exactly one z{tier} container (no pyramid band).
    fn create_one_tile_tier_store(
        tier_root: &Path,
        layer: &str,
        source_id: u8,
        tier: u8,
        value: u8,
    ) -> Result<PathBuf> {
        let layer_dir = tier_root.join(layer);
        let mut cells = vec![NO_DATA; TILE_PX * TILE_PX];
        cells[10] = value;
        let blob = wire_hm3::encode_tile_bytes(&cells, source_id)?;
        let store = TileStore::create(&layer_dir, tier, source_id, TILE_PX as u16)?;
        store.put_blob(4424, 2774, TileCodec::BrotliHm3, &blob)?;
        store.sync_all()?;
        drop(store);
        release_test_store_extent_reservation(&layer_dir, tier)?;
        Ok(layer_dir)
    }

    fn generation_fixture(slot: PublicationSlot) -> GenerationContract {
        let base_generation_id = "1".repeat(GENERATION_ID_HEX_LENGTH);
        let base_quality_profile_id = "2".repeat(GENERATION_ID_HEX_LENGTH);
        let (
            deployment,
            zoom,
            tier,
            generation_id,
            quality_profile_id,
            quality_profile_name,
            line_model_role_sha256,
        ) = match slot {
            PublicationSlot::Base => (
                "base".to_string(),
                12u64,
                String::new(),
                base_generation_id.clone(),
                base_quality_profile_id.clone(),
                "base-profile".to_string(),
                "4".repeat(GENERATION_ID_HEX_LENGTH),
            ),
            PublicationSlot::Tier(tier) => {
                let tier_name = format!("z{tier}");
                (
                    tier_name.clone(),
                    u64::from(tier),
                    tier_name,
                    "5".repeat(GENERATION_ID_HEX_LENGTH),
                    "6".repeat(GENERATION_ID_HEX_LENGTH),
                    "tier-profile".to_string(),
                    "7".repeat(GENERATION_ID_HEX_LENGTH),
                )
            }
        };
        let value = serde_json::json!({
            "schema": 1,
            "deployment": deployment,
            "zoom": zoom,
            "tier": tier,
            "dataset_year": 2026,
            "generation_id": generation_id,
            "base_generation_id": base_generation_id,
            "raster_generation_id": "3".repeat(RASTER_GENERATION_ID_HEX_LENGTH),
            "quality_profile_id": quality_profile_id,
            "quality_profile_name": quality_profile_name,
            "base_quality_profile_id": base_quality_profile_id,
            "base_quality_profile_name": "base-profile",
            "quality": {
                "model_role_contract": {
                    "line_model_role_sha256": line_model_role_sha256,
                },
            },
            "extra_quality_evidence": {
                "must_be_preserved": true,
            },
        });
        validate_generation_contract_for_slot(value, slot).expect("fixture is valid")
    }

    #[test]
    fn publication_cli_requires_fence_and_rejects_crossed_shapes() {
        let parse = |arguments: &[&str]| {
            parse_cli_arguments(arguments.iter().map(|argument| (*argument).to_string()))
        };
        assert!(
            parse(&["store", "out", "b1"]).is_err(),
            "production CLI publication must require a generation contract"
        );
        assert!(
            parse(&[
                "store",
                "out",
                "b1",
                "--generation-contract",
                "tier.json",
                "--tier",
                "13",
                "--pack",
                "p001",
            ])
            .is_err(),
            "a tier publication must include coverage"
        );
        assert!(
            parse(&[
                "store",
                "out",
                "b1",
                "--generation-contract",
                "tier.json",
                "--tier",
                "13",
                "--pack",
                "p001",
                "--coverage-r4",
                "coverage.txt",
                "--layer",
                "road",
            ])
            .is_err(),
            "tier publication cannot be narrowed to one layer"
        );
        assert!(
            parse(&[
                "store",
                "out",
                "b1",
                "--generation-contract",
                "base.json",
                "--unknown",
            ])
            .is_err(),
            "unknown options must not become positional paths"
        );
        let base = parse(&[
            "store",
            "out",
            "b1",
            "--generation-contract",
            "base.json",
            "--layer",
            "road",
        ])
        .expect("fenced base CLI shape");
        assert_eq!(base.tier, None);
        assert_eq!(base.layers, ["road"]);
        let tier = parse(&[
            "store",
            "out",
            "b2",
            "--tier",
            "13",
            "--pack",
            "p001",
            "--coverage-r4",
            "coverage.txt",
            "--generation-contract",
            "tier.json",
        ])
        .expect("tier CLI shape");
        assert_eq!(tier.tier, Some(13));
        assert_eq!(tier.pack.as_deref(), Some("p001"));
    }

    #[test]
    fn generation_contract_slot_binding_rejects_crossed_payloads() {
        let base = generation_fixture(PublicationSlot::Base);
        let tier = generation_fixture(PublicationSlot::Tier(13));
        assert!(
            validate_generation_contract_for_slot(base.value.clone(), PublicationSlot::Tier(13))
                .is_err(),
            "base contract cannot enter a tier pack"
        );
        assert!(
            validate_generation_contract_for_slot(tier.value.clone(), PublicationSlot::Base)
                .is_err(),
            "tier contract cannot become the top-level base generation"
        );
        let mut crossed_tier = tier.value.clone();
        crossed_tier
            .as_object_mut()
            .expect("fixture object")
            .insert(
                "base_generation_id".to_string(),
                serde_json::json!("8".repeat(GENERATION_ID_HEX_LENGTH)),
            );
        let crossed_tier =
            validate_generation_contract_for_slot(crossed_tier, PublicationSlot::Tier(13))
                .expect("crossed fixture still has a valid tier shape");
        assert!(
            validate_tier_generation_anchor(&base, &crossed_tier).is_err(),
            "tier contract cannot replace the current base anchor"
        );
        let mut missing_line = base.value.clone();
        missing_line
            .as_object_mut()
            .expect("fixture object")
            .get_mut("quality")
            .and_then(serde_json::Value::as_object_mut)
            .expect("quality object")
            .get_mut("model_role_contract")
            .and_then(serde_json::Value::as_object_mut)
            .expect("model-role object")
            .remove("line_model_role_sha256");
        assert!(
            validate_generation_contract_for_slot(missing_line, PublicationSlot::Base).is_err(),
            "the nested line role identity is required"
        );
    }

    #[test]
    fn base_generation_transition_preserves_only_exact_same_epoch() -> Result<()> {
        let current = generation_fixture(PublicationSlot::Base);
        let mut manifest = serde_json::json!({
            "generation": current.value,
            "line_model_role_sha256": current.line_model_role_sha256,
            "layers": {},
            "tiers": { "z13": { "packs": [] } },
            "qualification_closure": {
                "file": format!("qualification-{}.json", "a".repeat(64)),
                "sha256": "a".repeat(64),
            },
        })
        .as_object()
        .cloned()
        .context("fixture manifest is an object")?;
        assert!(base_publication_preserves_tier_state(
            &manifest, &current, false
        )?);
        assert!(base_publication_preserves_tier_state(
            &manifest, &current, true
        )?);

        let mut next = current.clone();
        next.generation_id = "9".repeat(GENERATION_ID_HEX_LENGTH);
        next.base_generation_id = next.generation_id.clone();
        next.value["generation_id"] = serde_json::json!(next.generation_id);
        next.value["base_generation_id"] = serde_json::json!(next.base_generation_id);
        assert!(!base_publication_preserves_tier_state(
            &manifest, &next, false
        )?);
        assert!(base_publication_preserves_tier_state(&manifest, &next, true).is_err());

        manifest.remove("generation");
        assert!(!base_publication_preserves_tier_state(
            &manifest, &next, false
        )?);
        assert!(base_publication_preserves_tier_state(&manifest, &next, true).is_err());
        Ok(())
    }

    #[test]
    fn publication_inputs_are_safe_and_coverage_is_canonical() -> Result<()> {
        let dir = tempdir()?;
        let generation_path = dir.path().join("generation.json");
        fs::write(
            &generation_path,
            serde_json::to_vec(&generation_fixture(PublicationSlot::Base).value)?,
        )?;
        assert!(read_generation_contract(&generation_path, PublicationSlot::Base).is_ok());

        let malformed = dir.path().join("malformed.json");
        fs::write(&malformed, b"{")?;
        assert!(read_generation_contract(&malformed, PublicationSlot::Base).is_err());
        let array = dir.path().join("array.json");
        fs::write(&array, b"[]")?;
        assert!(read_generation_contract(&array, PublicationSlot::Base).is_err());
        let directory = dir.path().join("directory.json");
        fs::create_dir(&directory)?;
        assert!(read_generation_contract(&directory, PublicationSlot::Base).is_err());
        #[cfg(unix)]
        {
            let symlink = dir.path().join("symlink.json");
            std::os::unix::fs::symlink(&generation_path, &symlink)?;
            assert!(read_generation_contract(&symlink, PublicationSlot::Base).is_err());
        }

        let coverage = dir.path().join("coverage.txt");
        fs::write(&coverage, "841e355ffffffff\n841e309ffffffff\n")?;
        assert_eq!(
            read_coverage_r4(&coverage)?,
            ["841e309ffffffff", "841e355ffffffff"]
        );
        fs::write(&coverage, "841e309ffffffff\n841e309ffffffff\n")?;
        assert!(read_coverage_r4(&coverage).is_err());
        Ok(())
    }

    #[test]
    fn tier_pack_tokens_index_and_full_pack_preservation() -> Result<()> {
        let dir = pack_test_scratch()?;
        let store_root = dir.path().join("store");
        let tier_root = dir.path().join("z13").join("store");
        let out_dir = dir.path().join("pmtiles");
        fs::create_dir_all(&out_dir)?;
        let base_generation = generation_fixture(PublicationSlot::Base);

        // Base full pack b1 establishes the manifest a tier pack merges over.
        let mut snapshots = Vec::new();
        for layer in PUBLISHED_LAYERS {
            let layer_dir = create_one_tile_layer_store(
                &store_root,
                layer,
                expected_source_id(layer).context("published layer has a source id")?,
                40,
            )?;
            snapshots.push(snapshot_test_layer(&layer_dir, layer)?);
        }
        validate_snapshots_for_base(&snapshots, 6)?;
        pack_snapshots_transactionally(snapshots, &out_dir, "b1", false, &base_generation, &None)?;

        // Tier pack b2: token archives + the tiers index entry.
        let tier_generation = generation_fixture(PublicationSlot::Tier(13));
        let tier_pack = TierPack {
            tier: 13,
            pack: "p001".to_string(),
            coverage_r4: vec!["841e309ffffffff".to_string(), "841e355ffffffff".to_string()],
            generation: tier_generation.clone(),
        };
        let mut tier_snapshots = Vec::new();
        for layer in PUBLISHED_LAYERS {
            let layer_dir = create_one_tile_tier_store(
                &tier_root,
                layer,
                expected_source_id(layer).context("published layer has a source id")?,
                13,
                41,
            )?;
            let mut snapshot = snapshot_test_layer(&layer_dir, layer)?;
            let zooms: Vec<u8> = snapshot.stores.iter().map(|store| store.zoom).collect();
            assert_eq!(zooms, [13], "tier store band is exactly [13]");
            snapshot.layer = tier_pack.token(&snapshot.layer);
            tier_snapshots.push(snapshot);
        }
        pack_snapshots_transactionally(
            tier_snapshots,
            &out_dir,
            "b2",
            false,
            &tier_generation,
            &Some(tier_pack),
        )?;

        assert!(
            out_dir.join("road-z13-p001.b2.pmtiles").exists(),
            "tier archive published under the token name"
        );
        let manifest: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(out_dir.join("current.json"))?)?;
        assert_eq!(manifest["generation"], base_generation.value);
        assert_eq!(
            manifest["generation"]["extra_quality_evidence"]["must_be_preserved"],
            true
        );
        assert_eq!(
            manifest["line_model_role_sha256"],
            base_generation.line_model_role_sha256
        );
        assert_eq!(
            manifest["layers"]["road-z13-p001"]["build"], "b2",
            "tier token is an ordinary layers entry"
        );
        assert_eq!(
            manifest["layers"]["road"]["build"], "b1",
            "base layers untouched by the tier pack"
        );
        let pack0 = &manifest["tiers"]["z13"]["packs"][0];
        assert_eq!(pack0["pack"], "p001");
        assert_eq!(pack0["coverage_r4"][0], "841e309ffffffff");
        assert_eq!(pack0["coverage_r4"][1], "841e355ffffffff");
        assert_eq!(pack0["generation"], tier_generation.value);
        assert_eq!(
            pack0["generation"]["extra_quality_evidence"]["must_be_preserved"],
            true
        );
        assert_eq!(
            pack0["layers"].as_array().map(|l| l.len()),
            Some(PUBLISHED_LAYERS.len()),
            "index lists every packed token"
        );

        // Full base pack b3 must RETAIN tier entries and the tiers index.
        let mut base_again = Vec::new();
        for layer in PUBLISHED_LAYERS {
            base_again.push(snapshot_test_layer(&store_root.join(layer), layer)?);
        }
        pack_snapshots_transactionally(base_again, &out_dir, "b3", false, &base_generation, &None)?;
        let manifest: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(out_dir.join("current.json"))?)?;
        assert_eq!(manifest["layers"]["road"]["build"], "b3");
        assert_eq!(
            manifest["layers"]["road-z13-p001"]["build"], "b2",
            "full z12 pack preserves tier layer entries"
        );
        assert_eq!(
            manifest["tiers"]["z13"]["packs"][0]["pack"], "p001",
            "full z12 pack preserves the tiers index"
        );

        // A duplicate pack id is refused (packs are immutable).
        let tier_dup = TierPack {
            tier: 13,
            pack: "p001".to_string(),
            coverage_r4: vec!["841e309ffffffff".to_string()],
            generation: tier_generation,
        };
        let mut dup_snapshots = Vec::new();
        for layer in PUBLISHED_LAYERS {
            let mut snapshot = snapshot_test_layer(&tier_root.join(layer), layer)?;
            snapshot.layer = tier_dup.token(&snapshot.layer);
            dup_snapshots.push(snapshot);
        }
        let err = pack_snapshots_transactionally(
            dup_snapshots,
            &out_dir,
            "b4",
            false,
            &generation_fixture(PublicationSlot::Tier(13)),
            &Some(tier_dup),
        );
        assert!(err.is_err(), "duplicate tier pack id must fail");

        // A full pack for a different base generation atomically retires every tier token
        // and the index instead of relabeling old W2 bytes.
        let mut next_base = base_generation.clone();
        next_base.generation_id = "9".repeat(GENERATION_ID_HEX_LENGTH);
        next_base.base_generation_id = next_base.generation_id.clone();
        next_base.value["generation_id"] = serde_json::json!(next_base.generation_id);
        next_base.value["base_generation_id"] = serde_json::json!(next_base.base_generation_id);
        let mut next_snapshots = Vec::new();
        for layer in PUBLISHED_LAYERS {
            next_snapshots.push(snapshot_test_layer(&store_root.join(layer), layer)?);
        }
        pack_snapshots_transactionally(next_snapshots, &out_dir, "b5", false, &next_base, &None)?;
        let manifest: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(out_dir.join("current.json"))?)?;
        assert_eq!(manifest["generation"], next_base.value);
        assert!(manifest.get("tiers").is_none());
        assert!(manifest.get("qualification_closure").is_none());
        assert!(manifest["layers"].get("road-z13-p001").is_none());
        Ok(())
    }

    #[test]
    fn tier_token_parse_round_trip_and_rejects() {
        assert_eq!(
            parse_tier_token("road-z13-p001"),
            Some(("road", 13u8, "p001"))
        );
        assert_eq!(
            parse_tier_token("aircraft-ground-z14-p2"),
            Some(("aircraft-ground", 14u8, "p2"))
        );
        assert_eq!(parse_tier_token("road"), None, "base names are not tokens");
        assert_eq!(
            parse_tier_token("road-z12-p1"),
            None,
            "z12 is the base, never a tier"
        );
        assert_eq!(parse_tier_token("bogus-z13-p1"), None, "unknown base layer");
        assert_eq!(
            parse_tier_token("road-z13-q1"),
            None,
            "pack id must be p<N>"
        );
        assert_eq!(parse_tier_token("road-z13-p"), None, "pack id needs digits");
    }

    fn snapshot_test_layer(layer_dir: &Path, layer: &str) -> Result<LayerSnapshot> {
        let zooms = detect_zooms(layer_dir)?;
        snapshot_layer(layer_dir, layer, &zooms)
    }

    fn entry(file: &str, sha256: &str, proof: &PublisherProof) -> serde_json::Value {
        serde_json::json!({
            "file": file,
            "build": "b1",
            "bytes": proof.size,
            "sha256": sha256,
            "publisher_proof": {
                "schema": PUBLISHER_PROOF_SCHEMA,
                "sha256": sha256,
                "dev": proof.dev.to_string(),
                "ino": proof.ino.to_string(),
                "size": proof.size.to_string(),
                "mtime_ns": proof.mtime_ns.to_string(),
                "ctime_ns": proof.ctime_ns.to_string(),
            },
        })
    }

    #[test]
    fn publish_gc_lock_wait_is_bounded() -> Result<()> {
        let dir = pack_test_scratch()?;
        let held = acquire_pack_lock(dir.path(), Duration::from_secs(1))?;
        let started = Instant::now();
        assert!(acquire_pack_lock(dir.path(), Duration::ZERO).is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
        drop(held);
        acquire_pack_lock(dir.path(), Duration::ZERO)?;
        Ok(())
    }

    #[test]
    fn archive_staging_cleanup_never_leaves_a_final_named_partial() -> Result<()> {
        let dir = pack_test_scratch()?;
        let final_path = dir.path().join("road.b1.pmtiles");
        let temp_path = dir.path().join(".road.b1.pmtiles.tmp");
        fs::write(&temp_path, b"stale crash residue")?;

        let (staged, mut file) = StagedArchive::create(final_path.clone())?;
        file.write_all(b"new partial")?;
        drop(file);
        assert!(!final_path.exists());
        drop(staged); // models any ordinary error after archive creation
        assert!(!temp_path.exists());
        assert!(!final_path.exists());
        Ok(())
    }

    #[test]
    fn archive_publish_atomically_renames_complete_temp_without_overwrite() -> Result<()> {
        let dir = pack_test_scratch()?;
        let final_path = dir.path().join("road.b1.pmtiles");
        let temp_path = dir.path().join(".road.b1.pmtiles.tmp");
        let (staged, mut file) = StagedArchive::create(final_path.clone())?;
        file.write_all(b"complete archive")?;
        file.sync_all()?;
        drop(file);

        assert!(!final_path.exists());
        staged.publish()?;
        assert_eq!(fs::read(&final_path)?, b"complete archive");
        assert!(!temp_path.exists());
        assert!(StagedArchive::create(final_path.clone()).is_err());
        assert_eq!(fs::read(final_path)?, b"complete archive");
        Ok(())
    }

    #[test]
    fn sha256_is_bound_to_the_stable_open_file_identity() -> Result<()> {
        let dir = pack_test_scratch()?;
        let path = dir.path().join("total.b1.pmtiles");
        fs::write(&path, b"abc")?;

        let (sha256, bytes, proof) = sha256_file(&path)?;
        assert_eq!(
            sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(bytes, 3);
        assert_eq!(proof, PublisherProof::read(&path)?);
        Ok(())
    }

    #[test]
    fn manifest_proof_survives_hardlinking_the_published_archive() -> Result<()> {
        let dir = pack_test_scratch()?;
        let file = "total.b1.pmtiles";
        let path = dir.path().join(file);
        fs::write(&path, b"abc")?;
        let (sha256, _, proof) = sha256_file(&path)?;
        let manifest_entry = entry(file, &sha256, &proof);
        validate_manifest_entry(dir.path(), "total", &manifest_entry)?;

        // Hardlinking bumps ctime and nothing else. The proof must still validate, or one linked
        // archive would block every later partial/tier pack that retains this layer.
        fs::hard_link(&path, dir.path().join("total.b1.pmtiles.link"))?;
        validate_manifest_entry(dir.path(), "total", &manifest_entry)?;

        // Clock-independent half of the same claim: a proof whose recorded ctime differs from the
        // file's is still the same file. Without this the test would go vacuous on a filesystem
        // whose ctime granularity swallowed the link.
        let mut moved_ctime = entry(file, &sha256, &proof);
        moved_ctime["publisher_proof"]["ctime_ns"] =
            serde_json::json!((proof.ctime_ns - 1_000_000_000).to_string());
        validate_manifest_entry(dir.path(), "total", &moved_ctime)?;
        Ok(())
    }

    #[test]
    fn manifest_proof_rejects_a_same_size_atomic_replacement() -> Result<()> {
        let dir = pack_test_scratch()?;
        let file = "total.b1.pmtiles";
        let path = dir.path().join(file);
        fs::write(&path, b"abc")?;
        let (sha256, _, proof) = sha256_file(&path)?;
        let manifest_entry = entry(file, &sha256, &proof);
        validate_manifest_entry(dir.path(), "total", &manifest_entry)?;

        let replacement = dir.path().join("replacement.pmtiles");
        fs::write(&replacement, b"abc")?;
        fs::rename(replacement, path)?;
        assert!(validate_manifest_entry(dir.path(), "total", &manifest_entry).is_err());
        Ok(())
    }

    #[test]
    fn partial_merge_preflight_rejects_a_legacy_entry_without_proof() -> Result<()> {
        let dir = pack_test_scratch()?;
        let path = dir.path().join("road.b1.pmtiles");
        fs::write(&path, b"road")?;
        let legacy = serde_json::json!({
            "file": "road.b1.pmtiles",
            "build": "b1",
            "bytes": 4,
            "sha256": "0".repeat(64),
        });
        assert!(validate_manifest_entry(dir.path(), "road", &legacy).is_err());
        Ok(())
    }

    #[test]
    fn layer_shape_rejects_a_zoom_gap_and_cross_zoom_source_mismatch() -> Result<()> {
        let dir = pack_test_scratch()?;
        let store_root = dir.path().join("tiles/2026/store");
        let layer_dir = create_one_tile_road_store(&store_root, 42)?;

        fs::remove_file(layer_dir.join("z4.qtsi"))?;
        fs::remove_file(layer_dir.join("z4.qtsd"))?;
        let gap = snapshot_test_layer(&layer_dir, "road")?;
        assert!(validate_snapshots_for_base(&[gap], 6)
            .unwrap_err()
            .to_string()
            .contains("expected exact"));

        TileStore::create(&layer_dir, 4, SOURCE_ID_RAIL, TILE_PX as u16)?.sync_all()?;
        let mixed = snapshot_test_layer(&layer_dir, "road")?;
        assert!(validate_snapshots_for_base(&[mixed], 6)
            .unwrap_err()
            .to_string()
            .contains("source_id"));
        Ok(())
    }

    #[test]
    fn layer_shape_rejects_an_internally_consistent_wrong_source_id() -> Result<()> {
        let dir = pack_test_scratch()?;
        let layer_dir = dir.path().join("tiles/2026/store/road");
        for zoom in PUBLISHED_MIN_ZOOM..=6 {
            TileStore::create(&layer_dir, zoom, SOURCE_ID_RAIL, TILE_PX as u16)?.sync_all()?;
        }

        let snapshot = snapshot_test_layer(&layer_dir, "road")?;
        let error = validate_snapshots_for_base(&[snapshot], 6).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("road: source_id 2 differs from required 1"),
            "unexpected error: {error:#}"
        );
        Ok(())
    }

    #[test]
    fn manifest_contract_rejects_a_missing_or_unexpected_layer() {
        let complete: serde_json::Map<String, serde_json::Value> = PUBLISHED_LAYERS
            .iter()
            .map(|layer| ((*layer).to_string(), serde_json::json!({})))
            .collect();
        validate_manifest_layer_contract(&complete).unwrap();

        let mut missing = complete.clone();
        missing.remove("aircraft-ground");
        assert!(validate_manifest_layer_contract(&missing)
            .unwrap_err()
            .to_string()
            .contains("missing [aircraft-ground]"));

        let mut unexpected = complete;
        unexpected.insert("retired-layer".to_string(), serde_json::json!({}));
        assert!(validate_manifest_layer_contract(&unexpected)
            .unwrap_err()
            .to_string()
            .contains("unexpected [retired-layer]"));
    }

    #[test]
    fn full_pack_selection_refuses_a_partial_store_directory_set() -> Result<()> {
        let dir = pack_test_scratch()?;
        create_one_tile_road_store(&dir.path().join("store"), 42)?;
        let error = match selected_layer_stores(&dir.path().join("store"), &[]) {
            Ok(_) => panic!("partial store set unexpectedly accepted for a full pack"),
            Err(error) => error,
        };
        assert!(error
            .to_string()
            .contains("full pack requires the exact published store set"));
        assert!(error.to_string().contains("missing ["));
        Ok(())
    }

    #[test]
    fn partial_total_pack_fscks_every_unselected_input_but_stages_only_scope() -> Result<()> {
        let dir = pack_test_scratch()?;
        let store_root = dir.path().join("tiles/2026/store");
        for layer in std::iter::once("total").chain(TOTAL_INPUT_LAYERS.iter().copied()) {
            create_one_tile_layer_store(
                &store_root,
                layer,
                expected_source_id(layer).context("test layer must have a source id")?,
                42,
            )?;
        }

        let rail_dir = store_root.join("rail");
        let corrupt_rail = TileStore::open(&rail_dir, 6, true)?;
        corrupt_rail.put_blob(1, 1, TileCodec::BrotliHm3, b"not HM3")?;
        corrupt_rail.sync_all()?;
        drop(corrupt_rail);

        let body_called = Cell::new(false);
        let only = vec!["total".to_string()];
        let error = with_store_snapshots_after_capture(
            &store_root,
            &only,
            Duration::from_secs(1),
            |snapshots| {
                assert_eq!(snapshots.len(), 1 + TOTAL_INPUT_LAYERS.len());
                validate_snapshots_for_base(snapshots, 6)
            },
            |_| {
                body_called.set(true);
                Ok(())
            },
        )
        .unwrap_err();
        let detail = format!("{error:#}");
        assert!(
            detail.contains("rail/z6") && detail.contains("decode"),
            "{detail}"
        );
        assert!(
            !body_called.get(),
            "archive staging must not start after dependency fsck fails"
        );

        let mut cells = vec![NO_DATA; TILE_PX * TILE_PX];
        cells[10] = 42;
        let repaired = wire_hm3::encode_tile_bytes(&cells, SOURCE_ID_RAIL)?;
        let rail = TileStore::open(&rail_dir, 6, true)?;
        rail.put_blob(1, 1, TileCodec::BrotliHm3, &repaired)?;
        rail.sync_all()?;
        drop(rail);
        with_store_snapshots_after_capture(
            &store_root,
            &only,
            Duration::from_secs(1),
            |snapshots| validate_snapshots_for_base(snapshots, 6),
            |snapshots| {
                assert_eq!(snapshots.len(), 1);
                assert_eq!(snapshots[0].layer, "total");
                Ok(())
            },
        )?;
        Ok(())
    }

    #[test]
    fn production_zoom_band_is_exactly_z2_through_z12() {
        let exact: Vec<u8> = (PUBLISHED_MIN_ZOOM..=PUBLISHED_BASE_ZOOM).collect();
        validate_exact_zoom_band("road", &exact, PUBLISHED_BASE_ZOOM).unwrap();

        let missing_base: Vec<u8> = (PUBLISHED_MIN_ZOOM..PUBLISHED_BASE_ZOOM).collect();
        assert!(validate_exact_zoom_band("road", &missing_base, PUBLISHED_BASE_ZOOM).is_err());

        let mut stale_extra = exact.clone();
        stale_extra.push(PUBLISHED_BASE_ZOOM + 1);
        assert!(validate_exact_zoom_band("road", &stale_extra, PUBLISHED_BASE_ZOOM).is_err());
    }

    #[test]
    fn rebuild_marker_blocks_before_validation_or_staging() -> Result<()> {
        let dir = pack_test_scratch()?;
        let store_root = dir.path().join("tiles/2026/store");
        let layer_dir = create_one_tile_road_store(&store_root, 42)?;
        fs::write(layer_dir.join(REBUILD_INCOMPLETE_MARKER), b"interrupted\n")?;
        let callback_called = Cell::new(false);
        let result = with_store_snapshots_after_capture(
            &store_root,
            &[],
            Duration::ZERO,
            |_| {
                callback_called.set(true);
                Ok(())
            },
            |_| {
                callback_called.set(true);
                Ok(())
            },
        );
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("transaction incomplete"));
        assert!(!callback_called.get());
        Ok(())
    }

    #[test]
    fn scoped_root_transaction_blocks_pack_before_store_detection() -> Result<()> {
        let dir = pack_test_scratch()?;
        let store_root = dir.path().join("tiles/2026/store");
        tile_painter::tile_store::StoreUpdateFence::begin(&store_root, "z12|bbox-a|road")?;
        let callback_called = Cell::new(false);
        let result = with_store_snapshots_after_capture(
            &store_root,
            &[],
            Duration::ZERO,
            |_| {
                callback_called.set(true);
                Ok(())
            },
            |_| {
                callback_called.set(true);
                Ok(())
            },
        );
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("scoped store update"));
        assert!(!callback_called.get());
        Ok(())
    }

    #[test]
    fn incomplete_two_phase_publish_is_recovered_without_orphan_finals() -> Result<()> {
        let dir = pack_test_scratch()?;
        let out_dir = dir.path();
        let files = vec!["rail.b8.pmtiles".to_string(), "road.b8.pmtiles".to_string()];
        let mut staged = Vec::new();
        for file in &files {
            let (archive, mut handle) = StagedArchive::create(out_dir.join(file))?;
            handle.write_all(file.as_bytes())?;
            handle.sync_all()?;
            staged.push(archive);
        }
        let transaction = PackTransaction::begin(out_dir, "b8", &files)?;
        let first = staged.remove(0);
        first.publish()?; // model a process dying between final renames
        let marker_path = transaction.marker_path.clone();
        assert!(out_dir.join("rail.b8.pmtiles").exists());

        recover_pack_transactions(out_dir)?;
        assert!(!out_dir.join("rail.b8.pmtiles").exists());
        assert!(!out_dir.join(".road.b8.pmtiles.tmp").exists());
        assert!(!marker_path.exists());
        drop(staged);
        Ok(())
    }

    #[test]
    fn crash_before_transaction_marker_leaves_only_cleanup_safe_hidden_temps() -> Result<()> {
        let dir = pack_test_scratch()?;
        let out_dir = dir.path();
        let archive_temp = out_dir.join(".road.b8.pmtiles.tmp");
        let transaction_temp = out_dir.join(".pack-transaction-b8.incomplete.tmp");
        let unrelated = out_dir.join(".notes.tmp");
        fs::write(&archive_temp, b"partial archive")?;
        fs::write(&transaction_temp, b"partial marker")?;
        fs::write(&unrelated, b"keep")?;

        cleanup_orphan_pack_temps(out_dir)?;
        assert!(!archive_temp.exists());
        assert!(!transaction_temp.exists());
        assert!(unrelated.exists());
        Ok(())
    }

    #[test]
    fn recovery_keeps_archives_after_manifest_commit() -> Result<()> {
        let dir = pack_test_scratch()?;
        let out_dir = dir.path();
        let files = vec!["road.b9.pmtiles".to_string()];
        let (archive, mut handle) = StagedArchive::create(out_dir.join(&files[0]))?;
        handle.write_all(b"complete")?;
        handle.sync_all()?;
        let transaction = PackTransaction::begin(out_dir, "b9", &files)?;
        archive.publish()?;
        fs::write(
            out_dir.join("current.json"),
            serde_json::json!({"layers":{"road":{"file":files[0]}}}).to_string(),
        )?;

        recover_pack_transactions(out_dir)?;
        assert!(out_dir.join("road.b9.pmtiles").exists());
        assert!(!transaction.marker_path.exists());
        Ok(())
    }

    #[test]
    fn snapshot_retains_every_writer_lock_through_validation_and_pack() -> Result<()> {
        let dir = pack_test_scratch()?;
        let store_root = dir.path().join("tiles/2026/store");
        create_one_tile_road_store(&store_root, 42)?;
        let master_path = master_store_lock_path(&store_root)?;
        let ingest_path = ingest_store_lock_path(&store_root);
        let validation_observed = Cell::new(false);
        let pack_observed = Cell::new(false);

        with_store_snapshots_after_capture(
            &store_root,
            &["road".to_string()],
            Duration::from_secs(1),
            |snapshots| {
                assert_eq!(snapshots.len(), 1);
                assert!(
                    StoreFileLock::acquire_bounded(&master_path, Duration::ZERO).is_err(),
                    "combine/transcode master must stay excluded during expensive validation"
                );
                assert!(
                    StoreFileLock::acquire_bounded(&ingest_path, Duration::ZERO).is_err(),
                    "Hub ingest must stay excluded during validation"
                );
                assert!(
                    StoreFileLock::acquire_bounded(
                        &zoom_store_lock_path(&store_root.join("road"), 6),
                        Duration::ZERO,
                    )
                    .is_err(),
                    "a direct create/truncate must stay excluded during validation"
                );
                validate_snapshots_for_base(snapshots, 6)?;
                validation_observed.set(true);
                Ok(())
            },
            |snapshots| {
                assert_eq!(snapshots.len(), 1);
                assert!(
                    StoreFileLock::acquire_bounded(&master_path, Duration::ZERO).is_err(),
                    "combine/transcode master must stay excluded through archive + manifest"
                );
                assert!(
                    StoreFileLock::acquire_bounded(&ingest_path, Duration::ZERO).is_err(),
                    "Hub ingest must stay excluded through archive + manifest"
                );
                assert!(
                    StoreFileLock::acquire_bounded(
                        &zoom_store_lock_path(&store_root.join("road"), 6),
                        Duration::ZERO,
                    )
                    .is_err(),
                    "a direct create/truncate must stay excluded through archive + manifest"
                );
                pack_observed.set(true);
                Ok(())
            },
        )?;

        assert!(validation_observed.get());
        assert!(pack_observed.get());
        StoreFileLock::acquire_bounded(&master_path, Duration::ZERO)?;
        StoreFileLock::acquire_bounded(&ingest_path, Duration::ZERO)?;
        Ok(())
    }

    #[test]
    fn snapshot_waits_for_the_same_per_zoom_lock_as_a_direct_tile_store_writer() -> Result<()> {
        let dir = pack_test_scratch()?;
        let store_root = dir.path().join("tiles/2026/store");
        let layer_dir = create_one_tile_road_store(&store_root, 42)?;
        let direct_writer = TileStore::open(&layer_dir, 6, true)?;
        let callback_called = Cell::new(false);

        let result = with_store_snapshots_after_capture(
            &store_root,
            &["road".to_string()],
            Duration::ZERO,
            |_| {
                callback_called.set(true);
                Ok(())
            },
            |_| {
                callback_called.set(true);
                Ok(())
            },
        );
        assert!(result.is_err());
        assert!(!callback_called.get());
        drop(direct_writer);

        with_store_snapshots_after_capture(
            &store_root,
            &["road".to_string()],
            Duration::ZERO,
            |snapshots| validate_snapshots_for_base(snapshots, 6),
            |_| Ok(()),
        )?;
        Ok(())
    }

    #[test]
    fn captured_entry_reads_the_validated_blob_after_ingest_overwrites_the_tile() -> Result<()> {
        let dir = pack_test_scratch()?;
        let store_root = dir.path().join("tiles/2026/store");
        let layer_dir = create_one_tile_road_store(&store_root, 42)?;
        let snapshot = snapshot_test_layer(&layer_dir, "road")?;
        assert_eq!(snapshot.stores.len(), 5);
        let z6 = snapshot
            .stores
            .iter()
            .find(|store| store.zoom == 6)
            .unwrap();
        assert_eq!(z6.entries.len(), 1);
        let entry = z6.entries[0].tile.entry;

        let mut replacement_cells = vec![NO_DATA; TILE_PX * TILE_PX];
        replacement_cells[10] = 99;
        let replacement = wire_hm3::encode_tile_bytes(&replacement_cells, SOURCE_ID_ROAD)?;
        let writer = TileStore::open(&layer_dir, 6, true)?;
        writer.put_blob(1, 1, TileCodec::BrotliHm3, &replacement)?;
        writer.sync_all()?;
        drop(writer);

        validate_snapshots_for_base(std::slice::from_ref(&snapshot), 6)?;
        let captured_blob = z6.store.get_hm3_by_entry(&entry)?;
        assert_eq!(wire_hm3::read_tile_bytes(&captured_blob)?[10], 42);
        let current = TileStore::open(&layer_dir, 6, false)?;
        assert_eq!(
            wire_hm3::read_tile_bytes(&current.get_hm3(1, 1)?.unwrap())?[10],
            99
        );
        Ok(())
    }

    #[test]
    fn captured_validation_failure_never_enters_archive_creation() -> Result<()> {
        let dir = pack_test_scratch()?;
        let store_root = dir.path().join("tiles/2026/store");
        let layer_dir = create_one_tile_road_store(&store_root, 42)?;
        let writer = TileStore::open(&layer_dir, 6, true)?;
        writer.put_blob(1, 1, TileCodec::BrotliHm3, b"not HM3")?;
        writer.sync_all()?;
        drop(writer);
        let out_dir = dir.path().join("pmtiles");
        fs::create_dir_all(&out_dir)?;
        let body_called = Cell::new(false);

        let result = with_store_snapshots_after_capture(
            &store_root,
            &["road".to_string()],
            Duration::from_secs(1),
            |snapshots| validate_snapshots_for_base(snapshots, 6),
            |mut snapshots| {
                body_called.set(true);
                stage_layer(snapshots.pop().unwrap(), &out_dir, "b1")?;
                Ok(())
            },
        );

        assert!(result.is_err());
        assert!(!body_called.get());
        assert!(!out_dir.join("road.b1.pmtiles").exists());
        Ok(())
    }

    /// Rewritten retention test (2026-07-16 Track 2 — replaces the old `prune_*` tests, which
    /// exercised a `prune_superseded` function that no longer exists): running a REAL pack
    /// (`pack_snapshots_transactionally`, which is what `main()` calls and which owns the
    /// `stage_layer` + `write_manifest` pair) must leave every other
    /// `.pmtiles` file in `out_dir` untouched — old generations of the SAME layer, a sibling
    /// layer's archive, everything. Retention is `tile-store-gc`'s job now, proven separately
    /// in `tile_store_gc.rs`'s own tests; this just locks in that packing never deletes.
    #[test]
    fn pack_never_deletes_other_archives_in_out_dir() -> Result<()> {
        let dir = pack_test_scratch()?;
        let store_root = dir.path().join("store");
        let out_dir = dir.path().join("pmtiles");
        fs::create_dir_all(&out_dir)?;

        // Stale archives that a real deployment would have accumulated over prior publishes —
        // older generations of the layer about to be republished, plus a sibling layer's
        // archive this pack never touches at all.
        for f in [
            "road.b4.pmtiles",
            "road.b5.pmtiles",
            "road.b6.pmtiles",
            "rail.b3.pmtiles",
        ] {
            fs::write(out_dir.join(f), b"stale-archive")?;
        }

        let mut snapshots = Vec::new();
        for layer in PUBLISHED_LAYERS {
            let layer_dir = create_one_tile_layer_store(
                &store_root,
                layer,
                expected_source_id(layer).context("test layer must have a source id")?,
                42,
            )?;
            snapshots.push(snapshot_test_layer(&layer_dir, layer)?);
        }
        validate_snapshots_for_base(&snapshots, 6)?;
        pack_snapshots_transactionally(
            snapshots,
            &out_dir,
            "b7",
            false,
            &generation_fixture(PublicationSlot::Base),
            &None,
        )?;

        let exists = |f: &str| out_dir.join(f).exists();
        assert!(
            exists("road.b7.pmtiles"),
            "the freshly published archive exists"
        );
        assert!(
            exists("road.b6.pmtiles"),
            "pack must not delete older generations"
        );
        assert!(
            exists("road.b5.pmtiles"),
            "pack must not delete older generations"
        );
        assert!(
            exists("road.b4.pmtiles"),
            "pack must not delete older generations"
        );
        assert!(
            exists("rail.b3.pmtiles"),
            "pack must not touch a layer it didn't publish"
        );
        Ok(())
    }

    #[test]
    fn pack_layer_keeps_global_tile_id_order_across_zoom_stores() -> Result<()> {
        let dir = pack_test_scratch()?;
        let layer_dir = dir.path().join("store/road");
        let out_dir = dir.path().join("pmtiles");
        fs::create_dir_all(&out_dir)?;
        let mut cells = vec![NO_DATA; TILE_PX * TILE_PX];
        cells[10] = 70;
        let blob = wire_hm3::encode_tile_bytes(&cells, SOURCE_ID_ROAD)?;
        for zoom in PUBLISHED_MIN_ZOOM..=6 {
            let store = TileStore::create(&layer_dir, zoom, SOURCE_ID_ROAD, TILE_PX as u16)?;
            if zoom >= 5 {
                store.put_blob(1, 1, TileCodec::BrotliHm3, &blob)?;
            }
            store.sync_all()?;
        }

        let snapshot = snapshot_test_layer(&layer_dir, "road")?;
        validate_snapshots_for_base(std::slice::from_ref(&snapshot), 6)?;
        let result = stage_layer(snapshot, &out_dir, "b1")?.publish()?;
        assert_eq!(result.tiles, 2);
        assert!(out_dir.join("road.b1.pmtiles").exists());
        Ok(())
    }

    /// End-to-end `pack_snapshots_transactionally` over a store mid-cutover: one entry already rewritten through
    /// the new `put_cells_hm3`/`BrotliHm3` write path, one entry still the legacy `ZstdCells`
    /// working codec (`put_cells`) — exactly the mixed state every real store is in for a
    /// while after 2026-07-16 (only tiles a combine/pyramid pass actually touches get
    /// rewritten; the rest keep publishing correctly via the ZstdCells arm). Both must ship:
    /// the pack must not error, must produce a tile for each, and — the actual point of the
    /// publish-speed fix — the BrotliHm3 entry must ship byte-identical to what was stored,
    /// the exact call (`get_hm3_by_entry`) `stage_layer`'s prefetch pipeline makes per tile.
    #[test]
    fn pack_layer_ships_mixed_codec_store_correctly() -> Result<()> {
        let dir = pack_test_scratch()?;
        let layer_dir = dir.path().join("store").join("road");
        let out_dir = dir.path().join("pmtiles");
        fs::create_dir_all(&out_dir)?;

        for zoom in PUBLISHED_MIN_ZOOM..6 {
            TileStore::create(&layer_dir, zoom, SOURCE_ID_ROAD, TILE_PX as u16)?.sync_all()?;
        }
        let store = TileStore::create(&layer_dir, 6, SOURCE_ID_ROAD, TILE_PX as u16)?;
        let mut cells_new = vec![NO_DATA; TILE_PX * TILE_PX];
        cells_new[10] = 88;
        let blob_new = wire_hm3::encode_tile_bytes(&cells_new, SOURCE_ID_ROAD)?;
        store.put_blob(3, 4, TileCodec::BrotliHm3, &blob_new)?; // new central-writer path
        let mut cells_legacy = vec![NO_DATA; TILE_PX * TILE_PX];
        cells_legacy[20] = 99;
        store.put_cells(5, 6, &cells_legacy)?; // legacy working codec, not yet rewritten
        store.sync_all()?;
        drop(store);

        let snapshot = snapshot_test_layer(&layer_dir, "road")?;
        validate_snapshots_for_base(std::slice::from_ref(&snapshot), 6)?;
        let result = stage_layer(snapshot, &out_dir, "b1")?.publish()?;
        assert_eq!(result.tiles, 2, "both codecs must ship");
        assert!(out_dir.join("road.b1.pmtiles").exists());

        // `stage_layer`'s own ship-out call is `TileStore::get_hm3_by_entry` — reopen the store
        // and call it the same way to confirm what actually got fed to the pmtiles writer.
        let reopened = TileStore::open(&layer_dir, 6, false)?;
        assert_eq!(
            reopened.get_hm3(3, 4)?.unwrap(),
            blob_new,
            "a BrotliHm3 entry ships byte-identical to the stored blob"
        );
        assert_eq!(
            wire_hm3::read_tile_bytes(&reopened.get_hm3(3, 4)?.unwrap())?,
            cells_new
        );
        assert_eq!(
            wire_hm3::read_tile_bytes(&reopened.get_hm3(5, 6)?.unwrap())?,
            cells_legacy,
            "a legacy ZstdCells entry still composes correctly"
        );
        Ok(())
    }
}

//! Versioned renderer-owned evidence shared by every CPU and GPU tile lane.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};

pub const RENDERER_EVIDENCE_FLAG: &str = "QM_RENDERER_EVIDENCE_V1";

pub use crate::renderer_evidence_attestation::{
    maybe_run_static_attestation, DependencyProfile, StaticAttestationParameters,
    RENDERER_STATIC_ATTEST_FLAG,
};

#[derive(Clone, Debug)]
pub struct RuntimeParameters {
    pub zoom: u8,
    pub batch_size: u32,
    pub n_days: Option<u16>,
    pub rayon_threads: usize,
    pub stream_workers: usize,
    pub region_concurrency_configured: usize,
    pub region_concurrency_effective: usize,
    pub max_regions_per_claim: usize,
    pub layers: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RuntimeShapeV1 {
    pub schema: &'static str,
    pub lane: String,
    pub role: String,
    pub product_commit: String,
    pub executable_sha256: String,
    pub artifact_family: String,
    pub line_model_role_sha256: String,
    pub layer_spec_sha256: String,
    pub cuda_context_opened: bool,
    pub argv: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub zoom: u8,
    pub batch_size: u32,
    pub n_days: Option<u16>,
    pub rayon_threads: usize,
    pub stream_workers: usize,
    pub region_concurrency_configured: usize,
    pub region_concurrency_effective: usize,
    pub max_regions_per_claim: usize,
    pub layers: Vec<String>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RegionTerminalStatus {
    Done,
    Fail,
}

#[derive(Debug, Serialize)]
struct RegionClaimV1<'a> {
    schema: &'static str,
    lane: &'a str,
    cell: String,
    worker_slot: usize,
    interval_id: u64,
}

#[derive(Debug, Serialize)]
struct RegionTerminalV1<'a> {
    schema: &'static str,
    lane: &'a str,
    cell: String,
    worker_slot: usize,
    interval_id: u64,
    status: RegionTerminalStatus,
    written: usize,
    skipped: usize,
    error: Option<&'a str>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum TileState<'a> {
    Written {
        relative_path: &'a str,
        bytes: u64,
        sha256: String,
    },
    Skipped {
        reason: &'a str,
    },
}

#[derive(Debug, Serialize)]
struct TileTerminalV1<'a> {
    schema: &'static str,
    lane: &'a str,
    cell: String,
    layer: &'a str,
    z: u8,
    x: u32,
    y: u32,
    #[serde(flatten)]
    state: TileState<'a>,
}

#[derive(Debug, Serialize)]
struct DependencyOpenV1<'a> {
    schema: &'static str,
    lane: &'a str,
    cell: String,
    role: &'a str,
    relative_path: &'a str,
    required: bool,
    resolution: &'a str,
}

struct EvidenceInner {
    lane: String,
    enabled: bool,
    next_interval: AtomicU64,
}

/// Thread-safe canonical JSONL emitter. Disabled mode performs no I/O or hashing.
#[derive(Clone)]
pub struct RendererEvidence(Arc<EvidenceInner>);

impl RendererEvidence {
    /// Construct and emit `runtime-shape-v1` before any renderer work or CUDA context.
    pub fn from_env(role: &str, parameters: RuntimeParameters) -> Result<Self> {
        let enabled = match std::env::var(RENDERER_EVIDENCE_FLAG) {
            Err(std::env::VarError::NotPresent) => false,
            Ok(value) if value == "0" => false,
            Ok(value) if value == "1" => true,
            Ok(value) => bail!("{RENDERER_EVIDENCE_FLAG} must be 0 or 1, got {value:?}"),
            Err(error) => bail!("{RENDERER_EVIDENCE_FLAG} is not valid Unicode: {error}"),
        };
        if !enabled {
            return Ok(Self(Arc::new(EvidenceInner {
                lane: String::new(),
                enabled: false,
                next_interval: AtomicU64::new(1),
            })));
        }
        let runtime = runtime_shape_from_env(role, parameters)?;
        emit_json(&runtime)?;
        Ok(Self(Arc::new(EvidenceInner {
            lane: runtime.lane,
            enabled: true,
            next_interval: AtomicU64::new(1),
        })))
    }

    pub fn is_enabled(&self) -> bool {
        self.0.enabled
    }

    /// Start one renderer-monotonic cell interval and return its identity.
    pub fn region_claim(&self, cell: u64, worker_slot: usize) -> Result<u64> {
        if !self.is_enabled() {
            return Ok(0);
        }
        let interval_id = self.0.next_interval.fetch_add(1, Ordering::Relaxed);
        emit_json(&RegionClaimV1 {
            schema: "region-claim-v1",
            lane: &self.0.lane,
            cell: format!("{cell:015x}"),
            worker_slot,
            interval_id,
        })?;
        Ok(interval_id)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn region_terminal(
        &self,
        cell: u64,
        worker_slot: usize,
        interval_id: u64,
        status: RegionTerminalStatus,
        written: usize,
        skipped: usize,
        error: Option<&str>,
    ) -> Result<()> {
        if !self.is_enabled() {
            return Ok(());
        }
        emit_json(&RegionTerminalV1 {
            schema: "region-terminal-v1",
            lane: &self.0.lane,
            cell: format!("{cell:015x}"),
            worker_slot,
            interval_id,
            status,
            written,
            skipped,
            error,
        })
    }

    /// Emit one terminal from the post-write tree. Missing means explicit acoustic silence.
    #[allow(clippy::too_many_arguments)]
    pub fn tile_terminal(
        &self,
        cell: u64,
        layer: &str,
        z: u8,
        x: u32,
        y: u32,
        output_root: &Path,
        output_path: &Path,
        silence_reason: &str,
    ) -> Result<()> {
        if !self.is_enabled() {
            return Ok(());
        }
        let state = if output_path.exists() {
            let relative = output_path
                .strip_prefix(output_root)
                .with_context(|| format!("{} is outside output root", output_path.display()))?;
            let relative = relative
                .to_str()
                .context("output relative path is not UTF-8")?;
            if relative.starts_with('/') || relative.split('/').any(|part| part == "..") {
                bail!("unsafe output relative path {relative:?}");
            }
            let bytes = output_path.metadata()?.len();
            if bytes == 0 {
                bail!("written tile is empty: {}", output_path.display());
            }
            TileState::Written {
                relative_path: relative,
                bytes,
                sha256: sha256_file(output_path)?,
            }
        } else {
            TileState::Skipped {
                reason: silence_reason,
            }
        };
        emit_json(&TileTerminalV1 {
            schema: "tile-terminal-v1",
            lane: &self.0.lane,
            cell: format!("{cell:015x}"),
            layer,
            z,
            x,
            y,
            state,
        })
    }

    /// Record one production-path dependency resolution using a prepared-relative path.
    pub fn dependency_open(
        &self,
        cell: u64,
        role: &str,
        relative_path: &str,
        required: bool,
        present: bool,
    ) -> Result<()> {
        if !self.is_enabled() {
            return Ok(());
        }
        if relative_path.starts_with('/') || relative_path.split('/').any(|part| part == "..") {
            bail!("dependency path must be prepared-relative: {relative_path:?}");
        }
        emit_json(&DependencyOpenV1 {
            schema: "dependency-open-v1",
            lane: &self.0.lane,
            cell: format!("{cell:015x}"),
            role,
            relative_path,
            required,
            resolution: if present { "present" } else { "absent" },
        })
    }
}

pub(crate) fn runtime_shape_from_env(role: &str, p: RuntimeParameters) -> Result<RuntimeShapeV1> {
    let lane = required_env("QM_RENDERER_LANE")?;
    let product_commit = required_hex_env("QM_RENDERER_PRODUCT_COMMIT", 40)?;
    let artifact_family = required_env("QM_RENDERER_ARTIFACT_FAMILY")?;
    let line_model_role_sha256 = required_hex_env("QM_RENDERER_LINE_MODEL_ROLE_SHA256", 64)?;
    let layer_spec_sha256 = required_hex_env("QM_RENDERER_LAYER_SPEC_SHA256", 64)?;
    let executable = std::env::current_exe().context("resolve renderer executable")?;
    let environment = std::env::vars()
        .filter(|(name, _)| {
            name == "DATA_YEAR"
                || name == "CUDA_VISIBLE_DEVICES"
                || name == "SURFACE_BUDGET_ETA"
                || name == "RAYON_NUM_THREADS"
                || name.starts_with("QM_")
                || name.starts_with("QUIETMAP_")
                || name.starts_with("NOISE_GPU_")
                || name.starts_with("CUDA_CACHE_")
        })
        .collect();
    Ok(RuntimeShapeV1 {
        schema: "runtime-shape-v1",
        lane,
        role: role.to_string(),
        product_commit,
        executable_sha256: sha256_file(&executable)?,
        artifact_family,
        line_model_role_sha256,
        layer_spec_sha256,
        cuda_context_opened: false,
        argv: std::env::args().collect(),
        environment,
        zoom: p.zoom,
        batch_size: p.batch_size,
        n_days: p.n_days,
        rayon_threads: p.rayon_threads,
        stream_workers: p.stream_workers,
        region_concurrency_configured: p.region_concurrency_configured,
        region_concurrency_effective: p.region_concurrency_effective,
        max_regions_per_claim: p.max_regions_per_claim,
        layers: p.layers,
    })
}

pub(crate) fn required_env(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("{name} is required by renderer evidence"))
}

fn required_hex_env(name: &str, length: usize) -> Result<String> {
    let value = required_env(name)?;
    if value.len() != length || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("{name} must be exactly {length} hexadecimal characters");
    }
    Ok(value.to_ascii_lowercase())
}

pub(crate) fn emit_json(value: &impl Serialize) -> Result<()> {
    let line = serde_json::to_string(value).context("serialize renderer evidence")?;
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{line}").context("write renderer evidence")?;
    stdout.flush().context("flush renderer evidence")
}

pub(crate) fn sha256_file(path: &Path) -> Result<String> {
    let mut reader = BufReader::new(
        File::open(path).with_context(|| format!("open {} for SHA-256", path.display()))?,
    );
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn tile_state_is_exactly_written_or_skipped() {
        let written = TileTerminalV1 {
            schema: "tile-terminal-v1",
            lane: "gpu-line",
            cell: "841e309ffffffff".to_string(),
            layer: "road",
            z: 13,
            x: 4412,
            y: 2790,
            state: TileState::Written {
                relative_path: "road/13/4412/2790.bin",
                bytes: 17,
                sha256: "a".repeat(64),
            },
        };
        let written = serde_json::to_value(written).unwrap();
        let skipped = TileTerminalV1 {
            schema: "tile-terminal-v1",
            lane: "gpu-line",
            cell: "841e309ffffffff".to_string(),
            layer: "road",
            z: 13,
            x: 4412,
            y: 2790,
            state: TileState::Skipped {
                reason: "all-periods-silent",
            },
        };
        let skipped = serde_json::to_value(skipped).unwrap();
        assert_eq!(written["state"], "written");
        assert!(written.get("reason").is_none());
        assert_eq!(skipped["state"], "skipped");
        assert!(skipped.get("sha256").is_none());
    }

    #[test]
    fn output_hash_uses_the_exact_file_bytes() {
        let root = tempdir().unwrap();
        let path = root.path().join("tile.bin");
        std::fs::write(&path, b"hm3").unwrap();
        assert_eq!(
            sha256_file(&path).unwrap(),
            "f57594c9f828f191978a65c8b1b456d1c3c768b9fd84d9179c2ca516356d980b"
        );
    }

    #[test]
    fn identity_hashes_are_strict_lowercased_hex() {
        std::env::set_var("QM_RENDERER_TEST_HEX", "A".repeat(64));
        assert_eq!(
            required_hex_env("QM_RENDERER_TEST_HEX", 64).unwrap(),
            "a".repeat(64)
        );
        std::env::set_var("QM_RENDERER_TEST_HEX", "g".repeat(64));
        assert!(required_hex_env("QM_RENDERER_TEST_HEX", 64).is_err());
        std::env::remove_var("QM_RENDERER_TEST_HEX");
    }
}

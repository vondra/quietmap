//! Shared pmtiles manifest parsing — the "is this `file` reference safe, and what does
//! this manifest keep alive" logic used by BOTH `tile_store_pack.rs` (partial-pack merge
//! preflight) and `tile_store_gc.rs` (retention mark-and-sweep). ONE definition so the two
//! never drift on what counts as a safe archive filename (AGENTS.md: correctness-critical
//! logic lives in one place, not duplicated per binary).

use std::path::Path;

use anyhow::{bail, Context, Result};

/// A `file` value is safe iff it is exactly one path component — no `/`, no `..`, no
/// leading `.` tricks. The same invariant `tile_store_pack.rs` has enforced since its
/// partial-merge preflight; a manifest that fails this can never be trusted to name a
/// real, containable archive.
pub fn is_safe_archive_filename(file: &str) -> bool {
    let relative = Path::new(file);
    relative.components().count() == 1
        && relative.file_name().and_then(|name| name.to_str()) == Some(file)
}

/// Every `file` value recorded in a manifest's `layers` object. Fails closed on the WHOLE
/// manifest: a missing `layers` object, a non-object layer entry, a missing/non-string
/// `file`, or an unsafe file name each abort with an error rather than silently skipping
/// just the one bad entry — a caller that can't fully trust a manifest must not trust ANY
/// part of it (this is what makes `tile_store_gc.rs`'s fail-closed contract hold: any
/// unreadable/unparseable manifest aborts the entire GC run before it deletes anything).
/// `context` is folded into every error message (e.g. a file path) so a caller with
/// several manifest sources can tell which one failed.
pub fn manifest_files(manifest: &serde_json::Value, context: &str) -> Result<Vec<String>> {
    let layers = manifest
        .get("layers")
        .and_then(|value| value.as_object())
        .with_context(|| format!("{context}: no layers object"))?;
    let mut files = Vec::with_capacity(layers.len());
    for (layer, entry) in layers {
        let file = entry
            .get("file")
            .and_then(|value| value.as_str())
            .with_context(|| format!("{context}: layer {layer} has no file"))?;
        if !is_safe_archive_filename(file) {
            bail!("{context}: layer {layer} has unsafe file name {file:?}");
        }
        files.push(file.to_string());
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_filename_accepts_a_plain_name_and_rejects_traversal() {
        assert!(is_safe_archive_filename("road.b1.pmtiles"));
        assert!(!is_safe_archive_filename("../road.b1.pmtiles"));
        assert!(!is_safe_archive_filename("sub/road.b1.pmtiles"));
        assert!(!is_safe_archive_filename("/etc/passwd"));
    }

    #[test]
    fn manifest_files_extracts_every_layer_file() {
        let manifest = serde_json::json!({
            "build": "b1",
            "layers": {
                "road": {"file": "road.b1.pmtiles"},
                "rail": {"file": "rail.b1.pmtiles"},
            },
        });
        let mut files = manifest_files(&manifest, "test").unwrap();
        files.sort();
        assert_eq!(files, vec!["rail.b1.pmtiles", "road.b1.pmtiles"]);
    }

    #[test]
    fn manifest_files_rejects_an_unsafe_name() {
        let manifest = serde_json::json!({
            "layers": {"road": {"file": "../escape.pmtiles"}},
        });
        assert!(manifest_files(&manifest, "test").is_err());
    }

    #[test]
    fn manifest_files_rejects_a_missing_layers_object() {
        let manifest = serde_json::json!({"build": "b1"});
        assert!(manifest_files(&manifest, "test").is_err());
    }
}

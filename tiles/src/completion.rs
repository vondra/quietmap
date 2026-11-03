//! A complete tree carries the exact manifest used to build it; every reader verifies its digest.
use crate::{COMPLETION_MARKER, LAYOUT};
use sha2::{Digest, Sha256};
use std::path::Path;

pub fn manifest_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn mark_year_complete(root: &Path, manifest: &[u8], note: &str) -> Result<(), String> {
    let value: serde_json::Value = serde_json::from_slice(manifest).map_err(|e| e.to_string())?;
    let as_of = value["as_of"].as_str().ok_or("manifest needs as_of")?;
    let marker = serde_json::json!({"layout":LAYOUT,"manifest_sha256":manifest_hash(manifest),"as_of":as_of,"note":note});
    std::fs::write(root.join("year.json"), manifest).map_err(|e| e.to_string())?;
    let partial = root.join("complete.partial");
    std::fs::write(
        &partial,
        serde_json::to_vec(&marker).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    std::fs::rename(partial, root.join(COMPLETION_MARKER)).map_err(|e| e.to_string())
}

pub fn check_year_complete(root: &Path) -> Result<String, String> {
    let read = |name: &str| {
        std::fs::read(root.join(name))
            .map_err(|e| format!("{} is not a complete release: {name}: {e}", root.display()))
    };
    let marker: serde_json::Value = serde_json::from_slice(&read(COMPLETION_MARKER)?)
        .map_err(|e| format!("invalid year completion marker: {e}"))?;
    let bytes = read("year.json")?;
    let manifest: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("invalid completion manifest: {e}"))?;
    if marker["layout"] != LAYOUT
        || marker["manifest_sha256"] != manifest_hash(&bytes)
        || !manifest["as_of"].is_string()
        || marker["as_of"] != manifest["as_of"]
    {
        return Err(format!(
            "{}: completion layout, manifest hash or as_of mismatch",
            root.display()
        ));
    }
    manifest["year"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "completion manifest needs a year".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completion_binds_the_exact_manifest_and_cutoff() {
        let root = std::env::temp_dir().join(format!("qm-completion-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let manifest = br#"{"year":"2000","as_of":"2001-01-01"}"#;
        mark_year_complete(&root, manifest, "test").unwrap();
        check_year_complete(&root).unwrap();
        std::fs::write(
            root.join("year.json"),
            br#"{"year":"2000","as_of":"2001-01-02"}"#,
        )
        .unwrap();
        assert!(check_year_complete(&root).unwrap_err().contains("mismatch"));
        mark_year_complete(&root, manifest, "test").unwrap();
        let path = root.join(COMPLETION_MARKER);
        let mut marker: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        marker["as_of"] = serde_json::json!("2001-01-02");
        std::fs::write(path, serde_json::to_vec(&marker).unwrap()).unwrap();
        assert!(check_year_complete(&root).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}

//! A complete tree carries the exact manifest used to build it; every reader verifies its digest.
//! The marker's first line stays the layout, as earlier readers check only that line.
use crate::{COMPLETION_MARKER, LAYOUT};
use sha2::{Digest, Sha256};
use std::path::Path;

pub fn manifest_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn mark_year_complete(root: &Path, manifest: &[u8], note: &str) -> Result<(), String> {
    let value: serde_json::Value = serde_json::from_slice(manifest).map_err(|e| e.to_string())?;
    let as_of = value["as_of"].as_str().ok_or("manifest needs as_of")?;
    let binding =
        serde_json::json!({"manifest_sha256":manifest_hash(manifest),"as_of":as_of,"note":note});
    std::fs::write(root.join("year.json"), manifest).map_err(|e| e.to_string())?;
    let partial = root.join("complete.partial");
    std::fs::write(&partial, format!("{LAYOUT}\n{binding}")).map_err(|e| e.to_string())?;
    std::fs::rename(partial, root.join(COMPLETION_MARKER)).map_err(|e| e.to_string())
}

/// The verified marker and manifest of a complete year root.
fn verified_completion(root: &Path) -> Result<(serde_json::Value, serde_json::Value), String> {
    let read = |name: &str| {
        std::fs::read(root.join(name))
            .map_err(|e| format!("{} is not a complete release: {name}: {e}", root.display()))
    };
    let text = read(COMPLETION_MARKER)?;
    let binding = text
        .strip_prefix(LAYOUT.as_bytes())
        .and_then(|rest| rest.strip_prefix(b"\n"))
        .ok_or_else(|| format!("{} is not laid out as {LAYOUT}", root.display()))?;
    let marker: serde_json::Value = serde_json::from_slice(binding)
        .map_err(|e| format!("invalid year completion marker: {e}"))?;
    let bytes = read("year.json")?;
    let manifest: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("invalid completion manifest: {e}"))?;
    if marker["manifest_sha256"] != manifest_hash(&bytes)
        || !manifest["as_of"].is_string()
        || marker["as_of"] != manifest["as_of"]
    {
        return Err(format!(
            "{}: completion layout, manifest hash or as_of mismatch",
            root.display()
        ));
    }
    Ok((marker, manifest))
}

/// The year a complete root was built for.
pub fn check_year_complete(root: &Path) -> Result<String, String> {
    verified_completion(root)?.1["year"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "completion manifest needs a year".into())
}

/// The note its builder recorded in a complete root's marker.
pub fn year_completion_note(root: &Path) -> Result<String, String> {
    verified_completion(root)?.0["note"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{}: completion marker lacks its note", root.display()))
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
        let text = std::fs::read_to_string(&path).unwrap();
        let (layout, binding) = text.split_once('\n').unwrap();
        assert_eq!(layout, LAYOUT, "earlier readers check the first line");
        let mut marker: serde_json::Value = serde_json::from_str(binding).unwrap();
        marker["as_of"] = serde_json::json!("2001-01-02");
        std::fs::write(&path, format!("{LAYOUT}\n{marker}")).unwrap();
        assert!(check_year_complete(&root).is_err());
        std::fs::write(&path, binding).unwrap();
        assert!(check_year_complete(&root).unwrap_err().contains("laid out"));
        std::fs::remove_dir_all(root).unwrap();
    }
}

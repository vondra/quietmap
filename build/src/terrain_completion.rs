//! A downstream builder may read finished terrain before the other kinds complete the year.
use crate::{dev4::Square, year::YearInputs};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fs, path::Path};

const RECEIPT: &str = "terrain.json";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    layout: String,
    manifest_sha256: String,
    squares: Vec<(u32, u32, Option<u64>)>,
}

fn size(root: &Path, square: Square) -> Result<Option<u64>, String> {
    let path = tiles::square::square_path(root, tiles::Kind::Terrain, square);
    match fs::metadata(&path) {
        Ok(metadata) => Ok(Some(metadata.len())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

pub fn write(inputs: &YearInputs, root: &Path, squares: &[Square]) -> Result<(), String> {
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let receipt = Receipt {
        layout: tiles::LAYOUT.into(),
        manifest_sha256: tiles::manifest_hash(
            &fs::read(&inputs.manifest_path).map_err(|e| e.to_string())?,
        ),
        squares: squares
            .iter()
            .map(|&s| Ok((s.x, s.y, size(root, s)?)))
            .collect::<Result<_, String>>()?,
    };
    crate::pipeline_files::write_json(
        &root.join(RECEIPT),
        &serde_json::to_value(receipt).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

/// A partial tree proves ocean only inside its completed squares.
pub fn coverage(root: &Path) -> Result<Option<BTreeSet<Square>>, String> {
    let path = root.join(RECEIPT);
    if root.join(tiles::COMPLETION_MARKER).exists() || !path.exists() {
        return Ok(None);
    }
    let receipt: Receipt = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    Ok(Some(
        receipt
            .squares
            .into_iter()
            .map(|(x, y, _)| Square { x, y })
            .collect(),
    ))
}

pub fn check(inputs: &YearInputs, root: &Path, required: &[Square]) -> Result<(), String> {
    if root.join(tiles::COMPLETION_MARKER).exists() {
        return inputs.check_complete(root);
    }
    let path = root.join(RECEIPT);
    if !path.exists() {
        return inputs.check_complete(root);
    }
    let receipt: Receipt = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let expected =
        tiles::manifest_hash(&fs::read(&inputs.manifest_path).map_err(|e| e.to_string())?);
    if receipt.layout != tiles::LAYOUT || receipt.manifest_sha256 != expected {
        return Err("terrain completion differs from the selected year or layout".into());
    }
    let covered = unchanged_squares(root, receipt.squares)?;
    if required.iter().any(|square| !covered.contains(square)) {
        return Err("terrain completion does not cover the requested context".into());
    }
    Ok(())
}

/// The squares a receipt lists, each still holding the bytes it recorded.
fn unchanged_squares(
    root: &Path,
    squares: Vec<(u32, u32, Option<u64>)>,
) -> Result<BTreeSet<Square>, String> {
    let mut covered = BTreeSet::new();
    for (x, y, bytes) in squares {
        let square = Square { x, y };
        if !covered.insert(square) || size(root, square)? != bytes {
            return Err(format!("terrain completion changed at {x}:{y}"));
        }
    }
    Ok(covered)
}

/// A stopped run's migration of finished terrain to an edited manifest, reviewed to change nothing
/// the terrain reads (`flows`, a description): the receipt must name `from_sha256` or already the
/// selected manifest (a retry), every square it lists must still hold the bytes it recorded, and it
/// keeps its squares. The terrain step's completion record `completion` then takes the receipt's new
/// stamp, its other files unchanged, so the runner reuses the step instead of building it again.
pub fn rebind(
    inputs: &YearInputs,
    root: &Path,
    from_sha256: &str,
    completion: &Path,
) -> Result<(), String> {
    let path = root.join(RECEIPT);
    let mut receipt: Receipt =
        serde_json::from_slice(&fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?)
            .map_err(|e| e.to_string())?;
    let selected =
        tiles::manifest_hash(&fs::read(&inputs.manifest_path).map_err(|e| e.to_string())?);
    if receipt.layout != tiles::LAYOUT {
        return Err("terrain completion has another layout".into());
    }
    if receipt.manifest_sha256 != from_sha256 && receipt.manifest_sha256 != selected {
        return Err(format!(
            "terrain completion names manifest {}, neither {from_sha256} nor the selected {selected}",
            receipt.manifest_sha256
        ));
    }
    unchanged_squares(root, receipt.squares.clone())?;
    let error = |e: anyhow::Error| format!("{e:#}");
    let mut record: serde_json::Value = serde_json::from_slice(
        &fs::read(completion).map_err(|e| format!("{}: {e}", completion.display()))?,
    )
    .map_err(|e| e.to_string())?;
    let files = record["files"]
        .as_array_mut()
        .ok_or("the completion record lists no files")?;
    let mut receipt_rows = 0;
    for file in files.iter() {
        let listed = Path::new(file[0].as_str().ok_or("a completion row names no file")?);
        if listed == path {
            receipt_rows += 1;
        } else if crate::pipeline_files::stamp(listed).map_err(error)? != *file {
            return Err(format!("completed output changed: {}", listed.display()));
        }
    }
    if receipt_rows != 1 {
        return Err(format!(
            "the completion record lists {} {receipt_rows} times",
            path.display()
        ));
    }
    if receipt.manifest_sha256 != selected {
        receipt.manifest_sha256 = selected;
        crate::pipeline_files::write_json(
            &path,
            &serde_json::to_value(receipt).map_err(|e| e.to_string())?,
        )
        .map_err(error)?;
    }
    let stamp = crate::pipeline_files::stamp(&path).map_err(error)?;
    for file in files.iter_mut() {
        if Path::new(file[0].as_str().unwrap_or_default()) == path {
            *file = stamp.clone();
        }
    }
    crate::pipeline_files::write_json(completion, &record).map_err(error)
}

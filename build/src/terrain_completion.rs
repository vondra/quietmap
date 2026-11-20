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

/// Binds finished terrain to the selected manifest without building it again: for a manifest that
/// changed in nothing the terrain reads (its national models and the rasters), which the caller
/// checks and records with the run. The old receipt must cover `squares`, every square it lists
/// still holding the bytes it recorded.
pub fn rebind(inputs: &YearInputs, root: &Path, squares: &[Square]) -> Result<(), String> {
    let path = root.join(RECEIPT);
    let receipt: Receipt =
        serde_json::from_slice(&fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?)
            .map_err(|e| e.to_string())?;
    if receipt.layout != tiles::LAYOUT {
        return Err("terrain completion has another layout".into());
    }
    let covered = unchanged_squares(root, receipt.squares)?;
    if squares.iter().any(|square| !covered.contains(square)) {
        return Err("terrain completion does not cover the requested squares".into());
    }
    write(inputs, root, squares)
}

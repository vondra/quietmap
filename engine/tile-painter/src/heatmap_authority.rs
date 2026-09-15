//! Read completed owner artifacts from the existing generation authority, never directory presence.
use crate::{
    corner_directory::validate_owner_result,
    corner_store::CornerGeneration,
    edge_bundle,
    generation_receipt::{hex_digest, GenerationReceipt},
};
use anyhow::{ensure, Context, Result};
use grid::{Square, Z9_TILES_PER_AXIS};
use rusqlite::{Connection, OpenFlags};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

pub(crate) struct OwnerArtifact {
    pub owner: Square,
    pub path: PathBuf,
    pub sha256: String,
    pub bytes: u64,
}

pub(crate) fn completed_owners(
    authority: &Path,
    generation_hex: &str,
) -> Result<(CornerGeneration, Vec<OwnerArtifact>)> {
    ensure!(
        generation_hex.len() == 64
            && generation_hex
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
        "invalid generation digest"
    );
    let mut digest = [0; 32];
    for (i, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&generation_hex[2 * i..2 * i + 2], 16)?;
    }
    let generation = CornerGeneration(digest);
    let db = Connection::open_with_flags(authority, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    // One read snapshot: the generation state and its task set must agree.
    db.execute_batch("BEGIN")?;
    let state: String = db.query_row(
        "SELECT state FROM generations WHERE generation=?1",
        [generation_hex],
        |r| r.get(0),
    )?;
    ensure!(state == "complete", "generation is not complete");
    let pending: u64 = db.query_row(
        "SELECT count(*) FROM tasks WHERE generation=?1 AND state!='complete'",
        [generation_hex],
        |r| r.get(0),
    )?;
    ensure!(pending == 0, "generation has unfinished planned tasks");
    let mut query = db.prepare("SELECT owner_z9_x,owner_z9_y,copy_a_path,result_sha256,result_bytes FROM tasks WHERE generation=?1 AND phase='owner'")?;
    let mut rows = query.query([generation_hex])?;
    let mut owners = Vec::new();
    let mut seen = BTreeSet::new();
    let mut roots = BTreeSet::new();
    while let Some(row) = rows.next()? {
        let owner = Square {
            x: row.get(0)?,
            y: row.get(1)?,
        };
        ensure!(
            owner.x < Z9_TILES_PER_AXIS
                && owner.y < Z9_TILES_PER_AXIS
                && seen.insert((owner.x, owner.y)),
            "invalid or duplicate planned owner"
        );
        let artifact = OwnerArtifact {
            owner,
            path: PathBuf::from(row.get::<_, String>(2)?),
            sha256: row.get(3)?,
            bytes: row.get(4)?,
        };
        artifact.verify(generation)?;
        let (_, receipt) = edge_bundle::read_receipt(&artifact.path, generation, owner)?;
        let root = artifact
            .path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .context("invalid owner copy path")?;
        ensure!(
            artifact.path
                == root
                    .join("z9")
                    .join(owner.x.to_string())
                    .join(format!("{}.sqlite", owner.y)),
            "noncanonical owner copy path"
        );
        if roots.insert(root.to_path_buf()) {
            ensure!(
                GenerationReceipt::read_compatible(root, receipt.sources)? == Some(receipt),
                "generation.sqlite differs from acknowledged owner"
            );
        }
        owners.push(artifact);
    }
    ensure!(!owners.is_empty(), "generation has no planned owners");
    owners.sort_unstable_by_key(|item| {
        pmtiles::TileId::from(
            pmtiles::TileCoord::new(9, u32::from(item.owner.x), u32::from(item.owner.y))
                .expect("owner below z9 axis limit"),
        )
        .value()
    });
    Ok((generation, owners))
}

impl OwnerArtifact {
    pub fn verify(&self, generation: CornerGeneration) -> Result<()> {
        let digest = validate_owner_result(&self.path, generation, self.owner)?;
        ensure!(
            std::fs::metadata(&self.path)?.len() == self.bytes && hex_digest(digest) == self.sha256,
            "owner {} differs from authority receipt",
            self.path.display()
        );
        Ok(())
    }
}

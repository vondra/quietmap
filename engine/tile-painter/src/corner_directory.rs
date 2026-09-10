//! Independent z9 SQLite owners allow concurrent producers without a global lock.
use crate::{
    corner_store::{
        take_requested_values, CommittedSurfaceTile, CornerEnergy, CornerGeneration, CornerStore,
    },
    corner_totals::SurfacePeriodTotals,
    hm3::EncodedHm3,
};
use anyhow::{ensure, Context, Result};
use grid::{
    surface_corner::{tile_corners, SurfaceCorner, CORNER_COUNT},
    Square,
};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

pub struct CornerDirectory {
    root: PathBuf,
    pub(crate) generation: CornerGeneration,
}

fn output_owner(x: u32, y: u32) -> Result<Square> {
    let corner = SurfaceCorner::for_tile(x, y, 0, 0).context("invalid tile coordinates")?;
    Ok(corner.owner())
}

impl CornerDirectory {
    pub fn new(root: &Path, generation: CornerGeneration) -> Self {
        Self {
            root: root.to_path_buf(),
            generation,
        }
    }

    pub(crate) fn owner_path(&self, owner: Square) -> PathBuf {
        self.root
            .join("z9")
            .join(owner.x.to_string())
            .join(format!("{}.sqlite", owner.y))
    }

    /// Exact stored outdoor corner energies only; no interpolation accuracy is implied.
    pub fn read(&self, corner: SurfaceCorner) -> Result<Option<SurfacePeriodTotals>> {
        let owner = corner.owner();
        match CornerStore::open_read_only(&self.owner_path(owner), self.generation, owner)? {
            Some(store) => store.read(corner),
            None => Ok(None),
        }
    }

    pub fn committed(&self, x: u32, y: u32) -> Result<Option<CommittedSurfaceTile>> {
        let owner = output_owner(x, y)?;
        match CornerStore::open_read_only(&self.owner_path(owner), self.generation, owner)? {
            Some(store) => store.committed(x, y),
            None => Ok(None),
        }
    }

    pub fn write(&self, x: u32, y: u32, tiles: &[EncodedHm3]) -> Result<CommittedSurfaceTile> {
        let owner = output_owner(x, y)?;
        let path = self.owner_path(owner);
        let parent = path.parent().context("owner store has no parent")?;
        crate::durable_directory::create_dir_all(parent)?;
        CornerStore::open(&path, self.generation, owner)?.write(x, y, tiles)
    }

    /// Every owner derives this receipt's exact dependency vertices from its tile coordinates.
    pub fn release(&self, receipt: CommittedSurfaceTile) -> Result<()> {
        ensure!(
            receipt.generation() == self.generation,
            "tile receipt generation mismatch"
        );
        let [x, y] = receipt.coordinates();
        let vertices = tile_corners(x, y).context("invalid tile receipt coordinates")?;
        let mut owners = BTreeMap::new();
        for corner in vertices {
            let owner = corner.owner();
            owners.entry((owner.x, owner.y)).or_insert(owner);
        }
        for owner in owners.into_values() {
            let path = self.owner_path(owner);
            ensure!(
                path.try_exists()?,
                "committed tile lost its canonical corner store"
            );
            CornerStore::open(&path, self.generation, owner)?.release(receipt)?;
        }
        Ok(())
    }

    /// Publish one complete five-layer z9 result without exposing a partial file.
    pub fn publish_owner_result(&self, owner: Square, destination: &Path) -> Result<[u8; 32]> {
        let source = self.owner_path(owner);
        let source_digest = validate_owner_result(&source, self.generation, owner)?;
        let parent = destination.parent().context("owner result has no parent")?;
        crate::durable_directory::create_dir_all(parent)?;
        let source_before = source.metadata()?;
        if destination.try_exists()? {
            ensure!(
                destination.is_file()
                    && crate::generation_receipt::file_digest(destination)? == source_digest,
                "refusing to overwrite a different owner result"
            );
            return Ok(source_digest);
        }

        let temporary = destination.with_extension(format!("part-{}", std::process::id()));
        let result = (|| -> Result<()> {
            let mut input = File::open(&source)?;
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            let mut buffer = [0_u8; 1024 * 1024];
            loop {
                let count = input.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                output.write_all(&buffer[..count])?;
            }
            output.sync_all()?;
            let source_after_copy = source.metadata()?;
            ensure!(
                same_file_identity(&source_before, &source_after_copy),
                "owner result changed while copying"
            );
            ensure!(
                crate::generation_receipt::file_digest(&temporary)? == source_digest,
                "owner result copy digest mismatch"
            );
            match std::fs::hard_link(&temporary, destination) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => ensure!(
                    destination.is_file()
                        && crate::generation_receipt::file_digest(destination)? == source_digest,
                    "concurrent owner result differs"
                ),
                Err(error) => return Err(error.into()),
            }
            crate::durable_directory::sync_directory(parent)?;
            Ok(())
        })();
        if temporary.try_exists()? {
            std::fs::remove_file(&temporary)?;
        }
        result?;
        Ok(source_digest)
    }

    /// Each owner's reservation covers at most one tile's missing vertices.
    /// Different owners never hold the same SQLite write lock. No transaction
    /// holds one owner lock while acquiring another, so neighboring workers cannot deadlock.
    pub fn resolve<F>(
        &self,
        requested: &[SurfaceCorner],
        mut evaluate: F,
    ) -> Result<Vec<CornerEnergy>>
    where
        F: FnMut(Square, &[SurfaceCorner]) -> Result<Vec<CornerEnergy>>,
    {
        ensure!(
            requested.len() <= CORNER_COUNT,
            "corner request exceeds one tile batch"
        );
        let mut owners: BTreeMap<(u16, u16), Vec<SurfaceCorner>> = BTreeMap::new();
        for &corner in requested {
            let owner = corner.owner();
            owners.entry((owner.x, owner.y)).or_default().push(corner);
        }
        let mut values = BTreeMap::new();
        for ((x, y), vertices) in owners {
            let owner = Square { x, y };
            let path = self.owner_path(owner);
            let parent = path.parent().context("owner store has no parent")?;
            crate::durable_directory::create_dir_all(parent)?;
            let mut store = CornerStore::open(&path, self.generation, owner)?;
            let energy = store.resolve(&vertices, |canonical_owner, missing| {
                ensure!(
                    canonical_owner == owner,
                    "corner dispatched to wrong SQLite owner"
                );
                evaluate(canonical_owner, missing)
            })?;
            values.extend(vertices.into_iter().zip(energy));
        }
        Ok(take_requested_values(values, requested))
    }
}

pub fn validate_owner_result(
    path: &Path,
    generation: CornerGeneration,
    owner: Square,
) -> Result<[u8; 32]> {
    let before = path.metadata()?;
    let store = CornerStore::open_read_only(path, generation, owner)?
        .context("owner result store does not exist")?;
    let ((x0, x1), (y0, y1)) = grid::owned_z13(owner);
    for y in y0..y1 {
        for x in x0..x1 {
            ensure!(
                store.committed(x, y)?.is_some(),
                "owner result lacks a five-layer tile"
            );
        }
    }
    let tile_rows: u64 =
        store
            .connection
            .query_row("SELECT count(*) FROM surface_tiles", [], |row| row.get(0))?;
    ensure!(
        tile_rows == 16 * 16 * 5,
        "owner result has extra surface tiles"
    );
    drop(store);
    let digest = crate::generation_receipt::file_digest(path)?;
    let after = path.metadata()?;
    ensure!(
        same_file_identity(&before, &after),
        "owner result changed while validating"
    );
    Ok(digest)
}

fn same_file_identity(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
}

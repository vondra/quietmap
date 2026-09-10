//! One authoritative z9 transaction for canonical corners and five-layer HM3 tiles.
use crate::{
    corner_codec::SourceDictionary,
    corner_totals::SurfacePeriodTotals,
    hm3::{EncodedHm3, SURFACE_SOURCE_IDS},
};
use anyhow::{bail, ensure, Context, Result};
use grid::{
    surface_corner::{tile_corners, SurfaceCorner, CORNER_COUNT},
    Square, Z13_PER_Z9_SIDE,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CornerGeneration(pub [u8; 32]);
impl CornerGeneration {
    /// Rasters carry no pin of their own: their identity is the release name and the code.
    pub fn from_manifests(sources: [u8; 32], code: [u8; 32], producer: [u8; 32]) -> Self {
        let mut hash = Sha256::new();
        hash.update(b"surface-corners-z18-v4");
        for digest in [sources, code, producer] {
            hash.update(digest);
        }
        Self(hash.finalize().into())
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SourceIdentity(pub [u8; 32]);
impl SourceIdentity {
    pub fn arrow_row(arrow_sha256: [u8; 32], row: u64, part: u32) -> Self {
        let mut hash = Sha256::new();
        hash.update(b"surface-source-v1");
        hash.update(arrow_sha256);
        hash.update(row.to_le_bytes());
        hash.update(part.to_le_bytes());
        Self(hash.finalize().into())
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct SourceEnergy {
    /// Road, rail, industrial, building, ground operations, in that order.
    pub layer: u8,
    pub source: SourceIdentity,
    pub periods: [f32; 3],
}
/// Sorted complete candidate set, retained only while dependent tiles need it.
#[derive(Clone, Debug, PartialEq)]
pub struct CornerEnergy(pub Vec<SourceEnergy>);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommittedSurfaceTile {
    x: u32,
    y: u32,
    generation: CornerGeneration,
    layer_sha256: [[u8; 32]; 5],
}
impl CommittedSurfaceTile {
    pub fn coordinates(self) -> [u32; 2] {
        [self.x, self.y]
    }
    pub fn layer_sha256(self) -> [[u8; 32]; 5] {
        self.layer_sha256
    }
    pub(crate) fn generation(self) -> CornerGeneration {
        self.generation
    }
    fn owner(self) -> Square {
        Square {
            x: (self.x / Z13_PER_Z9_SIDE) as u16,
            y: (self.y / Z13_PER_Z9_SIDE) as u16,
        }
    }
}

pub struct CornerStore {
    pub(crate) connection: Connection,
    pub(crate) generation: CornerGeneration,
    pub(crate) owner: Square,
    pub(crate) path: PathBuf,
}
impl CornerStore {
    pub fn open_read_only(
        path: &Path,
        generation: CornerGeneration,
        owner: Square,
    ) -> Result<Option<Self>> {
        if !path.try_exists()? {
            return Ok(None);
        }
        let connection =
            Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        connection.busy_timeout(Duration::from_secs(1))?;
        verify_generation(&connection, generation, owner)?;
        Ok(Some(Self {
            connection,
            generation,
            owner,
            path: path.to_path_buf(),
        }))
    }
    pub fn open(path: &Path, generation: CornerGeneration, owner: Square) -> Result<Self> {
        let mut connection = Connection::open(path)?;
        connection.busy_handler(Some(|_| {
            // A canonical producer may still be loading its scene; waiting never duplicates its GPU work.
            std::thread::sleep(Duration::from_millis(100));
            true
        }))?;
        connection.execute_batch("PRAGMA synchronous=FULL; PRAGMA auto_vacuum=INCREMENTAL;")?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS generation(id INTEGER PRIMARY KEY CHECK(id=1),digest BLOB NOT NULL CHECK(length(digest)=32),owner_x INTEGER NOT NULL,owner_y INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS sources(id INTEGER PRIMARY KEY CHECK(id BETWEEN 0 AND 4294967295),layer INTEGER NOT NULL CHECK(layer BETWEEN 0 AND 4),identity BLOB NOT NULL CHECK(length(identity)=32),UNIQUE(layer,identity));
            CREATE TABLE IF NOT EXISTS corners(x INTEGER NOT NULL,y INTEGER NOT NULL,totals BLOB NOT NULL CHECK(length(totals)=60),remaining INTEGER NOT NULL CHECK(remaining BETWEEN 0 AND 15),PRIMARY KEY(x,y)) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS staging(x INTEGER NOT NULL,y INTEGER NOT NULL,energy BLOB NOT NULL,PRIMARY KEY(x,y)) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS surface_tiles(x INTEGER NOT NULL,y INTEGER NOT NULL,layer INTEGER NOT NULL CHECK(layer BETWEEN 0 AND 4),hm3 BLOB NOT NULL,sha256 BLOB NOT NULL CHECK(length(sha256)=32),PRIMARY KEY(x,y,layer)) WITHOUT ROWID;")?;
        tx.execute(
            "INSERT OR IGNORE INTO generation VALUES(1,?1,?2,?3)",
            params![generation.0.as_slice(), owner.x, owner.y],
        )?;
        verify_generation(&tx, generation, owner)?;
        tx.commit()?;
        crate::durable_directory::sync_directory(
            path.parent().context("corner store has no parent")?,
        )?;
        Ok(Self {
            connection,
            generation,
            owner,
            path: path.to_path_buf(),
        })
    }
    /// One owner reservation spans evaluation: competitors consume the committed bytes.
    /// A crash before this transaction commits may repeat work; committed staging is reused.
    pub fn resolve<F>(
        &mut self,
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
        ensure!(
            requested.iter().all(|corner| corner.owner() == self.owner),
            "corner dispatched to wrong owner store"
        );
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut dictionary = SourceDictionary::load(&tx)?;
        let mut resolved = BTreeMap::new();
        let mut missing = Vec::new();
        for &corner in requested {
            if resolved.contains_key(&corner) {
                continue;
            }
            let [x, y] = corner.coordinates();
            let stored: Option<(u8, Option<Vec<u8>>)> = tx
                .query_row(
                    "SELECT c.remaining,s.energy FROM corners c LEFT JOIN staging s USING(x,y) WHERE c.x=?1 AND c.y=?2",
                    params![x, y],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let value = match stored {
                Some((remaining, Some(bytes))) => {
                    ensure!(remaining != 0, "released corner still has source staging");
                    Some(dictionary.decode(&bytes)?)
                }
                Some((0, None)) => {
                    bail!("corner dependencies already completed; reuse committed tile output")
                }
                Some((_, None)) => bail!("incomplete corner dependencies lost source staging"),
                None => {
                    missing.push(corner);
                    None
                }
            };
            resolved.insert(corner, value);
        }
        if !missing.is_empty() {
            let values =
                evaluate(self.owner, &missing).context("canonical owner corner evaluation")?;
            ensure!(
                values.len() == missing.len(),
                "corner producer returned wrong result count"
            );
            for (corner, energy) in missing.into_iter().zip(values) {
                let [x, y] = corner.coordinates();
                let bytes = dictionary.encode(&tx, &energy)?;
                let totals = SurfacePeriodTotals::from_sources(&energy)?.encode()?;
                tx.execute(
                    "INSERT INTO corners VALUES(?1,?2,?3,?4)",
                    params![x, y, totals, corner.dependency_mask()],
                )?;
                tx.execute("INSERT INTO staging VALUES(?1,?2,?3)", params![x, y, bytes])?;
                resolved.insert(corner, Some(energy));
            }
        }
        let result = take_requested_values(resolved, requested)
            .into_iter()
            .map(|value| value.expect("produced all missing corners"))
            .collect();
        tx.commit()?;
        Ok(result)
    }
    pub fn read(&self, corner: SurfaceCorner) -> Result<Option<SurfacePeriodTotals>> {
        ensure!(
            corner.owner() == self.owner,
            "corner read from wrong owner store"
        );
        let [x, y] = corner.coordinates();
        let bytes: Option<Vec<u8>> = self
            .connection
            .query_row(
                "SELECT totals FROM corners WHERE x=?1 AND y=?2",
                params![x, y],
                |row| row.get(0),
            )
            .optional()?;
        bytes
            .as_deref()
            .map(SurfacePeriodTotals::decode)
            .transpose()
    }
    pub fn committed(&self, x: u32, y: u32) -> Result<Option<CommittedSurfaceTile>> {
        read_committed(&self.connection, self.generation, self.owner, x, y)
    }
    /// HM3 bytes and this owner's dependency release commit in one transaction.
    pub fn write(&mut self, x: u32, y: u32, tiles: &[EncodedHm3]) -> Result<CommittedSurfaceTile> {
        ensure_owner_tile(self.owner, x, y)?;
        ensure!(
            tiles.len() == 5
                && tiles
                    .iter()
                    .zip(SURFACE_SOURCE_IDS)
                    .all(|(tile, id)| tile.source_id == id),
            "incomplete tile or wrong layer order"
        );
        let expected_hashes: [[u8; 32]; 5] =
            std::array::from_fn(|layer| Sha256::digest(&tiles[layer].bytes).into());
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        match read_committed(&tx, self.generation, self.owner, x, y)? {
            Some(receipt) => ensure!(
                receipt.layer_sha256 == expected_hashes,
                "committed surface tile is immutable"
            ),
            None => {
                for (layer, tile) in tiles.iter().enumerate() {
                    tx.execute(
                        "INSERT INTO surface_tiles VALUES(?1,?2,?3,?4,?5)",
                        params![x, y, layer, tile.bytes, expected_hashes[layer].as_slice()],
                    )?;
                }
            }
        }
        let receipt = CommittedSurfaceTile {
            x,
            y,
            generation: self.generation,
            layer_sha256: expected_hashes,
        };
        release_tile_corners(&tx, self.owner, receipt)?;
        tx.commit()?;
        Ok(receipt)
    }
    /// A committed receipt derives its exact canonical dependency set from tile coordinates.
    pub fn release(&mut self, receipt: CommittedSurfaceTile) -> Result<()> {
        ensure!(
            receipt.generation == self.generation,
            "tile receipt generation mismatch"
        );
        if receipt.owner() == self.owner {
            ensure!(
                self.committed(receipt.x, receipt.y)? == Some(receipt),
                "tile receipt does not match committed HM3 bytes"
            );
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        release_tile_corners(&tx, self.owner, receipt)?;
        tx.commit()?;
        Ok(())
    }
}

fn ensure_owner_tile(owner: Square, x: u32, y: u32) -> Result<()> {
    ensure!(
        x / Z13_PER_Z9_SIDE == u32::from(owner.x)
            && y / Z13_PER_Z9_SIDE == u32::from(owner.y)
            && tile_corners(x, y).is_some(),
        "tile outside output owner"
    );
    Ok(())
}

fn read_committed(
    connection: &Connection,
    generation: CornerGeneration,
    owner: Square,
    x: u32,
    y: u32,
) -> Result<Option<CommittedSurfaceTile>> {
    ensure_owner_tile(owner, x, y)?;
    let mut statement = connection
        .prepare("SELECT layer,hm3,sha256 FROM surface_tiles WHERE x=?1 AND y=?2 ORDER BY layer")?;
    let rows = statement
        .query_map(params![x, y], |row| {
            Ok((
                row.get::<_, u8>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.is_empty() {
        return Ok(None);
    }
    ensure!(rows.len() == 5, "incomplete published surface tile");
    let mut hashes = [[0_u8; 32]; 5];
    for (expected_layer, (layer, hm3, digest)) in rows.into_iter().enumerate() {
        ensure!(
            layer as usize == expected_layer,
            "invalid surface tile layer set"
        );
        let digest: [u8; 32] = digest
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid surface tile digest"))?;
        ensure!(
            <[u8; 32]>::from(Sha256::digest(&hm3)) == digest,
            "surface tile digest mismatch"
        );
        hashes[expected_layer] = digest;
    }
    Ok(Some(CommittedSurfaceTile {
        x,
        y,
        generation,
        layer_sha256: hashes,
    }))
}

fn release_tile_corners(
    tx: &Transaction<'_>,
    owner: Square,
    receipt: CommittedSurfaceTile,
) -> Result<()> {
    let vertices =
        tile_corners(receipt.x, receipt.y).context("invalid tile receipt coordinates")?;
    let mut matched = false;
    for corner in vertices
        .into_iter()
        .filter(|corner| corner.owner() == owner)
    {
        matched = true;
        let bit = corner
            .consumer_bit(receipt.x, receipt.y)
            .context("tile does not depend on vertex")?;
        let [x, y] = corner.coordinates();
        let remaining: u8 = tx.query_row(
            "SELECT remaining FROM corners WHERE x=?1 AND y=?2",
            params![x, y],
            |row| row.get(0),
        )?;
        let staging_exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM staging WHERE x=?1 AND y=?2)",
            params![x, y],
            |row| row.get(0),
        )?;
        ensure!(
            (remaining != 0) == staging_exists,
            "corner staging disagrees with pending dependencies"
        );
        let pending = remaining & !bit;
        tx.execute(
            "UPDATE corners SET remaining=?3 WHERE x=?1 AND y=?2",
            params![x, y, pending],
        )?;
        if pending == 0 {
            tx.execute("DELETE FROM staging WHERE x=?1 AND y=?2", params![x, y])?;
        }
    }
    ensure!(matched, "tile has no dependency in owner store");
    tx.execute(
        "DELETE FROM sources WHERE NOT EXISTS(SELECT 1 FROM staging)",
        [],
    )?;
    let empty: bool = tx.query_row("SELECT NOT EXISTS(SELECT 1 FROM staging)", [], |row| {
        row.get(0)
    })?;
    if empty {
        let mut vacuum = tx.prepare("PRAGMA incremental_vacuum")?;
        let mut pages = vacuum.query([])?;
        while pages.next()?.is_some() {}
    }
    Ok(())
}

/// Ordinary tile requests move their large source vectors; only repeated vertices need a copy.
pub(crate) fn take_requested_values<T: Clone>(
    mut values: BTreeMap<SurfaceCorner, T>,
    requested: &[SurfaceCorner],
) -> Vec<T> {
    let mut result: Vec<T> = Vec::with_capacity(requested.len());
    for (index, corner) in requested.iter().enumerate() {
        let value = values.remove(corner).unwrap_or_else(|| {
            let previous = requested[..index]
                .iter()
                .position(|value| value == corner)
                .expect("every requested vertex was resolved");
            result[previous].clone()
        });
        result.push(value);
    }
    result
}

pub(crate) fn verify_generation(
    connection: &Connection,
    generation: CornerGeneration,
    owner: Square,
) -> Result<()> {
    let (digest, owner_x, owner_y): (Vec<u8>, u16, u16) = connection.query_row(
        "SELECT digest,owner_x,owner_y FROM generation WHERE id=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    ensure!(
        digest == generation.0 && owner_x == owner.x && owner_y == owner.y,
        "corner store belongs to another immutable generation or owner"
    );
    Ok(())
}
#[cfg(test)]
mod tests;

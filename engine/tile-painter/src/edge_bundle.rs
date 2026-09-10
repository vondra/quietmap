//! Generation-bound SQLite edge bundles import through the existing owner transaction.
use crate::{
    corner_codec::SourceDictionary,
    corner_directory::CornerDirectory,
    corner_store::{CornerEnergy, CornerGeneration, CornerStore},
    corner_totals::SurfacePeriodTotals,
    durable_directory,
    generation_receipt::{GenerationReceipt, SURFACE_CODE_DIGEST},
};
use anyhow::{ensure, Context, Result};
use grid::{surface_corner::owner_edge_corners, Square};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use std::{collections::BTreeMap, fs::File, path::Path};

pub struct EdgeBundle {
    pub owner: Square,
    pub epoch: u64,
    pub receipt: GenerationReceipt,
    pub values: Vec<CornerEnergy>,
}

pub fn seal(store: &mut CornerStore, epoch: u64, receipt: GenerationReceipt) -> Result<()> {
    ensure!(
        receipt.generation() == store.generation && receipt.code == SURFACE_CODE_DIGEST,
        "edge bundle receipt differs from its corner generation"
    );
    let expected = owner_edge_corners(store.owner);
    let actual = stored_coordinates(&store.connection)?;
    ensure!(
        actual == expected,
        "edge bundle has missing or extra canonical vertices"
    );
    let staged: u64 = store
        .connection
        .query_row("SELECT count(*) FROM staging", [], |row| row.get(0))?;
    let tiles: u64 =
        store
            .connection
            .query_row("SELECT count(*) FROM surface_tiles", [], |row| row.get(0))?;
    ensure!(
        staged as usize == expected.len(),
        "edge bundle lost source staging"
    );
    ensure!(tiles == 0, "edge bundle contains painted tiles");
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS edge_bundle(
        id INTEGER PRIMARY KEY CHECK(id=1),epoch INTEGER NOT NULL CHECK(epoch>=0),
        sources BLOB NOT NULL,rasters BLOB NOT NULL,code BLOB NOT NULL,producer BLOB NOT NULL);",
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO edge_bundle VALUES(1,?1,?2,?3,?4,?5)",
        rusqlite::params![
            epoch,
            receipt.sources.as_slice(),
            receipt.rasters.as_slice(),
            receipt.code.as_slice(),
            receipt.producer.as_slice()
        ],
    )?;
    let stored: (u64, Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) = tx.query_row(
        "SELECT epoch,sources,rasters,code,producer FROM edge_bundle WHERE id=1",
        [],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )?;
    ensure!(
        stored.0 == epoch
            && stored.1 == receipt.sources
            && stored.2 == receipt.rasters
            && stored.3 == receipt.code
            && stored.4 == receipt.producer,
        "edge bundle belongs to another coordinator epoch or generation receipt"
    );
    tx.commit()?;
    File::open(&store.path)?.sync_all()?;
    durable_directory::sync_directory(store.path.parent().context("edge bundle has no parent")?)?;
    Ok(())
}

pub fn read_receipt(
    path: &Path,
    generation: CornerGeneration,
    owner: Square,
) -> Result<(u64, GenerationReceipt)> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("open edge receipt {}", path.display()))?;
    super::corner_store::verify_generation(&connection, generation, owner)?;
    let stored: (u64, Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) = connection.query_row(
        "SELECT epoch,sources,rasters,code,producer FROM edge_bundle WHERE id=1",
        [],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )?;
    let receipt = GenerationReceipt {
        sources: stored
            .1
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid source digest"))?,
        rasters: stored
            .2
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid raster digest"))?,
        code: stored
            .3
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid code digest"))?,
        producer: stored
            .4
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid producer digest"))?,
    };
    ensure!(
        receipt.generation() == generation && receipt.code == SURFACE_CODE_DIGEST,
        "edge bundle generation receipt mismatch"
    );
    Ok((stored.0, receipt))
}

pub fn read(path: &Path, generation: CornerGeneration) -> Result<EdgeBundle> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("open edge bundle {}", path.display()))?;
    let (owner_x, owner_y): (u16, u16) = connection.query_row(
        "SELECT owner_x,owner_y FROM generation WHERE id=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let owner = Square {
        x: owner_x,
        y: owner_y,
    };
    super::corner_store::verify_generation(&connection, generation, owner)?;
    let stored: (u64, Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) = connection.query_row(
        "SELECT epoch,sources,rasters,code,producer FROM edge_bundle WHERE id=1",
        [],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )?;
    let receipt = GenerationReceipt {
        sources: stored
            .1
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid source digest"))?,
        rasters: stored
            .2
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid raster digest"))?,
        code: stored
            .3
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid code digest"))?,
        producer: stored
            .4
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid producer digest"))?,
    };
    ensure!(
        receipt.generation() == generation && receipt.code == SURFACE_CODE_DIGEST,
        "edge bundle generation receipt mismatch"
    );
    let epoch = stored.0;
    let expected = owner_edge_corners(owner);
    ensure!(
        stored_coordinates(&connection)? == expected,
        "edge bundle vertex set is incomplete"
    );
    let dictionary = SourceDictionary::load(&connection)?;
    let mut statement = connection.prepare(
        "SELECT c.x,c.y,c.totals,c.remaining,s.energy FROM corners c
         LEFT JOIN staging s USING(x,y) ORDER BY c.x,c.y",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, u32>(0)?,
            row.get::<_, u32>(1)?,
            row.get::<_, Vec<u8>>(2)?,
            row.get::<_, u8>(3)?,
            row.get::<_, Option<Vec<u8>>>(4)?,
        ))
    })?;
    let mut values = Vec::with_capacity(expected.len());
    for (expected_corner, row) in expected.iter().zip(rows) {
        let (x, y, totals, remaining, energy) = row?;
        ensure!(
            expected_corner.coordinates() == [x, y],
            "edge bundle vertex order changed"
        );
        ensure!(
            remaining == expected_corner.dependency_mask(),
            "edge dependency mask changed"
        );
        let value = dictionary.decode(&energy.context("edge bundle lost source energy")?)?;
        ensure!(
            SurfacePeriodTotals::from_sources(&value)?.encode()? == totals,
            "edge totals disagree with source energy"
        );
        values.push(value);
    }
    ensure!(
        values.len() == expected.len(),
        "edge bundle vertex count changed"
    );
    Ok(EdgeBundle {
        owner,
        epoch,
        receipt,
        values,
    })
}

fn stored_coordinates(connection: &Connection) -> Result<Vec<grid::surface_corner::SurfaceCorner>> {
    let mut statement = connection.prepare("SELECT x,y FROM corners ORDER BY x,y")?;
    let coordinates = statement
        .query_map([], |row| Ok((row.get::<_, u32>(0)?, row.get::<_, u32>(1)?)))?
        .map(|row| {
            let (x, y) = row?;
            grid::surface_corner::SurfaceCorner::new(x, y).context("invalid edge coordinate")
        })
        .collect();
    coordinates
}

impl CornerDirectory {
    pub fn import_edge_bundle(&self, path: &Path) -> Result<(Square, u64)> {
        let bundle = read(path, self.generation)?;
        let vertices = owner_edge_corners(bundle.owner);
        let owner_path = self.owner_path(bundle.owner);
        durable_directory::create_dir_all(
            owner_path
                .parent()
                .context("edge owner store has no parent")?,
        )?;
        let mut store = CornerStore::open(&owner_path, self.generation, bundle.owner)?;
        let receipt_exists: bool = store.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='edge_bundle')",
            [],
            |row| row.get(0),
        )?;
        if receipt_exists {
            let (stored_epoch, stored_receipt) =
                read_receipt(&owner_path, self.generation, bundle.owner)?;
            ensure!(
                (stored_epoch, stored_receipt) == (bundle.epoch, bundle.receipt),
                "imported edge receipt differs from its durable values"
            );
        }
        for (corners, expected) in vertices
            .chunks(grid::surface_corner::CORNER_COUNT)
            .zip(bundle.values.chunks(grid::surface_corner::CORNER_COUNT))
        {
            let mut unresolved = Vec::new();
            let mut expected_by_corner = BTreeMap::new();
            for (&corner, value) in corners.iter().zip(expected) {
                let [x, y] = corner.coordinates();
                let stored: Option<(u8, Vec<u8>, Option<Vec<u8>>)> = store
                    .connection
                    .query_row(
                        "SELECT c.remaining,c.totals,s.energy FROM corners c                          LEFT JOIN staging s USING(x,y) WHERE c.x=?1 AND c.y=?2",
                        rusqlite::params![x, y],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .optional()?;
                match stored {
                    Some((0, totals, None)) => ensure!(
                        totals == SurfacePeriodTotals::from_sources(value)?.encode()?,
                        "released edge totals differ from imported bundle"
                    ),
                    Some((0, _, Some(_))) | Some((1.., _, None)) => {
                        anyhow::bail!("edge staging disagrees with dependencies")
                    }
                    _ => {
                        unresolved.push(corner);
                        expected_by_corner.insert(corner, value.clone());
                    }
                }
            }
            let imported = store.resolve(&unresolved, |owner, missing| {
                ensure!(owner == bundle.owner, "edge bundle owner changed");
                missing
                    .iter()
                    .map(|corner| {
                        expected_by_corner
                            .get(corner)
                            .cloned()
                            .context("edge bundle omitted requested vertex")
                    })
                    .collect()
            })?;
            let expected_unresolved: Vec<_> = unresolved
                .iter()
                .map(|corner| expected_by_corner[corner].clone())
                .collect();
            ensure!(
                imported == expected_unresolved,
                "existing edge bytes differ from imported bundle"
            );
        }
        if !receipt_exists {
            seal(&mut store, bundle.epoch, bundle.receipt)?;
        }
        drop(store);
        let (stored_epoch, stored_receipt) =
            read_receipt(&owner_path, self.generation, bundle.owner)?;
        ensure!(
            (stored_epoch, stored_receipt) == (bundle.epoch, bundle.receipt),
            "imported edge receipt differs from its durable values"
        );
        Ok((bundle.owner, bundle.epoch))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corner_store::{SourceEnergy, SourceIdentity};

    fn value() -> CornerEnergy {
        CornerEnergy(vec![SourceEnergy {
            layer: 1,
            source: SourceIdentity::arrow_row([7; 32], 4, 0),
            periods: [1.0, 2.0, 3.0],
        }])
    }

    #[test]
    fn sealed_edge_roundtrips_and_imports_through_owner_transaction() {
        let temporary = tempfile::tempdir().unwrap();
        let bundle_path = temporary.path().join("edge.sqlite");
        let owner = Square { x: 276, y: 173 };
        let receipt = GenerationReceipt {
            sources: [1; 32],
            rasters: [2; 32],
            code: SURFACE_CODE_DIGEST,
            producer: [4; 32],
        };
        let generation = receipt.generation();
        let vertices = owner_edge_corners(owner);
        let mut store = CornerStore::open(&bundle_path, generation, owner).unwrap();
        for corners in vertices.chunks(grid::surface_corner::CORNER_COUNT) {
            store
                .resolve(corners, |_, missing| Ok(vec![value(); missing.len()]))
                .unwrap();
        }
        seal(&mut store, 17, receipt).unwrap();
        drop(store);
        let decoded = read(&bundle_path, generation).unwrap();
        assert_eq!(
            (decoded.owner, decoded.epoch, decoded.values.len()),
            (owner, 17, 1023)
        );
        let directory = CornerDirectory::new(&temporary.path().join("import"), generation);
        assert_eq!(
            directory.import_edge_bundle(&bundle_path).unwrap(),
            (owner, 17)
        );
        assert_eq!(
            directory.import_edge_bundle(&bundle_path).unwrap(),
            (owner, 17)
        );
        let imported_path = directory.owner_path(owner);
        assert_eq!(
            read_receipt(&imported_path, generation, owner).unwrap(),
            (17, receipt)
        );
        let imported = Connection::open(&imported_path).unwrap();
        let [x, y] = vertices[0].coordinates();
        imported
            .execute(
                "UPDATE corners SET remaining=0 WHERE x=?1 AND y=?2",
                rusqlite::params![x, y],
            )
            .unwrap();
        imported
            .execute(
                "DELETE FROM staging WHERE x=?1 AND y=?2",
                rusqlite::params![x, y],
            )
            .unwrap();
        drop(imported);
        assert_eq!(
            directory.import_edge_bundle(&bundle_path).unwrap(),
            (owner, 17)
        );
        assert!(vertices
            .iter()
            .all(|corner| directory.read(*corner).unwrap().is_some()));

        let next_epoch_path = temporary.path().join("edge-next-epoch.sqlite");
        let mut next_epoch = CornerStore::open(&next_epoch_path, generation, owner).unwrap();
        for corners in vertices.chunks(grid::surface_corner::CORNER_COUNT) {
            next_epoch
                .resolve(corners, |_, missing| Ok(vec![value(); missing.len()]))
                .unwrap();
        }
        seal(&mut next_epoch, 18, receipt).unwrap();
        drop(next_epoch);
        assert!(directory.import_edge_bundle(&next_epoch_path).is_err());
        assert_eq!(
            read_receipt(&imported_path, generation, owner).unwrap(),
            (17, receipt)
        );
        assert!(vertices
            .iter()
            .all(|corner| directory.read(*corner).unwrap().is_some()));

        let missing = vertices[1].coordinates();
        let partial = Connection::open(&imported_path).unwrap();
        partial
            .execute(
                "DELETE FROM staging WHERE x=?1 AND y=?2",
                rusqlite::params![missing[0], missing[1]],
            )
            .unwrap();
        partial
            .execute(
                "DELETE FROM corners WHERE x=?1 AND y=?2",
                rusqlite::params![missing[0], missing[1]],
            )
            .unwrap();
        let partial_count: u64 = partial
            .query_row("SELECT count(*) FROM corners", [], |row| row.get(0))
            .unwrap();
        drop(partial);
        assert!(directory.import_edge_bundle(&next_epoch_path).is_err());
        let unchanged = Connection::open(&imported_path).unwrap();
        assert_eq!(
            unchanged
                .query_row("SELECT count(*) FROM corners", [], |row| row
                    .get::<_, u64>(0))
                .unwrap(),
            partial_count
        );
    }

    #[test]
    fn missing_vertex_or_changed_energy_fails_closed() {
        let temporary = tempfile::tempdir().unwrap();
        let bundle_path = temporary.path().join("edge.sqlite");
        let owner = Square { x: 0, y: 0 };
        let receipt = GenerationReceipt {
            sources: [3; 32],
            rasters: [4; 32],
            code: SURFACE_CODE_DIGEST,
            producer: [6; 32],
        };
        let generation = receipt.generation();
        let vertices = owner_edge_corners(owner);
        let mut store = CornerStore::open(&bundle_path, generation, owner).unwrap();
        store
            .resolve(&vertices, |_, missing| Ok(vec![value(); missing.len()]))
            .unwrap();
        seal(&mut store, 4, receipt).unwrap();
        store
            .connection
            .execute(
                "DELETE FROM staging WHERE x=?1 AND y=?2",
                rusqlite::params![vertices[0].coordinates()[0], vertices[0].coordinates()[1]],
            )
            .unwrap();
        drop(store);
        assert!(read(&bundle_path, generation).is_err());
    }
}

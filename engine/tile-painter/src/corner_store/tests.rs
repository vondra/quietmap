//! Producer reuse, immutable tile receipts and crash-safe release regression tests.
use super::*;
use crate::{corner_directory::CornerDirectory, hm3};
use sha2::{Digest, Sha256};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

fn value() -> CornerEnergy {
    CornerEnergy(vec![SourceEnergy {
        layer: 1,
        source: SourceIdentity::arrow_row([7; 32], 17, 0),
        periods: [f32::from_bits(0x3f123456), 3.0, 4.0],
    }])
}

fn tiles() -> Vec<EncodedHm3> {
    let pixels = grid::surface_corner::TILE_PIXEL_SIDE.pow(2);
    hm3::SURFACE_SOURCE_IDS
        .into_iter()
        .map(|id| hm3::encode_period_power(&vec![0.0; pixels * 3], id, &vec![0.0; pixels]).unwrap())
        .collect()
}

#[test]
fn adjacent_tiles_restart_and_concurrent_producers_reuse_the_same_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("corners");
    let generation = CornerGeneration::from_manifests([1; 32], [2; 32], [3; 32], [4; 32]);
    let count = Arc::new(AtomicUsize::new(0));
    std::thread::scope(|scope| {
        for tile_x in [15, 16] {
            let root = &root;
            let count = &count;
            scope.spawn(move || {
                let directory = CornerDirectory::new(root, generation);
                let vertices = tile_corners(tile_x, 15).unwrap();
                let result = directory
                    .resolve(&vertices, |owner, missing| {
                        assert!(missing.iter().all(|corner| corner.owner() == owner));
                        count.fetch_add(missing.len(), Ordering::SeqCst);
                        Ok(vec![value(); missing.len()])
                    })
                    .unwrap();
                assert!(result.iter().all(|energy| energy == &value()));
            });
        }
    });
    assert_eq!(count.load(Ordering::SeqCst), 65 * 33);
    let edge = SurfaceCorner::for_tile(16, 15, 0, 32).unwrap();
    assert_eq!(
        CornerDirectory::new(&root, generation)
            .resolve(&[edge, edge], |_, _| panic!("already computed"))
            .unwrap(),
        vec![value(), value()]
    );
    assert!(CornerDirectory::new(&root, CornerGeneration([9; 32]))
        .resolve(&[edge], |_, _| panic!("wrong generation"))
        .is_err());
}

#[test]
fn failed_or_invalid_owner_batch_publishes_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let owner = Square { x: 0, y: 0 };
    let mut store = CornerStore::open(
        &temp.path().join("corners.sqlite"),
        CornerGeneration([0; 32]),
        owner,
    )
    .unwrap();
    let vertices = [
        SurfaceCorner::new(510, 0).unwrap(),
        SurfaceCorner::new(511, 0).unwrap(),
    ];
    assert!(store
        .resolve(&vertices, |_, _| bail!("raster coverage absent"))
        .is_err());
    assert!(store.read(vertices[0]).unwrap().is_none());
    assert!(store
        .resolve(&vertices, |_, missing| {
            let mut invalid = value();
            invalid.0[0].periods[0] = f32::NAN;
            Ok(vec![invalid; missing.len()])
        })
        .is_err());
    assert!(vertices
        .iter()
        .all(|&corner| store.read(corner).unwrap().is_none()));
}

#[test]
fn five_hm3_hashes_and_local_corner_release_are_one_immutable_transaction() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("corners.sqlite");
    let owner = Square { x: 0, y: 0 };
    let generation = CornerGeneration([6; 32]);
    let mut store = CornerStore::open(&path, generation, owner).unwrap();
    let vertices = tile_corners(0, 0).unwrap();
    store
        .resolve(&vertices, |_, missing| Ok(vec![value(); missing.len()]))
        .unwrap();
    let tile_bytes = tiles();
    let receipt = store.write(0, 0, &tile_bytes).unwrap();
    assert!(receipt
        .layer_sha256()
        .iter()
        .all(|digest| *digest != [0; 32]));
    assert_eq!(store.committed(0, 0).unwrap(), Some(receipt));

    let local = SurfaceCorner::for_tile(0, 0, 1, 1).unwrap();
    let remaining: u8 = store
        .connection
        .query_row(
            "SELECT remaining FROM corners WHERE x=?1 AND y=?2",
            local.coordinates(),
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(remaining & local.consumer_bit(0, 0).unwrap(), 0);

    assert_eq!(store.write(0, 0, &tile_bytes).unwrap(), receipt);
    let mut changed = tile_bytes.clone();
    changed[0].bytes.push(0);
    assert!(store.write(0, 0, &changed).is_err());
    assert_eq!(store.committed(0, 0).unwrap(), Some(receipt));

    store
        .connection
        .execute(
            "UPDATE surface_tiles SET sha256=zeroblob(32) WHERE x=0 AND y=0 AND layer=0",
            [],
        )
        .unwrap();
    assert!(store.committed(0, 0).is_err());
}

#[test]
fn five_layer_contract_rejects_missing_or_reordered_tiles() {
    let temp = tempfile::tempdir().unwrap();
    let owner = Square { x: 0, y: 0 };
    let generation = CornerGeneration([8; 32]);
    let mut store =
        CornerStore::open(&temp.path().join("corners.sqlite"), generation, owner).unwrap();
    let vertices = tile_corners(0, 0).unwrap();
    store
        .resolve(&vertices, |_, missing| Ok(vec![value(); missing.len()]))
        .unwrap();
    let mut incomplete = tiles();
    incomplete.pop();
    assert!(store.write(0, 0, &incomplete).is_err());
    let mut reordered = tiles();
    reordered.swap(0, 1);
    assert!(store.write(0, 0, &reordered).is_err());
    assert!(store.committed(0, 0).unwrap().is_none());
}

#[test]
fn canonical_release_crosses_dateline_and_is_idempotent_after_restart() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("corners");
    let generation = CornerGeneration([0; 32]);
    let directory = CornerDirectory::new(&root, generation);
    let south = grid::surface_corner::WORLD_BLOCK_SIDE;
    let corner = SurfaceCorner::new(0, south).unwrap();
    assert_eq!(corner.dependent_tiles(), vec![(0, 8191), (8191, 8191)]);

    for (x, y) in corner.dependent_tiles() {
        let vertices = tile_corners(x, y).unwrap();
        directory
            .resolve(&vertices, |_, missing| Ok(vec![value(); missing.len()]))
            .unwrap();
        let receipt = directory.write(x, y, &tiles()).unwrap();
        directory.release(receipt).unwrap();
        directory.release(receipt).unwrap();
    }

    let store =
        CornerStore::open_read_only(&root.join("z9/0/511.sqlite"), generation, corner.owner())
            .unwrap()
            .unwrap();
    assert!(store.read(corner).unwrap().is_some());
    assert!(CornerDirectory::new(&root, generation)
        .resolve(&[corner], |_, _| panic!(
            "completed corner must not recompute"
        ))
        .is_err());
}

#[test]
fn missing_pending_staging_and_moved_owner_store_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("corners.sqlite");
    let owner = Square { x: 0, y: 0 };
    let generation = CornerGeneration([0; 32]);
    let mut store = CornerStore::open(&path, generation, owner).unwrap();
    let corner = SurfaceCorner::new(20, 20).unwrap();
    store.resolve(&[corner], |_, _| Ok(vec![value()])).unwrap();
    store.connection.execute("DELETE FROM staging", []).unwrap();
    assert!(store
        .resolve(&[corner], |_, _| panic!(
            "never recompute corrupted staging"
        ))
        .is_err());
    drop(store);
    assert!(CornerStore::open(&path, generation, Square { x: 1, y: 0 }).is_err());
}

#[test]
fn complete_owner_result_publishes_once_with_all_five_layer_hashes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("corners");
    let result = temp.path().join("results/owner.sqlite");
    let owner = Square { x: 3, y: 4 };
    let generation = CornerGeneration([11; 32]);
    let directory = CornerDirectory::new(&root, generation);
    let path = directory.owner_path(owner);
    crate::durable_directory::create_dir_all(path.parent().unwrap()).unwrap();
    let store = CornerStore::open(&path, generation, owner).unwrap();
    let bytes = b"exact hm3";
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    let tx = store.connection.unchecked_transaction().unwrap();
    let ((x0, x1), (y0, y1)) = grid::owned_z13(owner);
    for y in y0..y1 {
        for x in x0..x1 {
            for layer in 0..5 {
                tx.execute(
                    "INSERT INTO surface_tiles VALUES(?1,?2,?3,?4,?5)",
                    rusqlite::params![x, y, layer, bytes, digest.as_slice()],
                )
                .unwrap();
            }
        }
    }
    tx.commit().unwrap();
    drop(store);
    let published = directory.publish_owner_result(owner, &result).unwrap();
    assert_eq!(
        published,
        crate::generation_receipt::file_digest(&path).unwrap()
    );
    assert_eq!(
        directory.publish_owner_result(owner, &result).unwrap(),
        published
    );
    std::fs::write(&result, b"different").unwrap();
    assert!(directory.publish_owner_result(owner, &result).is_err());
}

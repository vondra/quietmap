//! Actual native-file validation, eviction and concurrent-reader regression tests.

use super::*;
use crate::test_fixture::write_square;
use std::cell::{Cell, RefCell};
use std::fs::{FileTimes, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{FileExt, MetadataExt};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

type HashObserver = Box<dyn FnMut(&File)>;
thread_local! {
    static HASH_OBSERVER: RefCell<Option<HashObserver>> = const { RefCell::new(None) };
}

pub(super) fn observe_full_hash(file: &File) {
    HASH_OBSERVER.with_borrow_mut(|observer| {
        if let Some(observer) = observer {
            observer(file);
        }
    });
}

struct ObserveHash;

impl ObserveHash {
    fn new(observer: impl FnMut(&File) + 'static) -> Self {
        HASH_OBSERVER.with_borrow_mut(|slot| *slot = Some(Box::new(observer)));
        Self
    }
}

impl Drop for ObserveHash {
    fn drop(&mut self) {
        HASH_OBSERVER.with_borrow_mut(|slot| *slot = None);
    }
}

const A: Square = Square { x: 256, y: 200 };
const B: Square = Square { x: 257, y: 200 };

fn fixture(root: &Path) -> TileStore {
    for square in [A, B] {
        write_square(root, Channel::Dem, square, |_, _| 100);
    }
    let capacity = [A, B]
        .map(|square| Channel::Dem.byte_len(RasterWindow::for_square(square)))
        .into_iter()
        .max()
        .unwrap()
        + std::mem::size_of::<CachedTile>();
    TileStore::new(root, Channel::Dem, capacity)
}

fn sample(tile: &RawTile) -> f64 {
    tile.read_pixel(tile.window.rows / 2, tile.window.columns / 2)
}

#[test]
fn verification_survives_eviction_but_revalidates_file_changes() {
    for (mutation, accepted, additional_hashes) in [
        ("unchanged", true, 0),
        ("rewrite", false, 1),
        ("restore_mtime", false, 1),
        ("replace_corrupt", false, 1),
        ("replace_identical", true, 1),
        ("truncate", false, 0),
    ] {
        let root = tempfile::tempdir().unwrap();
        let store = fixture(root.path());
        let hashes = Rc::new(Cell::new(0));
        let observed = Rc::clone(&hashes);
        let _observer = ObserveHash::new(move |_| observed.set(observed.get() + 1));
        assert_eq!(sample(&store.get_tile(A).unwrap()), 100.0);
        assert_eq!(sample(&store.get_tile(B).unwrap()), 100.0);
        assert!(!store.cache.lock().unwrap().tiles.contains_key(&A));
        assert_eq!(hashes.get(), 2);

        let path = Channel::Dem.path(root.path(), A);
        let before = std::fs::metadata(&path).unwrap();
        match mutation {
            "unchanged" => {}
            "rewrite" | "restore_mtime" => {
                let file = OpenOptions::new().write(true).open(&path).unwrap();
                file.write_all_at(&77_i16.to_be_bytes(), 0).unwrap();
                if mutation == "restore_mtime" {
                    file.set_times(FileTimes::new().set_modified(before.modified().unwrap()))
                        .unwrap();
                    let after = file.metadata().unwrap();
                    assert_eq!(after.modified().unwrap(), before.modified().unwrap());
                    assert_ne!(
                        (after.ctime(), after.ctime_nsec()),
                        (before.ctime(), before.ctime_nsec())
                    );
                }
            }
            "replace_corrupt" | "replace_identical" => {
                let mut bytes = std::fs::read(&path).unwrap();
                if mutation == "replace_corrupt" {
                    bytes[0] ^= 1;
                }
                let mut replacement =
                    tempfile::NamedTempFile::new_in(path.parent().unwrap()).unwrap();
                replacement.write_all(&bytes).unwrap();
                replacement.persist(&path).unwrap();
                assert_ne!(std::fs::metadata(&path).unwrap().ino(), before.ino());
            }
            "truncate" => OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .set_len(2)
                .unwrap(),
            _ => unreachable!(),
        }

        let tile = store.get_tile(A);
        assert_eq!(tile.is_some(), accepted, "{mutation}");
        if let Some(tile) = tile {
            assert_eq!(sample(&tile), 100.0, "{mutation}");
        }
        assert_eq!(hashes.get(), 2 + additional_hashes, "{mutation}");
    }
}

#[test]
fn metadata_change_during_hash_cannot_install_verification() {
    let root = tempfile::tempdir().unwrap();
    let store = fixture(root.path());
    let hashes = Rc::new(Cell::new(0));
    let observed = Rc::clone(&hashes);
    let _observer = ObserveHash::new(move |file| {
        observed.set(observed.get() + 1);
        if observed.get() == 1 {
            file.set_times(FileTimes::new().set_modified(SystemTime::UNIX_EPOCH))
                .unwrap();
        }
    });
    assert!(store.get_tile(A).is_none());
    assert!(store.get_tile(B).is_some());
    assert!(!store.cache.lock().unwrap().tiles.contains_key(&A));
    assert_eq!(sample(&store.get_tile(A).unwrap()), 100.0);
    assert_eq!(hashes.get(), 3);
}

#[test]
fn cold_validation_does_not_block_an_unrelated_warm_reader() {
    let root = tempfile::tempdir().unwrap();
    let store = fixture(root.path());
    assert!(store.get_tile(B).is_some());
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (warm_tx, warm_rx) = mpsc::channel();
    std::thread::scope(|scope| {
        let cold = scope.spawn(|| {
            let _observer = ObserveHash::new(move |_| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            });
            store.get_tile(A).map(|tile| sample(&tile))
        });
        if let Err(error) = entered_rx.recv_timeout(Duration::from_secs(5)) {
            drop(release_tx);
            panic!("cold validation never reached the hash: {error}");
        }
        scope.spawn(|| {
            warm_tx
                .send(store.get_tile(B).map(|tile| sample(&tile)))
                .unwrap()
        });
        let warm = warm_rx.recv_timeout(Duration::from_secs(5));
        release_tx.send(()).unwrap();
        assert_eq!(warm.unwrap(), Some(100.0));
        assert_eq!(cold.join().unwrap(), Some(100.0));
    });
}

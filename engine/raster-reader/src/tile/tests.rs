//! The three file states (data, 0-byte ocean, missing), wrong lengths and byte-bounded eviction.

use super::*;
use crate::test_fixture::{write_absent_square, write_square};
use std::fs::OpenOptions;

const A: Square = Square { x: 256, y: 200 };
const B: Square = Square { x: 257, y: 200 };

fn store_with_room_for_one_window(root: &Path, channel: Channel) -> TileStore {
    let capacity = [A, B]
        .map(|square| channel.byte_len(RasterWindow::for_square(square)))
        .into_iter()
        .max()
        .unwrap()
        + std::mem::size_of::<CachedTile>();
    TileStore::new(root, channel, capacity)
}

fn centre(tile: &RawTile) -> f64 {
    tile.read_pixel(tile.window.rows / 2, tile.window.columns / 2)
}

#[test]
fn zero_byte_file_samples_the_channel_ocean_value() {
    let root = tempfile::tempdir().unwrap();
    for (channel, ocean) in [
        (Channel::Dem, 0.0),
        (Channel::Forest, 0.0),
        (Channel::Imd, 100.0),
    ] {
        write_absent_square(root.path(), channel, A);
        let store = TileStore::new(root.path(), channel, usize::MAX);
        let tile = store.get_tile(A).unwrap();
        assert_eq!(centre(&tile), ocean, "{channel:?}");
        assert_eq!(
            tile.sample(36.3, 0.35, Interp::Bilinear),
            ocean,
            "{channel:?}"
        );
        assert_eq!(
            tile.sample(36.3, 0.35, Interp::Nearest),
            ocean,
            "{channel:?}"
        );
    }
}

#[test]
fn missing_file_is_refused_not_ocean() {
    let root = tempfile::tempdir().unwrap();
    write_absent_square(root.path(), Channel::Dem, B);
    let store = TileStore::new(root.path(), Channel::Dem, usize::MAX);
    assert!(store.get_tile(A).is_none());
    assert!(store.sample(36.3, 0.35).is_nan());
    assert!(store.get_tile(B).is_some());
}

#[test]
fn data_file_of_wrong_length_is_refused() {
    let root = tempfile::tempdir().unwrap();
    write_square(root.path(), Channel::Dem, A, |_, _| 100);
    let store = TileStore::new(root.path(), Channel::Dem, usize::MAX);
    assert_eq!(centre(&store.get_tile(A).unwrap()), 100.0);
    OpenOptions::new()
        .write(true)
        .open(Channel::Dem.path(root.path(), A))
        .unwrap()
        .set_len(2)
        .unwrap();
    let fresh = TileStore::new(root.path(), Channel::Dem, usize::MAX);
    assert!(fresh.get_tile(A).is_none());
}

#[test]
fn eviction_keeps_the_cache_within_its_byte_bound_and_reloads() {
    let root = tempfile::tempdir().unwrap();
    for square in [A, B] {
        write_square(root.path(), Channel::Dem, square, |_, _| 100);
    }
    let store = store_with_room_for_one_window(root.path(), Channel::Dem);
    assert_eq!(centre(&store.get_tile(A).unwrap()), 100.0);
    assert_eq!(centre(&store.get_tile(B).unwrap()), 100.0);
    let cache = store.cache.lock().unwrap();
    assert!(!cache.tiles.contains_key(&A));
    assert!(cache.tiles.contains_key(&B));
    assert!(cache.bytes <= store.max_bytes);
    drop(cache);
    assert_eq!(centre(&store.get_tile(A).unwrap()), 100.0);
}

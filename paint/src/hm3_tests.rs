//! HM3 predictor identity, malformed input and atomic square completion.

use super::*;
use crate::paint::{NO_LEVEL, SILENCE};

#[test]
fn a_non_file_completion_marker_cannot_skip_a_square() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(square_path(directory.path(), 13, (1, 2))).unwrap();
    assert!(
        painted(directory.path(), 13, (1, 2))
            .unwrap_err()
            .contains("not a square bundle file")
    );
    let path = square_path(directory.path(), 13, (1, 3));
    std::fs::write(&path, b"incomplete bundle").unwrap();
    assert!(painted(directory.path(), 13, (1, 3)).is_err());
}

#[test]
fn every_byte_and_row_boundary_round_trips() {
    let cells: Vec<u8> = (0..TILE_PX * TILE_PX)
        .map(|at| ((at / TILE_PX * 19 + at % TILE_PX * 31) % 256) as u8)
        .collect();
    let encoded = encode_tile(&cells, LAYERS).unwrap();
    assert_eq!(decode_tile(&encoded, LAYERS).unwrap(), cells);
    assert!(decode_tile(&encoded, 0).is_err());
    assert!(decode_tile(&encoded[..encoded.len() / 2], LAYERS).is_err());
    let mut trailing = encoded.clone();
    trailing.push(0);
    assert!(decode_tile(&trailing, LAYERS).is_err());
    assert!(decode_tile(&[], LAYERS).is_err());
    assert!(encode_tile(&cells[..TILE_PX], LAYERS).is_err());
}

#[test]
fn one_complete_bundle_keeps_quadrants_layers_and_assessed_silence() {
    let directory = tempfile::tempdir().unwrap();
    for zoom in [12, 13] {
        let side = (1usize << (zoom - 12)) * TILE_PX;
        let mut cells = vec![[SILENCE; LAYERS + 1]; side * side];
        for (at, pixel) in cells.iter_mut().enumerate() {
            for (layer, byte) in pixel.iter_mut().enumerate() {
                *byte = ((at / side / TILE_PX * 7 + at % side / TILE_PX * 13 + layer) % 254) as u8;
            }
        }
        cells[0] = [NO_LEVEL; LAYERS + 1];
        cells[side] = [SILENCE; LAYERS + 1];
        assert!(!painted(directory.path(), zoom, (9, 17)).unwrap());
        write(directory.path(), zoom, (9, 17), &cells).unwrap();
        assert!(painted(directory.path(), zoom, (9, 17)).unwrap());
        assert_eq!(read(directory.path(), zoom, (9, 17)).unwrap(), cells);
        assert_eq!(squares(directory.path(), zoom).unwrap(), [(9, 17)].into());
        let before = std::fs::read(square_path(directory.path(), zoom, (9, 17))).unwrap();
        assert!(write(directory.path(), zoom, (9, 17), &cells).is_err());
        assert_eq!(
            std::fs::read(square_path(directory.path(), zoom, (9, 17))).unwrap(),
            before
        );
    }
}

#[test]
fn identical_quadrants_share_bytes_and_invalid_ranges_are_refused() {
    let directory = tempfile::tempdir().unwrap();
    let cells = vec![[SILENCE; LAYERS + 1]; 1024 * 1024];
    write(directory.path(), 13, (0, 0), &cells).unwrap();
    let square = PaintedSquare::open(directory.path(), 13, (0, 0)).unwrap();
    for entries in square.entries.chunks_exact(4) {
        assert!(entries.iter().all(|entry| *entry == entries[0]));
    }
    let path = square_path(directory.path(), 13, (0, 0));
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[SQUARE_HEADER_BYTES..SQUARE_HEADER_BYTES + 4].fill(0);
    std::fs::write(&path, bytes).unwrap();
    assert!(PaintedSquare::open(directory.path(), 13, (0, 0)).is_err());
}

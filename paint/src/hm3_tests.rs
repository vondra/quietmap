//! HM3 predictor identity, malformed input and atomic square completion.

use super::*;
use crate::hm3_square::SQUARE_HEADER_BYTES;
use crate::levels::PaintedCells;
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
        write(
            directory.path(),
            zoom,
            (9, 17),
            &PaintedCells {
                nden: cells.clone(),
                lden: cells.clone(),
            },
        )
        .unwrap();
        assert!(painted(directory.path(), zoom, (9, 17)).unwrap());
        assert_eq!(read_nden(directory.path(), zoom, (9, 17)).unwrap(), cells);
        assert_eq!(
            PaintedSquare::open(directory.path(), zoom, (9, 17))
                .unwrap()
                .read_lden()
                .unwrap(),
            cells
        );
        assert_eq!(squares(directory.path(), zoom).unwrap(), [(9, 17)].into());
        let before = std::fs::read(square_path(directory.path(), zoom, (9, 17))).unwrap();
        assert!(
            write(
                directory.path(),
                zoom,
                (9, 17),
                &PaintedCells {
                    nden: cells.clone(),
                    lden: cells.clone()
                }
            )
            .is_err()
        );
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
    write(
        directory.path(),
        13,
        (0, 0),
        &PaintedCells {
            nden: cells.clone(),
            lden: cells.clone(),
        },
    )
    .unwrap();
    let square = PaintedSquare::open(directory.path(), 13, (0, 0)).unwrap();
    for layer in 0..=LAYERS {
        let first = square.compressed_tile(layer, 0, 0).unwrap();
        for (x, y) in [(1, 0), (0, 1), (1, 1)] {
            assert_eq!(square.compressed_tile(layer, x, y).unwrap(), first);
        }
    }
    let path = square_path(directory.path(), 13, (0, 0));
    let mut bytes = std::fs::read(&path).unwrap();
    let stored_bytes: usize = (0..=LAYERS)
        .map(|layer| {
            [Metric::Nden, Metric::Lden]
                .into_iter()
                .map(|metric| {
                    encode_metric_tile(&vec![SILENCE; TILE_PX * TILE_PX], layer, metric)
                        .unwrap()
                        .len()
                })
                .sum::<usize>()
        })
        .sum();
    assert_eq!(
        bytes.len(),
        SQUARE_HEADER_BYTES + 2 * (LAYERS + 1) * 4 * 8 + stored_bytes
    );
    bytes[SQUARE_HEADER_BYTES..SQUARE_HEADER_BYTES + 4].fill(0);
    std::fs::write(&path, bytes).unwrap();
    assert!(PaintedSquare::open(directory.path(), 13, (0, 0)).is_err());
}

#[test]
fn legacy_squares_stay_complete_and_dual_streams_keep_the_same_nden_bytes() {
    let old = tempfile::tempdir().unwrap();
    let path = square_path(old.path(), 12, (9, 17));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, include_bytes!("../tests/fixtures/square-v7.hm3")).unwrap();
    assert!(painted(old.path(), 12, (9, 17)).unwrap());
    assert!(!verify(old.path(), 12, (9, 17)).unwrap());
    let legacy = PaintedSquare::open(old.path(), 12, (9, 17)).unwrap();
    assert!(legacy.read_lden().is_err());
    let nden = read_nden(old.path(), 12, (9, 17)).unwrap();
    let lden = nden
        .iter()
        .map(|pixel| pixel.map(|v| if v >= SILENCE { v } else { v + 1 }))
        .collect();
    let new = tempfile::tempdir().unwrap();
    write(
        new.path(),
        12,
        (9, 17),
        &PaintedCells {
            nden: nden.clone(),
            lden,
        },
    )
    .unwrap();
    assert!(verify(new.path(), 12, (9, 17)).unwrap());
    let dual = PaintedSquare::open(new.path(), 12, (9, 17)).unwrap();
    for layer in 0..=LAYERS {
        assert_eq!(
            legacy.compressed_tile(layer, 0, 0).unwrap(),
            dual.compressed_tile(layer, 0, 0).unwrap()
        );
    }
    assert_eq!(read_nden(new.path(), 12, (9, 17)).unwrap(), nden);
    let published_old = tempfile::tempdir().unwrap();
    let published_new = tempfile::tempdir().unwrap();
    crate::pack::pack(old.path(), 12, published_old.path(), "b1", None).unwrap();
    crate::pack::pack(new.path(), 12, published_new.path(), "b1", None).unwrap();
    for name in layer_names() {
        let file = format!("{name}.b1.pmtiles");
        assert_eq!(
            std::fs::read(published_old.path().join(&file)).unwrap(),
            std::fs::read(published_new.path().join(&file)).unwrap()
        );
    }
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(published_new.path().join("current.json")).unwrap())
            .unwrap();
    assert_eq!(manifest["hm3_version"], 7);
    assert!(crate::merge::merge(old.path(), new.path(), 12).is_err());
    let path = square_path(new.path(), 12, (9, 17));
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.pop();
    std::fs::write(&path, bytes).unwrap();
    assert!(verify(new.path(), 12, (9, 17)).is_err());
}

#[test]
fn metric_tags_and_assessment_cannot_be_swapped_or_lost() {
    let cells = vec![100; TILE_PX * TILE_PX];
    let lden = encode_metric_tile(&cells, 0, Metric::Lden).unwrap();
    assert_eq!(decode_metric_tile(&lden, 0, Metric::Lden).unwrap(), cells);
    assert!(decode_tile(&lden, 0).is_err());
    assert!(decode_metric_tile(&encode_tile(&cells, 0).unwrap(), 0, Metric::Lden).is_err());
    let out = tempfile::tempdir().unwrap();
    let mut values = PaintedCells::silence(TILE_PX * TILE_PX);
    values.lden[0][0] = NO_LEVEL;
    write(out.path(), 12, (0, 0), &values).unwrap();
    assert!(
        verify(out.path(), 12, (0, 0))
            .unwrap_err()
            .contains("assessment differs")
    );
    values.nden[0][0] = NO_LEVEL;
    write(out.path(), 12, (1, 0), &values).unwrap();
    assert!(
        verify(out.path(), 12, (1, 0))
            .unwrap_err()
            .contains("assessment differs")
    );
}

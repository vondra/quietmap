//! Worker bundle merging, verbatim archive bodies and fail-closed publication.

use super::*;
use crate::hm3::{self, square_path};
use crate::levels::PaintedCells;

fn write(
    out: &Path,
    zoom: u32,
    square: (u32, u32),
    cells: &crate::paint::Cells,
) -> Result<(), String> {
    hm3::write(
        out,
        zoom,
        square,
        &PaintedCells {
            nden: cells.clone(),
            lden: cells.clone(),
        },
    )
}

#[test]
fn worker_bundles_merge_atomically_and_publication_preserves_their_streams() {
    let workers = tempfile::tempdir().unwrap();
    let merged = tempfile::tempdir().unwrap();
    let published = tempfile::tempdir().unwrap();
    let cells = vec![[80; LAYERS + 1]; 1024 * 1024];
    for (worker, square) in [("a", (2, 2)), ("b", (3, 2))] {
        let root = workers.path().join(worker);
        write(&root, 13, square, &cells).unwrap();
        let destination = square_path(merged.path(), 13, square);
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        let mut interrupted =
            tempfile::NamedTempFile::new_in(destination.parent().unwrap()).unwrap();
        interrupted.write_all(b"incomplete copy").unwrap();
        assert!(!hm3::painted(merged.path(), 13, square).unwrap());
        crate::merge::merge(&root, merged.path(), 13).unwrap();
        assert_eq!(
            std::fs::read(square_path(&root, 13, square)).unwrap(),
            std::fs::read(square_path(merged.path(), 13, square)).unwrap()
        );
        write(&root, 13, (square.0, square.1 + 1), &cells).unwrap();
        crate::merge::merge(&root, merged.path(), 13).unwrap();
        assert!(hm3::painted(merged.path(), 13, (square.0, square.1 + 1)).unwrap());
    }
    pack(merged.path(), 13, published.path(), "b1", None).unwrap();
    let manifest_path = published.path().join("current.json");
    let manifest = std::fs::read(&manifest_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
    assert_eq!(parsed["layers"].as_object().unwrap().len(), LAYERS + 1);
    let square = PaintedSquare::open(merged.path(), 13, (2, 2)).unwrap();
    for (number, layer) in layer_names().iter().enumerate() {
        let archive = std::fs::read(published.path().join(format!("{layer}.b1.pmtiles"))).unwrap();
        let header = pmtiles::Header::try_from_bytes(archive[..127].to_vec().into()).unwrap();
        assert_eq!(header.tile_compression, Compression::Brotli);
        assert!(header.clustered());
        assert!(
            header.n_tile_contents().unwrap().get() < header.n_addressed_tiles().unwrap().get()
        );
        let stream = square.compressed_tile(number, 0, 0).unwrap();
        let data_offset = u64::from_le_bytes(archive[56..64].try_into().unwrap()) as usize;
        let data_length = u64::from_le_bytes(archive[64..72].try_into().unwrap()) as usize;
        let data = &archive[data_offset..][..data_length];
        assert!(data.windows(stream.len()).any(|window| window == stream));
    }
    assert!(pack(merged.path(), 13, published.path(), "b1", None).is_err());
    assert_eq!(std::fs::read(&manifest_path).unwrap(), manifest);
    let source = square_path(merged.path(), 13, (2, 2));
    File::create(source).unwrap();
    assert!(crate::merge::merge(&workers.path().join("a"), merged.path(), 13).is_err());
    assert!(pack(merged.path(), 13, published.path(), "b2", None).is_err());
    assert_eq!(std::fs::read(&manifest_path).unwrap(), manifest);
    assert!(!published.path().join("road.b2.pmtiles").exists());
}

/// Kept regions give the archives of a pack without them, byte for byte; a repainted square's region
/// is folded again, never served from its kept part.
#[test]
fn kept_regions_pack_the_same_archives_and_a_repainted_square_refolds_its_region() {
    let merged = tempfile::tempdir().unwrap();
    let regions = tempfile::tempdir().unwrap();
    let cells = |level: u8, at: usize| {
        let mut cells = vec![[crate::paint::NO_LEVEL; LAYERS + 1]; 1024 * 1024];
        for (k, pixel) in cells.iter_mut().enumerate().skip(at).step_by(7) {
            *pixel = [level.wrapping_add((k % 13) as u8); LAYERS + 1];
        }
        cells
    };
    // Two squares in zoom-8 region (0, 0), one in region (1, 0).
    for (k, square) in [(2, 2), (3, 2), (20, 2)].into_iter().enumerate() {
        write(merged.path(), 13, square, &cells(60 + 9 * k as u8, k)).unwrap();
    }
    let archives = |directory: &Path| -> Vec<Vec<u8>> {
        layer_names()
            .iter()
            .map(|layer| std::fs::read(directory.join(format!("{layer}.b1.pmtiles"))).unwrap())
            .collect()
    };
    let plain = tempfile::tempdir().unwrap();
    pack(merged.path(), 13, plain.path(), "b1", None).unwrap();
    assert_eq!(
        pack_region(merged.path(), 13, regions.path(), (0, 0)).unwrap(),
        2
    );
    assert_eq!(
        pack_region(merged.path(), 13, regions.path(), (5, 5)).unwrap(),
        0
    );
    let kept = tempfile::tempdir().unwrap();
    pack(merged.path(), 13, kept.path(), "b1", Some(regions.path())).unwrap();
    assert_eq!(archives(kept.path()), archives(plain.path()));
    assert!(regions.path().join("1_0.region").exists());
    // A square repainted after its region was kept.
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::remove_file(square_path(merged.path(), 13, (3, 2))).unwrap();
    write(merged.path(), 13, (3, 2), &cells(90, 3)).unwrap();
    let (fresh, reused) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    pack(merged.path(), 13, fresh.path(), "b1", None).unwrap();
    pack(merged.path(), 13, reused.path(), "b1", Some(regions.path())).unwrap();
    assert_eq!(archives(reused.path()), archives(fresh.path()));
    assert_ne!(archives(fresh.path()), archives(plain.path()));
}

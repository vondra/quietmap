//! Worker bundle merging, verbatim archive bodies and fail-closed publication.

use super::*;
use crate::hm3::{self, square_path};

#[test]
fn worker_bundles_merge_atomically_and_publication_preserves_their_streams() {
    let workers = tempfile::tempdir().unwrap();
    let merged = tempfile::tempdir().unwrap();
    let published = tempfile::tempdir().unwrap();
    let cells = vec![[80; LAYERS + 1]; 1024 * 1024];
    for (worker, square) in [("a", (2, 2)), ("b", (3, 2))] {
        let root = workers.path().join(worker);
        hm3::write(&root, 13, square, &cells).unwrap();
        let destination = square_path(merged.path(), 13, square);
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        let mut interrupted =
            tempfile::NamedTempFile::new_in(destination.parent().unwrap()).unwrap();
        interrupted.write_all(b"incomplete copy").unwrap();
        assert!(!hm3::painted(merged.path(), 13, square));
        crate::merge::merge(&root, merged.path(), 13).unwrap();
        assert_eq!(
            std::fs::read(square_path(&root, 13, square)).unwrap(),
            std::fs::read(square_path(merged.path(), 13, square)).unwrap()
        );
        hm3::write(&root, 13, (square.0, square.1 + 1), &cells).unwrap();
        crate::merge::merge(&root, merged.path(), 13).unwrap();
        assert!(hm3::painted(merged.path(), 13, (square.0, square.1 + 1)));
    }
    pack(merged.path(), 13, published.path(), "b1").unwrap();
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
    assert!(pack(merged.path(), 13, published.path(), "b1").is_err());
    assert_eq!(std::fs::read(&manifest_path).unwrap(), manifest);
    let source = square_path(merged.path(), 13, (2, 2));
    File::create(source).unwrap();
    assert!(crate::merge::merge(&workers.path().join("a"), merged.path(), 13).is_err());
    assert!(pack(merged.path(), 13, published.path(), "b2").is_err());
    assert_eq!(std::fs::read(&manifest_path).unwrap(), manifest);
    assert!(!published.path().join("road.b2.pmtiles").exists());
}

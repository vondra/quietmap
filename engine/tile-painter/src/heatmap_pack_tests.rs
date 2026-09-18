//! Round-trip authority completeness, original z13 bytes and independent energy-mean layers.
use super::*;
use crate::{
    corner_store::CornerStore,
    generation_receipt::{hex_digest, GenerationReceipt, SURFACE_FORMAT_AND_PHYSICS_GENERATION},
    hm3::decode_cells,
};
use grid::Z13_PER_Z9_SIDE;
use std::future::Future;
use std::task::{Poll, Waker};

/// In-memory PMTiles source: every read resolves immediately, so the reader's
/// futures can be driven by a plain poll loop with no async runtime.
struct ArchiveReader {
    bytes: Vec<u8>,
}
impl pmtiles::AsyncBackend for ArchiveReader {
    async fn read(
        &self,
        offset: usize,
        length: usize,
    ) -> pmtiles::PmtResult<pmtiles::BackendResponse> {
        let start = offset.min(self.bytes.len());
        let end = (offset + length).min(self.bytes.len());
        Ok(pmtiles::BackendResponse::new(
            bytes::Bytes::copy_from_slice(&self.bytes[start..end]),
        ))
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut context = std::task::Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut context) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("in-memory archive reads never block"),
    }
}

/// Read one tile through the crate's own reader, proving the archive a server
/// sees: v3, Brotli tiles served verbatim, gzip internal directories.
fn tile(archive: &[u8], z: u8, x: u32, y: u32) -> Vec<u8> {
    let reader = block_on(pmtiles::AsyncPmTilesReader::try_from_source(
        ArchiveReader {
            bytes: archive.to_vec(),
        },
    ))
    .unwrap();
    let header = reader.get_header();
    assert_eq!(header.spec_version(), 3);
    assert_eq!(header.tile_compression, Compression::Brotli);
    assert_eq!(header.internal_compression(), Compression::Gzip);
    assert_eq!((header.min_zoom, header.max_zoom), (2, 13));
    block_on(reader.get_tile(TileCoord::new(z, x, y).unwrap()))
        .unwrap()
        .expect("requested tile exists")
        .to_vec()
}

fn fixture(root: &Path) -> (std::path::PathBuf, String, std::path::PathBuf) {
    let receipt = GenerationReceipt {
        sources: [1; 32],
        code: SURFACE_FORMAT_AND_PHYSICS_GENERATION,
        producer: [2; 32],
    };
    let generation = receipt.generation();
    let hex = hex_digest(generation.0);
    let corners = root.join("corners");
    receipt.publish(&corners).unwrap();
    let path = corners.join("z9/0/0.sqlite");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    drop(CornerStore::open(&path, generation, grid::Square { x: 0, y: 0 }).unwrap());
    let mut db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE edge_bundle(id INTEGER PRIMARY KEY,epoch INTEGER,sources BLOB,code BLOB,producer BLOB)").unwrap();
    db.execute(
        "INSERT INTO edge_bundle VALUES(1,1,?1,?2,?3)",
        params![
            receipt.sources.as_slice(),
            receipt.code.as_slice(),
            receipt.producer.as_slice()
        ],
    )
    .unwrap();
    let tx = db.transaction().unwrap();
    for (i, layer) in ALL_LAYERS.into_iter().enumerate() {
        let mut cells = vec![255; TILE_PIXEL_SIDE * TILE_PIXEL_SIDE];
        // One loud pixel per source 2x2 block: area mean is 60 - 10log10(4).
        cells[0] = if layer == Hm3Layer::Total { 140 } else { 120 };
        let painted = encode_cells(&cells, layer).unwrap();
        let silent = encode_cells(&vec![255; cells.len()], layer).unwrap();
        for y in 0..Z13_PER_Z9_SIDE {
            for x in 0..Z13_PER_Z9_SIDE {
                let bytes = if x == 0 && y == 0 {
                    painted.as_bytes()
                } else {
                    silent.as_bytes()
                };
                tx.execute(
                    "INSERT INTO surface_tiles VALUES(?1,?2,?3,?4,?5)",
                    params![x, y, i, bytes, Sha256::digest(bytes).as_slice()],
                )
                .unwrap();
            }
        }
    }
    tx.commit().unwrap();
    drop(db);
    let authority = root.join("authority.sqlite");
    let db = Connection::open(&authority).unwrap();
    db.execute_batch("CREATE TABLE generations(generation TEXT,state TEXT); CREATE TABLE tasks(generation TEXT,phase TEXT,state TEXT,owner_z9_x INTEGER,owner_z9_y INTEGER,copy_a_path TEXT,result_sha256 TEXT,result_bytes INTEGER)").unwrap();
    db.execute("INSERT INTO generations VALUES(?1,'complete')", [&hex])
        .unwrap();
    let sha = hex_digest(file_digest(&path).unwrap());
    db.execute(
        "INSERT INTO tasks VALUES(?1,'owner','complete',0,0,?2,?3,?4)",
        params![
            hex,
            path.to_str().unwrap(),
            sha,
            path.metadata().unwrap().len()
        ],
    )
    .unwrap();
    (authority, hex, path)
}

#[test]
fn complete_owner_roundtrip_preserves_z13_and_never_sums_quantized_layers() {
    let root = tempfile::tempdir().unwrap();
    let (authority, generation, owner_path) = fixture(root.path());
    let output = root.path().join("staged");
    pack(&authority, &generation, &output, "b7").unwrap();
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(output.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["build"], "b7");
    assert_eq!(manifest["zoom"], 13);
    assert_eq!(manifest["layers"].as_object().unwrap().len(), 9);
    let owner = Connection::open(&owner_path).unwrap();
    for (i, layer) in ALL_LAYERS.into_iter().enumerate() {
        let archive = std::fs::read(output.join(format!("{}.b7.pmtiles", layer.name()))).unwrap();
        assert_eq!(
            tile(&archive, 13, 0, 0),
            read_tile(&owner, i, 0, 0).unwrap()
        );
        let parent = decode_cells(&tile(&archive, 12, 0, 0), layer).unwrap();
        assert_eq!(parent[0], if layer == Hm3Layer::Total { 128 } else { 108 });
        assert_eq!(parent[1], 255);
        assert_eq!(
            decode_cells(&tile(&archive, 2, 0, 0), layer).unwrap().len(),
            TILE_PIXEL_SIDE * TILE_PIXEL_SIDE
        );
    }
    let db = Connection::open(&authority).unwrap();
    db.execute(
        "INSERT INTO tasks VALUES(?1,'owner','pending',1,0,NULL,NULL,NULL)",
        [&generation],
    )
    .unwrap();
    let failed = root.path().join("incomplete");
    assert!(pack(&authority, &generation, &failed, "b8").is_err());
    assert!(!failed.exists());
    db.execute("DELETE FROM tasks WHERE state='pending'", [])
        .unwrap();
    // A missing whole planned artifact must fail even though its authority state says complete.
    std::fs::rename(&owner_path, owner_path.with_extension("saved")).unwrap();
    assert!(pack(&authority, &generation, &failed, "b8").is_err());
    assert!(!failed.exists());
}

#[test]
fn malformed_hm3_is_never_treated_as_silence() {
    let pixels = vec![255; TILE_PIXEL_SIDE * TILE_PIXEL_SIDE];
    let tile = encode_cells(&pixels, Hm3Layer::Road).unwrap();
    assert!(decode_cells(tile.as_bytes(), Hm3Layer::Rail).is_err());
    assert!(decode_cells(
        &tile.as_bytes()[..tile.as_bytes().len() / 2],
        Hm3Layer::Road
    )
    .is_err());
    assert!(encode_cells(&pixels[..pixels.len() - 1], Hm3Layer::Road).is_err());
}

//! On-disk layout of the tile store: file headers + the 16-byte index entry.
//!
//! Both files start with a small header carrying (magic, version, zoom,
//! source_id) so a stray file identifies itself. All integers little-endian.
//! The index entry is exactly 16 bytes and 16-byte aligned, so one entry never
//! straddles a page and a single `pwrite` replaces it whole.

use anyhow::{bail, Result};

/// Index file magic ("Quiet Tile Store Index").
pub const MAGIC_INDEX: &[u8; 4] = b"QTSI";
/// Data file magic ("Quiet Tile Store Data").
pub const MAGIC_DATA: &[u8; 4] = b"QTSD";
/// Store format version.
pub const VERSION: u8 = 1;
/// Header size of BOTH files (index uses all fields; data repeats them so a
/// lone `.qtsd` is still self-describing).
pub const HEADER_BYTES: u64 = 32;
/// One index entry: offset u64 + len u32 + codec u8 + 3 reserved.
pub const ENTRY_BYTES: u64 = 16;

/// How a stored blob is encoded. Kept as a u8 tag in every index entry so a
/// store can read legacy Zstd cells alongside current Brotli HM3 blobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileCodec {
    /// A complete HM3 file image: whole-file Brotli of header + cells —
    /// byte-identical to its loose staging `.bin`. Ships verbatim into PMTiles
    /// (`TileStore::get_hm3_by_entry`) — since 2026-07-16 this is how EVERY
    /// central writer (`build_heatmap_combine`, `pyramid::build_one_level`,
    /// via `TileStore::put_cells`) stores its output too, not just
    /// fleet-encoded ingest/transcode blobs: the Brotli-q9 encode happens
    /// once at write time, amortized over the days between publishes,
    /// instead of hundreds of GB of it being redone on every publish.
    BrotliHm3 = 0,
    /// zstd-1 over the raw cell grid (no HM3 header — zoom/source_id live
    /// in the store header). LEGACY READ-ONLY since 2026-07-16 — it was the
    /// cheap working codec central rewrites used before that date, which
    /// forced `tile-store-pack` to zstd-decode-then-brotli-q9-re-encode every
    /// one of those tiles on EVERY publish (measured ~63 min for one 580k-tile
    /// layer). New central writes are `BrotliHm3` (`put_cells`); this variant
    /// and `decode_cells`'s `ZstdCells` arm stay only so a store not yet fully
    /// rewritten keeps publishing correctly. Delete this codec (and every arm handling it)
    /// once every store's z2..z11+total entries have been rewritten
    /// post-cutover.
    ZstdCells = 1,
}

impl TileCodec {
    pub fn from_u8(v: u8) -> Result<Self> {
        match v {
            0 => Ok(TileCodec::BrotliHm3),
            1 => Ok(TileCodec::ZstdCells),
            other => bail!("unknown tile codec tag {other}"),
        }
    }
}

/// One decoded index entry. `len == 0` means the tile is absent (the all-zero
/// entry of a sparse index page reads as "absent" for free).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub offset: u64,
    pub len: u32,
    pub codec: TileCodec,
}

impl Entry {
    pub fn encode(&self) -> [u8; ENTRY_BYTES as usize] {
        let mut b = [0u8; ENTRY_BYTES as usize];
        b[0..8].copy_from_slice(&self.offset.to_le_bytes());
        b[8..12].copy_from_slice(&self.len.to_le_bytes());
        b[12] = self.codec as u8;
        b
    }

    /// `None` = absent tile (len 0). Errors only on a corrupt codec tag.
    pub fn decode(b: &[u8]) -> Result<Option<Self>> {
        let len = u32::from_le_bytes(b[8..12].try_into().unwrap());
        if len == 0 {
            return Ok(None);
        }
        Ok(Some(Entry {
            offset: u64::from_le_bytes(b[0..8].try_into().unwrap()),
            len,
            codec: TileCodec::from_u8(b[12])?,
        }))
    }
}

/// Header of either file. `tile_px` makes the stored grid size explicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub zoom: u8,
    pub source_id: u8,
    pub tile_px: u16,
}

impl Header {
    pub fn encode(&self, magic: &[u8; 4]) -> [u8; HEADER_BYTES as usize] {
        let mut b = [0u8; HEADER_BYTES as usize];
        b[0..4].copy_from_slice(magic);
        b[4] = VERSION;
        b[5] = self.zoom;
        b[6] = self.source_id;
        b[8..10].copy_from_slice(&self.tile_px.to_le_bytes());
        b
    }

    pub fn decode(b: &[u8], magic: &[u8; 4]) -> Result<Self> {
        if &b[0..4] != magic {
            bail!("bad store magic {:?} (want {:?})", &b[0..4], magic);
        }
        if b[4] != VERSION {
            bail!("store version {} ≠ {}", b[4], VERSION);
        }
        Ok(Header {
            zoom: b[5],
            source_id: b[6],
            tile_px: u16::from_le_bytes(b[8..10].try_into().unwrap()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_round_trip() {
        let e = Entry {
            offset: 0x00DE_ADBE_EF00,
            len: 41_460,
            codec: TileCodec::ZstdCells,
        };
        let decoded = Entry::decode(&e.encode()).unwrap().unwrap();
        assert_eq!(decoded, e);
    }

    #[test]
    fn zero_entry_is_absent() {
        assert!(Entry::decode(&[0u8; 16]).unwrap().is_none());
    }

    #[test]
    fn header_round_trip_and_magic_check() {
        let h = Header {
            zoom: 12,
            source_id: 3,
            tile_px: 512,
        };
        let enc = h.encode(MAGIC_INDEX);
        assert_eq!(Header::decode(&enc, MAGIC_INDEX).unwrap(), h);
        assert!(Header::decode(&enc, MAGIC_DATA).is_err());
    }

    #[test]
    fn corrupt_codec_tag_errors() {
        let mut b = Entry {
            offset: 0,
            len: 1,
            codec: TileCodec::BrotliHm3,
        }
        .encode();
        b[12] = 9;
        assert!(Entry::decode(&b).is_err());
    }
}

//! A tile file as stored: its kind's contents as the kind's module writes and reads them, in one
//! zstd frame for the kinds whose records still repeat what a coder finds (terrain and the fine
//! terrain carry their own coder), written whole and renamed into place, read whole and decoded
//! once: a reader holds the contents only, the stored bytes go as they are decoded.

use crate::geo::TileId;
use crate::{Kind, tile_path};
use std::path::Path;

/// zstd level of a framed file, written once and read at every click: on an aircraft-far file of 3
/// MB, 17 stores 2.35x (19: 2.36x, 15: 2.17x) and decodes at 1.37 GB/s a core (19: 1.19, 15:
/// 1.9) at 11 MB/s of writing (19: 7).
const FRAME_LEVEL: i32 = 17;

/// The bytes a file of `kind` stores for `contents`.
pub fn stored(kind: Kind, contents: &[u8]) -> Vec<u8> {
    if kind.framed() {
        zstd::bulk::compress(contents, FRAME_LEVEL).expect("zstd compresses a buffer")
    } else {
        contents.to_vec()
    }
}

/// The contents of a file of `kind` from its stored bytes: a framed file is one zstd frame that
/// names its size.
pub fn contents(kind: Kind, stored: Vec<u8>) -> Result<Vec<u8>, String> {
    if !kind.framed() {
        return Ok(stored);
    }
    let size = match zstd::zstd_safe::get_frame_content_size(&stored) {
        Ok(Some(size)) if size <= u64::from(u32::MAX) => size as usize,
        _ => return Err(format!("{}: not one zstd frame of a size", kind.name())),
    };
    let contents = zstd::bulk::decompress(&stored, size)
        .map_err(|error| format!("{}: {error}", kind.name()))?;
    if contents.len() != size {
        return Err(format!("{}: the frame is not its stated size", kind.name()));
    }
    Ok(contents)
}

/// Writes one tile file atomically (a temporary name renamed into place).
pub fn write_tile(
    year_root: &Path,
    tile: TileId,
    kind: Kind,
    contents: &[u8],
) -> Result<(), String> {
    let path = tile_path(year_root, tile, kind);
    let directory = path.parent().expect("tile paths have a directory");
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("{}: {error}", directory.display()))?;
    let temporary = path.with_extension(format!("{}.partial", kind.name()));
    std::fs::write(&temporary, stored(kind, contents))
        .map_err(|error| format!("{}: {error}", temporary.display()))?;
    std::fs::rename(&temporary, &path).map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A framed kind's file is one frame of its contents and reads back to them; an unframed
    /// kind's is its contents; a framed kind's file that is not a frame is refused.
    #[test]
    fn contents_round_trip_through_what_a_file_stores() {
        let bytes: Vec<u8> = (0..10_000u32).flat_map(|k| (k % 7).to_le_bytes()).collect();
        let framed = stored(Kind::Aircraft, &bytes);
        assert!(framed.len() < bytes.len() / 4);
        assert_eq!(contents(Kind::Aircraft, framed).unwrap(), bytes);
        assert_eq!(stored(Kind::Terrain, &bytes), bytes);
        assert_eq!(contents(Kind::Terrain, bytes.clone()).unwrap(), bytes);
        assert!(contents(Kind::Sources, bytes.clone()).is_err());
    }
}

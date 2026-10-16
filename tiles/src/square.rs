//! z9 squares, the unit a builder builds and a file holds: `<year root>/<x9>/<y9>/<kind>` keeps a
//! square's 64 z12 tiles of one kind behind a directory of their offsets, a square's kinds side by
//! side (copied square by square, they lie together on a disk, and a cutout of the world is a set of
//! square directories). A read takes
//! exactly the tiles asked for, each with one `pread` of its bytes after its square's directory,
//! every read at once (one reader a read, so the disk sees the requests in parallel); a write
//! streams a square's tiles in its order into a temporary file and renames it into place. One file a square and kind keeps a year at 377,082 files where
//! one a tile and kind made 17,554,074 (r069), and a kind rebuilt rewrites only its own files.
//!
//! ```text
//! magic "qmsq1\n\0\0", u32 x9, u32 y9
//! u64 offsets[65]: tile i (row-major from the north-west) is the bytes [offsets[i],
//!   offsets[i + 1]); equal offsets: the tile has nothing of the kind
//! the tiles' bytes in that order, each in its kind's own format
//! ```

use crate::Kind;
use crate::geo::TileId;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// z9 squares per axis.
pub const SQUARES_PER_AXIS: u32 = 512;
/// z12 tiles per square side.
pub const TILES_PER_SIDE: u32 = 8;
/// z12 tiles a square holds.
pub const TILES_PER_SQUARE: usize = (TILES_PER_SIDE * TILES_PER_SIDE) as usize;
const MAGIC: &[u8; 8] = b"qmsq1\n\0\0";
const HEADER_BYTES: usize = 16 + 8 * (TILES_PER_SQUARE + 1);

/// A z9 square in the XYZ numbering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Square {
    pub x: u32,
    pub y: u32,
}

impl Square {
    pub fn of(tile: TileId) -> Square {
        Square {
            x: tile.x / TILES_PER_SIDE,
            y: tile.y / TILES_PER_SIDE,
        }
    }

    /// Tile `index` of the square, row-major from the north-west.
    pub fn tile(self, index: usize) -> TileId {
        let index = index as u32;
        TileId {
            x: self.x * TILES_PER_SIDE + index % TILES_PER_SIDE,
            y: self.y * TILES_PER_SIDE + index / TILES_PER_SIDE,
        }
    }

    /// The index of one of the square's tiles ([`Square::tile`]).
    pub fn index_of(self, tile: TileId) -> usize {
        debug_assert_eq!(Square::of(tile), self);
        ((tile.y % TILES_PER_SIDE) * TILES_PER_SIDE + tile.x % TILES_PER_SIDE) as usize
    }

    /// The square's tiles, row-major from the north-west.
    pub fn tiles(self) -> impl Iterator<Item = TileId> {
        (0..TILES_PER_SQUARE).map(move |index| self.tile(index))
    }

    /// The square of a z30 cell (x east from 180 W, y north from the equator at 2^29).
    pub fn of_z30(gx: i32, gy: i32) -> Square {
        let per_square = (1i64 << 30) / i64::from(SQUARES_PER_AXIS);
        let last = i64::from(SQUARES_PER_AXIS) - 1;
        Square {
            x: (i64::from(gx) / per_square).clamp(0, last) as u32,
            y: ((((1i64 << 30) - 1) - i64::from(gy)) / per_square).clamp(0, last) as u32,
        }
    }

    /// The square and its existing neighbours (x wraps, y stops at the poles).
    pub fn with_neighbours(self) -> Vec<Square> {
        let mut squares = Vec::new();
        for dy in -1i64..=1 {
            for dx in -1i64..=1 {
                let y = i64::from(self.y) + dy;
                if !(0..i64::from(SQUARES_PER_AXIS)).contains(&y) {
                    continue;
                }
                let x = (i64::from(self.x) + dx).rem_euclid(i64::from(SQUARES_PER_AXIS));
                squares.push(Square {
                    x: x as u32,
                    y: y as u32,
                });
            }
        }
        squares
    }
}

/// `<year root>/<x9>/<y9>/<kind>`.
pub fn square_path(year_root: &Path, kind: Kind, square: Square) -> PathBuf {
    year_root
        .join(square.x.to_string())
        .join(square.y.to_string())
        .join(kind.name())
}

/// An open square file and its tiles' offsets.
struct SquareFile {
    file: File,
    offsets: [u64; TILES_PER_SQUARE + 1],
}

/// Opens a square's file of a kind and reads its directory: `None` where the square has none (in
/// a complete release: nothing of the kind there); a file whose directory is not its own square's
/// or does not span its bytes is an error.
fn open(year_root: &Path, kind: Kind, square: Square) -> Result<Option<SquareFile>, String> {
    let path = square_path(year_root, kind, square);
    let fail = |what: String| format!("{}: {what}", path.display());
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(fail(error.to_string())),
    };
    let length = file
        .metadata()
        .map_err(|error| fail(error.to_string()))?
        .len();
    let mut header = [0u8; HEADER_BYTES];
    file.read_exact_at(&mut header, 0)
        .map_err(|_| fail("the directory is cut".into()))?;
    let word = |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().unwrap());
    if &header[..8] != MAGIC || (word(8), word(12)) != (square.x, square.y) {
        return Err(fail("not this square's file".into()));
    }
    let mut offsets = [0u64; TILES_PER_SQUARE + 1];
    for (index, offset) in offsets.iter_mut().enumerate() {
        let at = 16 + 8 * index;
        *offset = u64::from_le_bytes(header[at..at + 8].try_into().unwrap());
    }
    if offsets[0] != HEADER_BYTES as u64
        || offsets.windows(2).any(|pair| pair[0] > pair[1])
        || offsets[TILES_PER_SQUARE] != length
    {
        return Err(fail("its directory does not span its bytes".into()));
    }
    Ok(Some(SquareFile { file, offsets }))
}

/// Reads the tiles asked for, each whole: per request, in their order, its bytes, `None` where its
/// square has no file of the kind or no bytes for the tile (empty only in a complete release).
/// Each square file reads its directory, then its tiles, every file and tile at once; any other
/// failure is an error, never a quieter answer.
pub fn read_tiles(
    year_root: &Path,
    requests: &[(TileId, Kind)],
) -> Result<Vec<Option<Vec<u8>>>, String> {
    let mut by_file: HashMap<(Kind, Square), Vec<usize>> = HashMap::new();
    for (at, &(tile, kind)) in requests.iter().enumerate() {
        by_file
            .entry((kind, Square::of(tile)))
            .or_default()
            .push(at);
    }
    let by_file: Vec<((Kind, Square), Vec<usize>)> = by_file.into_iter().collect();
    let read: Vec<Vec<(usize, Option<Vec<u8>>)>> = by_file
        .into_par_iter()
        .with_max_len(1)
        .map(|((kind, square), ats)| {
            let Some(file) = open(year_root, kind, square)? else {
                return Ok(ats.into_iter().map(|at| (at, None)).collect());
            };
            ats.into_par_iter()
                .with_max_len(1)
                .map(|at| {
                    let index = square.index_of(requests[at].0);
                    let (start, end) = (file.offsets[index], file.offsets[index + 1]);
                    if start == end {
                        return Ok((at, None));
                    }
                    let mut bytes = vec![0u8; (end - start) as usize];
                    file.file
                        .read_exact_at(&mut bytes, start)
                        .map_err(|error| {
                            format!(
                                "{}: {error}",
                                square_path(year_root, kind, square).display()
                            )
                        })?;
                    Ok((at, Some(bytes)))
                })
                .collect()
        })
        .collect::<Result<_, String>>()?;
    let mut tiles: Vec<Option<Vec<u8>>> = (0..requests.len()).map(|_| None).collect();
    for (at, bytes) in read.into_iter().flatten() {
        tiles[at] = bytes;
    }
    Ok(tiles)
}

/// One tile's bytes ([`read_tiles`]).
pub fn read_tile(year_root: &Path, tile: TileId, kind: Kind) -> Result<Option<Vec<u8>>, String> {
    Ok(read_tiles(year_root, &[(tile, kind)])?.pop().flatten())
}

/// The tiles among `tiles` holding anything of a kind, with their bytes ([`read_tiles`]).
pub fn read_held(
    year_root: &Path,
    kind: Kind,
    tiles: impl IntoIterator<Item = TileId>,
) -> Result<Vec<(TileId, Vec<u8>)>, String> {
    let requests: Vec<(TileId, Kind)> = tiles.into_iter().map(|tile| (tile, kind)).collect();
    let read = read_tiles(year_root, &requests)?;
    Ok(requests
        .into_iter()
        .zip(read)
        .filter_map(|((tile, _), bytes)| Some((tile, bytes?)))
        .collect())
}

/// Which of a square's tiles hold anything of a kind (none where the square has no file of it).
pub fn tiles_held(
    year_root: &Path,
    kind: Kind,
    square: Square,
) -> Result<[bool; TILES_PER_SQUARE], String> {
    let mut held = [false; TILES_PER_SQUARE];
    if let Some(file) = open(year_root, kind, square)? {
        for (index, held) in held.iter_mut().enumerate() {
            *held = file.offsets[index] < file.offsets[index + 1];
        }
    }
    Ok(held)
}

/// A square's file of a kind being written: its tiles added in the square's order and streamed to
/// a temporary file of its own; when finished, the directory written, the file synced, renamed into
/// place and its directory synced, so a square reported written survives a crash. A writer that
/// fails or is dropped unfinished removes its temporary file.
pub struct SquareWriter {
    square: Square,
    path: PathBuf,
    temporary: PathBuf,
    file: File,
    offsets: [u64; TILES_PER_SQUARE + 1],
    /// The first index a tile may still take.
    next: usize,
    tiles: usize,
    /// The temporary file is gone: renamed into place or removed.
    settled: bool,
}

impl SquareWriter {
    pub fn create(year_root: &Path, kind: Kind, square: Square) -> Result<Self, String> {
        // Unique within the machine: two writers of one square never share a temporary file.
        static WRITERS: AtomicU64 = AtomicU64::new(0);
        let path = square_path(year_root, kind, square);
        let directory = path.parent().expect("a square path has a directory");
        std::fs::create_dir_all(directory).map_err(|error| failed(directory, error))?;
        let temporary = path.with_file_name(format!(
            "{}.{}-{}.partial",
            kind.name(),
            std::process::id(),
            WRITERS.fetch_add(1, Ordering::Relaxed)
        ));
        let file = File::options()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| failed(&temporary, error))?;
        let mut writer = SquareWriter {
            square,
            path,
            temporary,
            file,
            offsets: [HEADER_BYTES as u64; TILES_PER_SQUARE + 1],
            next: 0,
            tiles: 0,
            settled: false,
        };
        writer
            .file
            .write_all(&[0; HEADER_BYTES])
            .map_err(|error| failed(&writer.temporary, error))?;
        Ok(writer)
    }

    /// Appends a tile of the square after every tile added before it in the square's order.
    pub fn add(&mut self, tile: TileId, bytes: &[u8]) -> Result<(), String> {
        assert_eq!(
            Square::of(tile),
            self.square,
            "{tile:?} outside {:?}",
            self.square
        );
        let index = self.square.index_of(tile);
        assert!(
            index >= self.next,
            "{tile:?} again or after a later tile of its square"
        );
        let start = self.offsets[self.next];
        self.offsets[self.next..=index].fill(start);
        self.file
            .write_all(bytes)
            .map_err(|error| failed(&self.temporary, error))?;
        self.offsets[index + 1] = start + bytes.len() as u64;
        self.next = index + 1;
        self.tiles += usize::from(!bytes.is_empty());
        Ok(())
    }

    /// Puts the file in place and returns its tiles; a square without any gets no file (one left
    /// from before is removed).
    pub fn finish(mut self) -> Result<usize, String> {
        let end = self.offsets[self.next];
        self.offsets[self.next..].fill(end);
        let directory = self.path.parent().expect("a square path has a directory");
        if self.tiles == 0 {
            std::fs::remove_file(&self.temporary)
                .map_err(|error| failed(&self.temporary, error))?;
            self.settled = true;
            match std::fs::remove_file(&self.path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
                Err(error) => return Err(failed(&self.path, error)),
            }
            sync_directory(directory)?;
            return Ok(0);
        }
        let mut header = Vec::with_capacity(HEADER_BYTES);
        header.extend_from_slice(MAGIC);
        header.extend_from_slice(&self.square.x.to_le_bytes());
        header.extend_from_slice(&self.square.y.to_le_bytes());
        for offset in self.offsets {
            header.extend_from_slice(&offset.to_le_bytes());
        }
        self.file
            .write_all_at(&header, 0)
            .and_then(|()| self.file.sync_all())
            .map_err(|error| failed(&self.temporary, error))?;
        std::fs::rename(&self.temporary, &self.path).map_err(|error| failed(&self.path, error))?;
        self.settled = true;
        sync_directory(directory)?;
        Ok(self.tiles)
    }
}

impl Drop for SquareWriter {
    fn drop(&mut self) {
        if !self.settled {
            let _ = std::fs::remove_file(&self.temporary);
        }
    }
}

fn sync_directory(directory: &Path) -> Result<(), String> {
    File::open(directory)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| failed(directory, error))
}

fn failed(path: &Path, error: std::io::Error) -> String {
    format!("{}: {error}", path.display())
}

/// Writes a square's tiles of a kind, any order, as its file ([`SquareWriter`]); returns them.
pub fn write_square(
    year_root: &Path,
    kind: Kind,
    square: Square,
    tiles: &[(TileId, Vec<u8>)],
) -> Result<usize, String> {
    let mut ordered: Vec<&(TileId, Vec<u8>)> = tiles.iter().collect();
    ordered.sort_unstable_by_key(|(tile, _)| (tile.y, tile.x));
    let mut writer = SquareWriter::create(year_root, kind, square)?;
    for (tile, bytes) in ordered {
        writer.add(*tile, bytes)?;
    }
    writer.finish()
}

/// Writes tiles of a kind, any squares, each square's file once ([`write_square`]): a square's
/// tiles come in one call, as its file holds them all.
pub fn write_tiles(
    year_root: &Path,
    kind: Kind,
    tiles: &[(TileId, Vec<u8>)],
) -> Result<(), String> {
    let mut by_square: HashMap<Square, Vec<(TileId, Vec<u8>)>> = HashMap::new();
    for (tile, bytes) in tiles {
        by_square
            .entry(Square::of(*tile))
            .or_default()
            .push((*tile, bytes.clone()));
    }
    by_square
        .iter()
        .try_for_each(|(&square, tiles)| write_square(year_root, kind, square, tiles).map(|_| ()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("qm-square-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn prague_lies_in_its_square_at_its_index() {
        let tile = TileId { x: 2212, y: 1387 };
        let square = Square::of(tile);
        assert_eq!(square, Square { x: 276, y: 173 });
        assert_eq!(square.index_of(tile), 3 * 8 + 4);
        assert_eq!(square.tile(square.index_of(tile)), tile);
        assert_eq!(
            square.tiles().filter(|&t| Square::of(t) == square).count(),
            64
        );
    }

    /// Every tile comes back as written, empty and absent ones as `None`, from one square's file
    /// and another's, in the order asked.
    #[test]
    fn tiles_read_back_as_written() {
        let root = root("round-trip");
        let (here, there) = (Square { x: 276, y: 173 }, Square { x: 0, y: 511 });
        let tiles: Vec<(TileId, Vec<u8>)> = [0, 5, 6, 63]
            .iter()
            .map(|&index| (here.tile(index), vec![index as u8; 3 + index]))
            .collect();
        write_square(&root, Kind::Sources, here, &tiles).unwrap();
        write_square(
            &root,
            Kind::Sources,
            there,
            &[(there.tile(9), b"far".to_vec())],
        )
        .unwrap();
        write_square(&root, Kind::Terrain, here, &[]).unwrap();
        let requests = [
            (here.tile(63), Kind::Sources),
            (here.tile(1), Kind::Sources),
            (there.tile(9), Kind::Sources),
            (here.tile(0), Kind::Sources),
            (here.tile(0), Kind::Terrain),
        ];
        let read = read_tiles(&root, &requests).unwrap();
        assert_eq!(read[0].as_deref(), Some(&[63u8; 66][..]));
        assert_eq!(read[1], None);
        assert_eq!(read[2].as_deref(), Some(&b"far"[..]));
        assert_eq!(read[3].as_deref(), Some(&[0u8; 3][..]));
        assert_eq!(read[4], None);
        assert_eq!(
            read_tile(&root, here.tile(5), Kind::Sources).unwrap(),
            Some(vec![5; 8])
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A square written empty removes the file it had; a writer dropped unfinished leaves nothing.
    #[test]
    fn an_empty_square_leaves_no_file() {
        let root = root("empty");
        let square = Square { x: 3, y: 4 };
        write_square(
            &root,
            Kind::Terrain,
            square,
            &[(square.tile(1), vec![9; 5])],
        )
        .unwrap();
        let path = square_path(&root, Kind::Terrain, square);
        assert!(path.exists());
        assert_eq!(write_square(&root, Kind::Terrain, square, &[]).unwrap(), 0);
        assert!(!path.exists());
        let mut writer = SquareWriter::create(&root, Kind::Terrain, square).unwrap();
        writer.add(square.tile(2), b"abc").unwrap();
        drop(writer);
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            0
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    #[should_panic(expected = "after a later tile")]
    fn tiles_come_in_the_squares_order() {
        let root = root("order");
        let square = Square { x: 5, y: 6 };
        let mut writer = SquareWriter::create(&root, Kind::Terrain, square).unwrap();
        writer.add(square.tile(9), b"a").unwrap();
        writer.add(square.tile(8), b"b").unwrap();
    }

    /// A cut file and another square's file under this square's name are errors.
    #[test]
    fn a_file_not_matching_its_directory_is_an_error() {
        let root = root("bad");
        let square = Square { x: 1, y: 2 };
        write_square(
            &root,
            Kind::Obstacles,
            square,
            &[(square.tile(7), vec![1; 100])],
        )
        .unwrap();
        let path = square_path(&root, Kind::Obstacles, square);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() - 1]).unwrap();
        assert!(read_tile(&root, square.tile(7), Kind::Obstacles).is_err());
        let other = Square { x: 2, y: 2 };
        let other_path = square_path(&root, Kind::Obstacles, other);
        std::fs::create_dir_all(other_path.parent().unwrap()).unwrap();
        std::fs::write(&other_path, &bytes).unwrap();
        assert!(read_tile(&root, other.tile(0), Kind::Obstacles).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }
}

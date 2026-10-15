//! Whole-file reads: every file of a ring is read whole, all files at once, one reader per file,
//! so the disk sees large requests in parallel (measured cold: 26 MB in a tenth to a fifth of the
//! time of mmap with MADV_WILLNEED and page touching, whose faults read 32 KB at a time), and each
//! decoded to its contents by the reader that read it ([`crate::stored`]).

use crate::Kind;
use crate::stored::contents;
use rayon::prelude::*;
use std::io::Read;
use std::path::{Path, PathBuf};

/// A tile file read: its contents and the bytes it took from the disk.
#[derive(Debug, PartialEq)]
pub struct TileFile {
    pub contents: Vec<u8>,
    pub stored_bytes: u64,
}

/// Reads one file of `kind` whole and decodes it. An absent file is `None` (empty only in a
/// complete release); any other failure is an error, never a quieter answer.
pub fn read_tile(path: &Path, kind: Kind) -> Result<Option<TileFile>, String> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let length = file
        .metadata()
        .map_err(|error| format!("{}: {error}", path.display()))?
        .len();
    let mut bytes = Vec::with_capacity(length as usize);
    file.read_to_end(&mut bytes)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    contents(kind, bytes)
        .map(|contents| {
            Some(TileFile {
                contents,
                stored_bytes: length,
            })
        })
        .map_err(|error| format!("{}: {error}", path.display()))
}

/// Reads every file whole and at once, each decoded as it is read ([`read_tile`]).
pub fn read_all(files: &[(PathBuf, Kind)]) -> Result<Vec<Option<TileFile>>, String> {
    files
        .par_iter()
        .with_max_len(1)
        .map(|(path, kind)| read_tile(path, *kind))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_files_are_none_and_present_ones_read_whole() {
        let directory = std::env::temp_dir().join(format!("qm-read-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let (full, empty, absent) = (
            directory.join("full"),
            directory.join("empty"),
            directory.join("absent"),
        );
        std::fs::write(&full, b"tile bytes").unwrap();
        std::fs::write(&empty, b"").unwrap();
        let terrain = |path: &PathBuf| (path.clone(), Kind::Terrain);
        let read = read_all(&[terrain(&full), terrain(&empty), terrain(&absent)]).unwrap();
        let file = |contents: &[u8], stored_bytes| {
            Some(TileFile {
                contents: contents.to_vec(),
                stored_bytes,
            })
        };
        assert_eq!(read[0], file(b"tile bytes", 10));
        assert_eq!(read[1], file(b"", 0));
        assert!(read[2].is_none());
        // A framed kind's file reads as its contents; one that is not a frame is an error.
        let framed = directory.join("framed");
        let stored = crate::stored::stored(Kind::Sources, b"tile bytes");
        std::fs::write(&framed, &stored).unwrap();
        assert_eq!(
            read_tile(&framed, Kind::Sources).unwrap(),
            file(b"tile bytes", stored.len() as u64)
        );
        assert!(read_tile(&full, Kind::Sources).is_err());
        std::fs::remove_dir_all(&directory).unwrap();
        let missing = directory.join("missing-dir").join("x");
        assert!(read_all(&[terrain(&missing)]).unwrap()[0].is_none());
    }
}

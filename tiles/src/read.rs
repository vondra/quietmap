//! Whole-file reads: every file of a ring is read whole, all files at once, one reader per file,
//! so the disk sees large requests in parallel (measured cold on NVMe: 26 MB in 3-5 ms against
//! 23-29 ms for mmap with MADV_WILLNEED and page touching, whose faults read 32 KB at a time).

use rayon::prelude::*;
use std::io::Read;
use std::path::PathBuf;

/// Reads every path whole and at once. An absent file is `None` (empty only in a complete
/// release); any other failure is an error, never a quieter answer.
pub fn read_all(paths: &[PathBuf]) -> Result<Vec<Option<Vec<u8>>>, String> {
    paths
        .par_iter()
        .with_max_len(1)
        .map(|path| {
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
            Ok(Some(bytes))
        })
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
        let read = read_all(&[full, empty, absent]).unwrap();
        assert_eq!(read[0].as_deref(), Some(&b"tile bytes"[..]));
        assert_eq!(read[1].as_deref(), Some(&b""[..]));
        assert!(read[2].is_none());
        std::fs::remove_dir_all(&directory).unwrap();
        assert!(read_all(&[directory.join("missing-dir").join("x")]).unwrap()[0].is_none());
    }
}

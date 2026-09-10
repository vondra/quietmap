//! Read-only manifest pins every prepared source file to immutable Arrow bytes.
use anyhow::{ensure, Context, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use sha2::{Digest, Sha256};
use std::path::Path;
pub use tile_painter::generation_receipt::file_digest;

pub struct InputManifest {
    connection: Connection,
    pub digest: [u8; 32],
}

impl InputManifest {
    /// Schema: input_files(relative_path TEXT PRIMARY KEY, sha256 BLOB NOT NULL).
    /// Paths are relative to the prepared year; omitted paths assert no source file.
    pub fn open(path: &Path, expected: [u8; 32]) -> Result<Self> {
        ensure!(
            file_digest(path)? == expected,
            "input manifest digest mismatch"
        );
        Ok(Self {
            connection: Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?,
            digest: expected,
        })
    }

    pub fn read_arrow(&self, root: &Path, relative: &str) -> Result<Option<(Vec<u8>, [u8; 32])>> {
        ensure!(
            !relative.starts_with('/') && !relative.split('/').any(|part| part == ".."),
            "invalid manifest path"
        );
        let expected: Option<Vec<u8>> = self
            .connection
            .query_row(
                "SELECT sha256 FROM input_files WHERE relative_path=?1",
                [relative],
                |row| row.get(0),
            )
            .optional()?;
        let path = root.join(relative);
        let Some(expected) = expected else {
            ensure!(!path.try_exists()?, "unmanifested source file: {relative}");
            return Ok(None);
        };
        let digest: [u8; 32] = expected
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid source digest for {relative}"))?;
        let bytes = std::fs::read(&path).with_context(|| format!("read {relative}"))?;
        ensure!(
            <[u8; 32]>::from(Sha256::digest(&bytes)) == digest,
            "source digest mismatch: {relative}"
        );
        Ok(Some((bytes, digest)))
    }
}
pub fn parse_digest(value: &str) -> Result<[u8; 32]> {
    ensure!(
        value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "expected SHA256"
    );
    let mut bytes = [0; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)?;
    }
    Ok(bytes)
}

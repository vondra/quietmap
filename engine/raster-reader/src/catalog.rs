//! SQLite publication authority: an absent row is unknown, never inferred ocean.

use crate::channel::{Channel, CONTRACT};
use grid::{square_from_id, square_id, Square};
use rusqlite::{params, Connection, OpenFlags};
use std::collections::HashMap;
use std::path::Path;

pub const CATALOG_FILE: &str = "rasters.sqlite";
pub type Digest = [u8; 32];
pub type Coverage = HashMap<Square, Option<Digest>>;

pub fn content_digest(bytes: &[u8]) -> Digest {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes).into()
}

pub fn read_channel(root: &Path, channel: Channel) -> Result<Coverage, String> {
    let database =
        Connection::open_with_flags(root.join(CATALOG_FILE), OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| format!("raster catalog: {error}"))?;
    let (contract, source_identity): (String, String) = database
        .query_row(
            "SELECT contract, source_identity FROM raster_channels WHERE channel = ?1",
            [channel.name()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| format!("{} channel unavailable: {error}", channel.name()))?;
    if contract != CONTRACT || !valid_identity(&source_identity) {
        return Err(format!("{} channel requires {CONTRACT}", channel.name()));
    }
    let mut statement = database
        .prepare("SELECT square, sha256 FROM raster_squares WHERE channel = ?1")
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([channel.name()], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<Vec<u8>>>(1)?))
        })
        .map_err(|error| error.to_string())?;
    let mut coverage = Coverage::new();
    for row in rows {
        let (id, digest) = row.map_err(|error| error.to_string())?;
        let square = square_from_id(id).ok_or_else(|| format!("invalid raster square {id}"))?;
        let digest = digest
            .map(|bytes| {
                bytes
                    .try_into()
                    .map_err(|_| "invalid raster SHA256".to_string())
            })
            .transpose()?;
        if coverage.insert(square, digest).is_some() {
            return Err(format!("duplicate raster square {id}"));
        }
    }
    Ok(coverage)
}

fn valid_identity(identity: &str) -> bool {
    identity.len() == 64 && identity.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The caller binds source identity before any output and cannot silently mix source generations.
pub fn begin_channel(
    root: &Path,
    channel: Channel,
    source_identity: &str,
) -> Result<Connection, String> {
    if !valid_identity(source_identity) {
        return Err("audited source generation identity must be a SHA256".into());
    }
    std::fs::create_dir_all(root).map_err(|error| error.to_string())?;
    let database = Connection::open(root.join(CATALOG_FILE)).map_err(|error| error.to_string())?;
    database
        .execute_batch(
            "PRAGMA foreign_keys = ON;
        CREATE TABLE IF NOT EXISTS raster_channels (
            channel TEXT PRIMARY KEY CHECK(channel IN ('dem','forest','imd')),
            contract TEXT NOT NULL, source_identity TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS raster_squares (
            channel TEXT NOT NULL REFERENCES raster_channels(channel),
            square INTEGER NOT NULL CHECK(square BETWEEN 0 AND 262143),
            sha256 BLOB CHECK(sha256 IS NULL OR length(sha256) = 32),
            PRIMARY KEY(channel, square));",
        )
        .map_err(|error| error.to_string())?;
    database
        .execute(
            "INSERT OR IGNORE INTO raster_channels VALUES (?1, ?2, ?3)",
            params![channel.name(), CONTRACT, source_identity],
        )
        .map_err(|error| error.to_string())?;
    let actual: (String, String) = database
        .query_row(
            "SELECT contract, source_identity FROM raster_channels WHERE channel = ?1",
            [channel.name()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| error.to_string())?;
    if actual != (CONTRACT.into(), source_identity.into()) {
        return Err(format!(
            "{} source identity changed; use a new release root",
            channel.name()
        ));
    }
    Ok(database)
}

/// Publish only after the corresponding file is atomically complete, or its whole window proved empty.
pub fn record_square(
    database: &Connection,
    channel: Channel,
    square: Square,
    digest: Option<Digest>,
) -> Result<(), String> {
    database
        .execute(
            "INSERT INTO raster_squares VALUES (?1, ?2, ?3)
        ON CONFLICT(channel, square) DO UPDATE SET sha256 = excluded.sha256",
            params![
                channel.name(),
                square_id(square),
                digest.as_ref().map(|bytes| bytes.as_slice())
            ],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

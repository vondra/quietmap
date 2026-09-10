//! A completed raw-spill boundary binds executable, immutable inputs and sampling window.

use super::*;
use rusqlite::{params, Connection, OpenFlags};
use sha2::{Digest, Sha256};
use std::io::Read;

pub(super) fn input_identities(paths: &[PathBuf]) -> Result<Vec<(String, String)>> {
    let mut result = paths
        .iter()
        .map(|path| {
            let canonical = path.canonicalize()?;
            Ok((
                canonical
                    .to_str()
                    .context("non-UTF8 cruise input path")?
                    .to_owned(),
                census::identity(&canonical)?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    result.sort_unstable();
    Ok(result)
}

fn visit_raw_parts(
    directory: &Path,
    mut visit: impl FnMut(String, String) -> Result<()>,
) -> Result<u64> {
    let mut count = 0;
    for bucket in 0..SPILL_HASH_BUCKETS {
        for path in list_spill_parts(&spill_bucket_dir(directory, bucket))? {
            visit(
                path.strip_prefix(directory)?
                    .to_str()
                    .context("non-UTF8 spill part")?
                    .to_owned(),
                census::identity(&path)?,
            )?;
            count += 1;
        }
    }
    Ok(count)
}

pub(crate) fn receipt_page_limit(parts: u64, inputs: usize, page_size: u64) -> u64 {
    // Enforced storage budget: one KiB per inventory row, plus four pages per
    // input and eight schema/state entries. Actual records need far less.
    (parts * 1024).div_ceil(page_size) + 4 * (inputs as u64 + 8)
}

fn executable_digest() -> Result<Vec<u8>> {
    static DIGEST: std::sync::OnceLock<std::result::Result<Vec<u8>, String>> =
        std::sync::OnceLock::new();
    DIGEST
        .get_or_init(|| hash_executable().map_err(|error| error.to_string()))
        .clone()
        .map_err(anyhow::Error::msg)
}

fn hash_executable() -> Result<Vec<u8>> {
    hash_file(&std::env::current_exe()?)
}

fn hash_file(path: &Path) -> Result<Vec<u8>> {
    let mut file = std::fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(digest.finalize().to_vec())
}

fn scope_key(scope: Option<&ScopeBbox>) -> String {
    scope
        .map(|value| {
            format!(
                "{}:{}:{}:{}",
                value.min_lat, value.min_lon, value.max_lat, value.max_lon
            )
        })
        .unwrap_or_default()
}

pub(super) fn create(
    directory: &Path,
    inputs: &[(String, String)],
    days: u16,
    scope: Option<&ScopeBbox>,
    ga_cruise: u64,
) -> Result<()> {
    let watermark = crate::arrow_io::spill_receipt_watermark(directory)?;
    let mut db = Connection::open(directory.join("state.sqlite"))?;
    db.execute_batch("PRAGMA page_size=4096; PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;
        CREATE TABLE state(phase TEXT NOT NULL, executable_sha256 BLOB NOT NULL, n_days INTEGER NOT NULL, scope TEXT NOT NULL, ga_cruise INTEGER NOT NULL);
        CREATE TABLE inputs(path TEXT PRIMARY KEY, stat_identity TEXT NOT NULL);
        CREATE TABLE raw_parts(path TEXT PRIMARY KEY, stat_identity TEXT NOT NULL);
        CREATE TABLE disk_reservation(start_free_bytes INTEGER NOT NULL, minimum_free_bytes INTEGER NOT NULL);
        CREATE TABLE allocation_plan(phase TEXT PRIMARY KEY, allocation_bytes INTEGER NOT NULL, input_bytes INTEGER NOT NULL);")?;
    let parts = visit_raw_parts(directory, |_, _| Ok(()))?;
    // Each raw file is synced by its writer; persist their directory entries
    // before committing the completed-spill receipt.
    for bucket in 0..SPILL_HASH_BUCKETS {
        std::fs::File::open(spill_bucket_dir(directory, bucket))?.sync_all()?;
    }
    let page_size: u64 = db.query_row("PRAGMA page_size", [], |row| row.get(0))?;
    db.pragma_update(
        None,
        "max_page_count",
        receipt_page_limit(parts, inputs.len(), page_size),
    )?;
    let transaction = db.transaction()?;
    transaction.execute(
        "INSERT INTO state VALUES ('spill-complete',?1,?2,?3,?4)",
        params![executable_digest()?, days, scope_key(scope), ga_cruise],
    )?;
    if let Some((start_free, minimum_free)) = watermark {
        transaction.execute(
            "INSERT INTO disk_reservation VALUES (?1,?2)",
            params![start_free, minimum_free],
        )?;
    }
    for (path, identity) in inputs {
        transaction.execute("INSERT INTO inputs VALUES (?1,?2)", params![path, identity])?;
    }
    visit_raw_parts(directory, |path, identity| {
        transaction.execute(
            "INSERT INTO raw_parts VALUES (?1,?2)",
            params![path, identity],
        )?;
        Ok(())
    })?;
    transaction.commit()?;
    std::fs::File::open(directory)?.sync_all()?;
    crate::arrow_io::spill_receipt_committed(directory)?;
    Ok(())
}

/// The spill receipt binds inputs, window and scope; the finish executable may
/// differ from the producer because the raw part schema is checked on read.
pub(super) fn verify(
    directory: &Path,
    inputs: &[(String, String)],
    days: u16,
    scope: Option<&ScopeBbox>,
    fail_on_ga: bool,
) -> Result<()> {
    let db = Connection::open_with_flags(
        directory.join("state.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let (phase, saved_days, saved_scope, ga): (String, u16, String, u64) = db.query_row(
        "SELECT phase,n_days,scope,ga_cruise FROM state",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    anyhow::ensure!(
        phase == "spill-complete",
        "cruise spill is {phase}; partial-fold resume is not supported"
    );
    anyhow::ensure!(
        saved_days == days && saved_scope == scope_key(scope),
        "cruise spill sampling window or scope differs"
    );
    anyhow::ensure!(
        !fail_on_ga || ga == 0,
        "GA-class cruise found in retained spill"
    );
    let saved = db
        .prepare("SELECT path,stat_identity FROM inputs ORDER BY path")?
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    anyhow::ensure!(
        saved == inputs,
        "cruise spill primary input identity differs"
    );
    let expected: u64 = db.query_row("SELECT COUNT(*) FROM raw_parts", [], |row| row.get(0))?;
    let mut lookup = db.prepare("SELECT stat_identity FROM raw_parts WHERE path=?1")?;
    let actual = visit_raw_parts(directory, |path, identity| {
        let saved: String = lookup
            .query_row([&path], |row| row.get(0))
            .context("cruise raw spill inventory differs")?;
        anyhow::ensure!(saved == identity, "cruise raw spill part identity differs");
        Ok(())
    })?;
    anyhow::ensure!(
        actual == expected,
        "cruise raw spill part inventory differs"
    );
    Ok(())
}

pub(super) fn begin_fold(directory: &Path) -> Result<()> {
    let db = Connection::open(directory.join("state.sqlite"))?;
    db.execute_batch("PRAGMA synchronous=FULL;")?;
    let changed = db.execute(
        "UPDATE state SET phase='folding' WHERE phase='spill-complete'",
        [],
    )?;
    anyhow::ensure!(changed == 1, "cruise spill already claimed by another fold");
    Ok(())
}

pub(super) fn record_fold_plan(
    directory: &Path,
    allocation: u64,
    input_bytes: u64,
    raw_allocated_bytes: u64,
) -> Result<()> {
    let db = Connection::open(directory.join("state.sqlite"))?;
    db.execute(
        "INSERT OR REPLACE INTO allocation_plan VALUES ('fold',?1,?2)",
        params![allocation, input_bytes],
    )?;
    db.execute(
        "INSERT OR REPLACE INTO allocation_plan VALUES ('raw-spill-blocks',0,?1)",
        [raw_allocated_bytes],
    )?;
    crate::arrow_io::spill_receipt_committed(directory)?;
    Ok(())
}

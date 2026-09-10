//! Durable shuffle successor inventory binds unchanged shards, windows and source receipts.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OpenFlags};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::{flight::Phase, scope::ScopeBbox};

type SourceReceipts = BTreeMap<(String, PathBuf), Vec<u8>>;
const INVENTORY: &str = "complete.sqlite";

fn digest(path: &Path) -> Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut bytes = [0; 65536];
    loop {
        let count = file.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    Ok(hash.finalize().to_vec())
}

fn scope_key(scope: Option<&ScopeBbox>) -> String {
    scope
        .map(|s| format!("{},{},{},{}", s.min_lat, s.max_lat, s.min_lon, s.max_lon))
        .unwrap_or_default()
}

fn identity(path: &Path) -> Result<[i64; 5]> {
    let stat = std::fs::symlink_metadata(path)?;
    anyhow::ensure!(
        stat.is_file(),
        "not a regular shuffle artifact: {}",
        path.display()
    );
    Ok([
        i64::try_from(stat.dev())?,
        i64::try_from(stat.ino())?,
        i64::try_from(stat.len())?,
        stat.mtime() * 1_000_000_000 + stat.mtime_nsec(),
        stat.ctime() * 1_000_000_000 + stat.ctime_nsec(),
    ])
}

pub(super) fn input_receipts(primary: &[PathBuf], ga: &[PathBuf]) -> Result<SourceReceipts> {
    let mut receipts = BTreeMap::new();
    for (window, paths) in [("days", primary), ("ga_days", ga)] {
        for path in paths {
            let work = path
                .parent()
                .and_then(Path::parent)
                .context("missing source work parent")?;
            let receipt = work.join("source-receipts.sqlite");
            if receipt.try_exists()? {
                let path = receipt.canonicalize()?;
                if !receipts.contains_key(&(window.to_owned(), path.clone())) {
                    receipts.insert((window.to_owned(), path), digest(&receipt)?);
                }
            }
        }
    }
    Ok(receipts)
}

fn files(root: &Path) -> Result<Vec<(u64, Phase, PathBuf)>> {
    let mut files = Vec::new();
    for (square, directory) in crate::spatial::square_directories(root)? {
        for (name, phase) in [
            ("airborne.arrow", Phase::Airborne),
            ("ground.arrow", Phase::Ground),
        ] {
            let path = directory.join(name);
            if path.try_exists()? || path.is_symlink() {
                files.push((square, phase, path));
            }
        }
    }
    Ok(files)
}

pub(super) fn publish(
    root: &Path,
    primary: &[PathBuf],
    ga: &[PathBuf],
    scope: Option<&ScopeBbox>,
    receipts: &SourceReceipts,
    expected_files: u64,
    expected_rows: impl Fn(Phase, u64) -> usize,
) -> Result<()> {
    anyhow::ensure!(
        !root
            .parent()
            .context("missing shuffle parent")?
            .join("temp_shuffle")
            .try_exists()?,
        "shuffle scatter is not retired"
    );
    for (name, paths) in [("days", primary), ("ga_days", ga)] {
        let mut days: Vec<_> = paths
            .iter()
            .map(|p| p.file_stem().unwrap().to_str().unwrap())
            .collect();
        days.sort_unstable();
        let mut file = File::create(root.join(name))?;
        file.write_all(days.join("\n").as_bytes())?;
        file.sync_all()?;
    }
    let path = root.join(INVENTORY);
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let mut db = Connection::open(&path)?;
    db.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;
        CREATE TABLE state(scope TEXT NOT NULL, days_sha256 BLOB NOT NULL, ga_days_sha256 BLOB NOT NULL);
        CREATE TABLE files(path TEXT PRIMARY KEY, rows INTEGER NOT NULL, dev INTEGER, ino INTEGER, size INTEGER, mtime_ns INTEGER, ctime_ns INTEGER);
        CREATE TABLE source_receipts(window TEXT, path TEXT, sha256 BLOB, PRIMARY KEY(window,path));")?;
    let tx = db.transaction()?;
    let artifacts = files(root)?;
    anyhow::ensure!(
        artifacts.len() as u64 == expected_files,
        "shuffle output inventory count differs from gather"
    );
    let mut directories = BTreeSet::new();
    for (square, phase, path) in artifacts {
        let rows = expected_rows(phase, square);
        anyhow::ensure!(
            rows > 0,
            "unexpected shuffle destination: {}",
            path.display()
        );
        let [dev, ino, size, mtime, ctime] = identity(&path)?;
        tx.execute(
            "INSERT INTO files VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                path.strip_prefix(root)?.to_str(),
                rows,
                dev,
                ino,
                size,
                mtime,
                ctime
            ],
        )?;
        let mut parent = path.parent();
        while let Some(directory) = parent.filter(|p| p.starts_with(root)) {
            directories.insert(directory.to_path_buf());
            parent = directory.parent();
        }
    }
    for ((window, receipt), expected) in receipts {
        anyhow::ensure!(
            digest(receipt)? == *expected,
            "source receipt changed during shuffle"
        );
        tx.execute(
            "INSERT INTO source_receipts VALUES (?1,?2,?3)",
            params![window, receipt.to_str(), expected],
        )?;
    }
    for directory in directories.iter().rev() {
        File::open(directory)?.sync_all()?;
    }
    tx.execute(
        "INSERT INTO state VALUES (?1,?2,?3)",
        params![
            scope_key(scope),
            digest(&root.join("days"))?,
            digest(&root.join("ga_days"))?
        ],
    )?;
    tx.commit()?;
    drop(db);
    File::open(path)?.sync_all()?;
    File::open(root)?.sync_all()?;
    File::open(root.parent().context("missing shuffle parent")?)?.sync_all()?;
    Ok(())
}

/// Read-only reuse gate; a running, partial or modified successor cannot replace its day inputs.
pub fn validate(root: &Path, scope: Option<&ScopeBbox>) -> Result<()> {
    anyhow::ensure!(
        !root
            .parent()
            .context("missing shuffle parent")?
            .join("temp_shuffle")
            .try_exists()?,
        "shuffle is incomplete: scatter directory exists"
    );
    let db = Connection::open_with_flags(root.join(INVENTORY), OpenFlags::SQLITE_OPEN_READ_ONLY)
        .context("missing durable shuffle inventory; verified successor required")?;
    let (saved_scope, days, ga): (String, Vec<u8>, Vec<u8>) = db.query_row(
        "SELECT scope,days_sha256,ga_days_sha256 FROM state",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    anyhow::ensure!(saved_scope == scope_key(scope), "shuffle scope differs");
    anyhow::ensure!(
        digest(&root.join("days"))? == days && digest(&root.join("ga_days"))? == ga,
        "shuffle sampling manifests changed"
    );
    let artifacts = files(root)?;
    let expected: u64 = db.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
    anyhow::ensure!(
        artifacts.len() as u64 == expected,
        "shuffle shard set changed"
    );
    for (_, _, path) in artifacts {
        let saved: [i64; 5] = db.query_row(
            "SELECT dev,ino,size,mtime_ns,ctime_ns FROM files WHERE path=?1",
            [path.strip_prefix(root)?.to_str()],
            |r| Ok([r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?]),
        )?;
        anyhow::ensure!(
            identity(&path)? == saved,
            "shuffle shard changed: {}",
            path.display()
        );
    }
    for (_, path, expected) in source_receipts(root)? {
        anyhow::ensure!(
            digest(&path)? == expected,
            "shuffle source receipt changed: {}",
            path.display()
        );
    }
    Ok(())
}

pub fn source_receipts(root: &Path) -> Result<Vec<(String, PathBuf, Vec<u8>)>> {
    let db = Connection::open_with_flags(root.join(INVENTORY), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let rows = db
        .prepare("SELECT window,path,sha256 FROM source_receipts ORDER BY window,path")?
        .query_map([], |r| {
            Ok((r.get(0)?, PathBuf::from(r.get::<_, String>(1)?), r.get(2)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// Expected file and segment counts for one phase in a sealed shuffle.
pub fn inventory_counts(root: &Path, phase: Phase) -> Result<(u64, u64)> {
    let filename = match phase {
        Phase::Airborne => "airborne.arrow",
        Phase::Ground => "ground.arrow",
        other => anyhow::bail!("shuffle has no {other:?} phase"),
    };
    let db = Connection::open_with_flags(root.join(INVENTORY), OpenFlags::SQLITE_OPEN_READ_ONLY)
        .context("missing durable shuffle inventory; verified successor required")?;
    let mut statement = db.prepare("SELECT path,rows FROM files")?;
    let mut files = 0_u64;
    let mut rows = 0_u64;
    for row in statement.query_map([], |row| {
        Ok((
            PathBuf::from(row.get::<_, String>(0)?),
            row.get::<_, u64>(1)?,
        ))
    })? {
        let (path, count) = row?;
        if path.file_name().and_then(|name| name.to_str()) == Some(filename) {
            files = files
                .checked_add(1)
                .context("shuffle file count overflow")?;
            rows = rows
                .checked_add(count)
                .context("shuffle segment count overflow")?;
        }
    }
    Ok((files, rows))
}

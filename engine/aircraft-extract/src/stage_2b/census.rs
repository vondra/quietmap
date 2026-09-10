//! Streaming primary-input census records actual transit counts without world-sized output.

use super::*;
use crate::arrow_io::inspect_ipc_allocation;
use rusqlite::{params, Connection};
use std::os::unix::fs::MetadataExt;

pub(super) fn identity(path: &Path) -> Result<String> {
    let metadata = path.metadata()?;
    Ok(format!(
        "{}:{}:{}:{}:{}:{}:{}",
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec()
    ))
}

/// The report is advisory staged evidence, not a cryptographic generation receipt.
/// Only completed inputs are committed; an interrupted census cannot claim a full window.
pub fn census_cruise_inputs(paths: &[PathBuf], output: &Path) -> Result<()> {
    anyhow::ensure!(
        !paths.is_empty(),
        "cruise census requires primary day inputs"
    );
    anyhow::ensure!(!output.exists(), "cruise census output must be new");
    let largest_batch = paths
        .iter()
        .map(|path| inspect_ipc_allocation(path).map(|facts| facts.largest_batch_bytes))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .max()
        .unwrap_or(0);
    crate::memory::max_concurrent_tasks(
        1,
        largest_batch.saturating_mul(2) + 2 * SPILL_TRIGGER_BYTES as u64,
    )?;
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut db = Connection::open(output)?;
    db.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;
        CREATE TABLE census(expected_inputs INTEGER NOT NULL, completed_inputs INTEGER NOT NULL);
        CREATE TABLE allocation_facts(name TEXT PRIMARY KEY, bytes INTEGER NOT NULL);
        CREATE TABLE inputs(path TEXT PRIMARY KEY, stat_identity TEXT NOT NULL, file_bytes INTEGER NOT NULL,
            input_rows INTEGER NOT NULL, cruise_segments INTEGER NOT NULL, ga_cruise_segments INTEGER NOT NULL,
            transit_rows INTEGER NOT NULL, transit_callsign_bytes INTEGER NOT NULL,
            maximum_segment_transits INTEGER NOT NULL, maximum_segment_length_m REAL NOT NULL, maximum_callsign_bytes INTEGER NOT NULL);
        CREATE TABLE hash_counts(input_path TEXT NOT NULL, hash_bucket INTEGER NOT NULL, transit_rows INTEGER NOT NULL,
            transit_callsign_bytes INTEGER NOT NULL, PRIMARY KEY(input_path, hash_bucket));")?;
    db.execute("INSERT INTO census VALUES (?1,0)", [paths.len()])?;
    for (name, bytes) in [
        ("spill_trigger", SPILL_TRIGGER_BYTES as u64),
        (
            "transit_charge_without_callsign",
            allocation::transit_allocation(0) as u64,
        ),
        (
            "spill_file_overhead_bound",
            crate::arrow_io::spill_file_overhead_bound()?,
        ),
        (
            "spill_workers_fixed_buffers",
            4 * SPILL_TRIGGER_BYTES as u64,
        ),
        (
            "decoded_segment_rows",
            (crate::arrow_io::SEGMENT_READ_CHUNK_ROWS * (std::mem::size_of::<FlightSegment>() + 32))
                as u64,
        ),
    ] {
        db.execute(
            "INSERT INTO allocation_facts VALUES (?1,?2)",
            params![name, bytes],
        )?;
    }
    for path in paths {
        let before = identity(path)?;
        let expected_day = crate::period::parse_date_id(
            path.file_stem()
                .context("missing input day")?
                .to_str()
                .context("non-UTF8 input day")?,
        )?;
        let mut counts = vec![(0u64, 0u64); SPILL_HASH_BUCKETS as usize];
        let (mut rows, mut cruise_segments, mut ga_cruise) = (0u64, 0u64, 0u64);
        let (mut max_transits, mut max_callsign) = (0usize, 0usize);
        let mut max_length = 0.0f32;
        let progress = Milestone::new("stage2b/census", "input rows", 10_000_000);
        crate::arrow_io::for_each_segment_batch(path, |segments| {
            rows += segments.len() as u64;
            progress.add(segments.len() as u64);
            for segment in segments {
                anyhow::ensure!(
                    segment.date_id == expected_day,
                    "census input date differs from filename"
                );
                if segment.phase != Phase::Cruise || segment.veh_kind != 0 {
                    continue;
                }
                cruise_segments += 1;
                ga_cruise += u64::from(crate::profile::is_ga_sampled_profile(segment.profile_idx));
                let transits = cruise_transits(
                    segment.start_lat,
                    segment.start_lon,
                    segment.end_lat,
                    segment.end_lon,
                );
                max_transits = max_transits.max(transits.len());
                max_callsign = max_callsign.max(segment.callsign.len());
                max_length = max_length.max(segment.length_m);
                for (cell, _) in transits {
                    let entry = &mut counts[spill_bucket(cruise_parent(cell)) as usize];
                    entry.0 += 1;
                    entry.1 += segment.callsign.len() as u64;
                }
            }
            Ok(())
        })?;
        anyhow::ensure!(
            identity(path)? == before,
            "primary input changed during census"
        );
        let total_rows: u64 = counts.iter().map(|count| count.0).sum();
        let total_callsigns: u64 = counts.iter().map(|count| count.1).sum();
        let transaction = db.transaction()?;
        transaction.execute(
            "INSERT INTO inputs VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                path.to_str().context("non-UTF8 input path")?,
                before,
                path.metadata()?.len(),
                rows,
                cruise_segments,
                ga_cruise,
                total_rows,
                total_callsigns,
                max_transits,
                max_length,
                max_callsign
            ],
        )?;
        for (bucket, (transit_rows, callsign_bytes)) in counts.into_iter().enumerate() {
            transaction.execute(
                "INSERT INTO hash_counts VALUES (?1,?2,?3,?4)",
                params![path.to_str().unwrap(), bucket, transit_rows, callsign_bytes],
            )?;
        }
        transaction.execute("UPDATE census SET completed_inputs=completed_inputs+1", [])?;
        transaction.commit()?;
        eprintln!("{} [stage2b/census] {}: {rows} input rows; {cruise_segments} cruise segments; {total_rows} transits; maximum {max_transits} transits/segment", ts(), path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn census_counts_visits_without_merging_and_marks_interrupted_windows_incomplete() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("2025-01-01.arrow");
        let second = directory.path().join("2025-02-01.arrow");
        let mut segment = super::super::tests::cruise(42, 50.1, 14.2, 50.1, 14.20001);
        segment.date_id = crate::period::parse_date_id("2025-01-01").unwrap();
        segment.callsign = "TEST42".into();
        let mut airborne = segment.clone();
        airborne.phase = Phase::Airborne;
        crate::arrow_io::write_segments(&first, &[segment.clone(), segment.clone(), airborne])
            .unwrap();
        crate::arrow_io::write_segments(&second, &[segment]).unwrap();
        let output = directory.path().join("census.sqlite");
        assert!(census_cruise_inputs(&[first, second], &output).is_err());
        let db = Connection::open(output).unwrap();
        assert_eq!(
            db.query_row(
                "SELECT expected_inputs, completed_inputs FROM census",
                [],
                |row| Ok((row.get::<_, usize>(0)?, row.get::<_, usize>(1)?))
            )
            .unwrap(),
            (2, 1)
        );
        assert_eq!(db.query_row("SELECT input_rows, cruise_segments, transit_rows, transit_callsign_bytes FROM inputs", [], |row| Ok((row.get::<_,usize>(0)?,row.get::<_,usize>(1)?,row.get::<_,usize>(2)?,row.get::<_,usize>(3)?))).unwrap(), (3,2,2,12));
    }
}

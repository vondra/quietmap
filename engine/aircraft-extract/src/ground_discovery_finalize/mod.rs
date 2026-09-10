//! Canonical discovery owners retain every unique strip and reject conflicting identities.
mod batches;
use crate::{
    scope::ScopeBbox,
    synth_airport_io::{DISCOVERED_AIRSTRIP_NAME, SYNTH_AREAS_FILE, SYNTH_LINES_FILE},
};
use anyhow::{ensure, Context, Result};
use arrow::{
    array::UInt32Array,
    compute::{concat_batches, take_record_batch},
    record_batch::RecordBatch,
};
use rusqlite::{params, Connection, OptionalExtension};
use std::{collections::BTreeMap, fs::File, path::Path};

#[derive(Clone, Copy)]
struct Group {
    id: u64,
    start: usize,
    count: usize,
    area: Option<usize>,
}

fn owner(id: u64) -> Result<u64> {
    crate::synth_airport_io::synth_owner_for_id(id)
}

const INDEX_BYTES: u64 = 256 << 20;

fn read_group(row: &rusqlite::Row<'_>) -> rusqlite::Result<(u64, Group)> {
    Ok((
        row.get::<_, i64>(0)? as u64,
        Group {
            id: row.get::<_, i64>(1)? as u64,
            start: row.get(2)?,
            count: row.get(3)?,
            area: row.get(4)?,
        },
    ))
}

/// Finalize a closed raw discovery tree into a new tree; never mutate its inputs.
/// The caller may publish the returned generation only after this function succeeds.
pub fn finalize_ground_discovery(
    source: &Path,
    output: &Path,
    scope: Option<&ScopeBbox>,
    retained_bytes: u64,
) -> Result<usize> {
    ensure!(
        !output.exists(),
        "discovery finalization requires a fresh output tree"
    );
    let sources = crate::spatial::square_directories(source)?;
    let mut allowances = BTreeMap::new();
    for (id, _) in &sources {
        allowances.insert(*id, batches::source_allowance(source, *id)?);
    }
    let fixed_allowance = retained_bytes
        .checked_add(1 << 30)
        .context("retained discovery allocation overflow")?;
    let maximum = allowances.values().copied().max().unwrap_or(0);
    crate::memory::max_concurrent_tasks(
        1,
        maximum
            .checked_mul(2)
            .and_then(|n| n.checked_add(fixed_allowance))
            .context("discovery memory allowance overflow")?,
    )?;
    crate::arrow_io::create_directory_all_synced(output)?;
    let database_path = output.join("discovery.sqlite");
    let mut database = Connection::open(&database_path)?;
    database.pragma_update(None, "page_size", 4096)?;
    database.pragma_update(None, "max_page_count", INDEX_BYTES / 4096)?;
    ensure!(
        database.pragma_query_value(None, "page_size", |r| r.get::<_, u64>(0))? == 4096,
        "unexpected discovery index page size"
    );
    database.execute_batch("PRAGMA synchronous=FULL; PRAGMA cache_size=-16384; PRAGMA temp_store=MEMORY;
        CREATE TABLE strips(id INTEGER PRIMARY KEY, owner INTEGER NOT NULL, source INTEGER NOT NULL, start INTEGER NOT NULL, count INTEGER NOT NULL, area INTEGER);
        CREATE INDEX destination ON strips(owner,source,start);
        CREATE TABLE owners(id INTEGER PRIMARY KEY);
        CREATE TABLE state(complete INTEGER NOT NULL);")?;
    for (source_owner, _) in &sources {
        // Reserve the capped SQLite file, its rollback copy and journal/page overhead.
        index_disk_guard(output)?;
        let current = batches::load(source, *source_owner)?;
        let transaction = database.transaction()?;
        transaction.execute(
            "INSERT OR IGNORE INTO owners VALUES(?)",
            [*source_owner as i64],
        )?;
        for group in batches::groups(&current)? {
            let existing = transaction
                .query_row(
                    "SELECT source,id,start,count,area FROM strips WHERE id=?",
                    [group.id as i64],
                    read_group,
                )
                .optional()?;
            if let Some((previous_owner, previous)) = existing {
                let other = batches::load(source, previous_owner)?;
                ensure!(
                    batches::same_strip(&current, group, &other, previous),
                    "conflicting discovered strip identity {}",
                    group.id
                );
            } else {
                let target = owner(group.id)?;
                transaction.execute(
                    "INSERT INTO strips VALUES(?,?,?,?,?,?)",
                    params![
                        group.id as i64,
                        target as i64,
                        *source_owner as i64,
                        group.start,
                        group.count,
                        group.area
                    ],
                )?;
                transaction.execute("INSERT OR IGNORE INTO owners VALUES(?)", [target as i64])?;
            }
        }
        transaction.commit()?;
        crate::arrow_io::spill_receipt_watermark(output)?;
    }
    let destinations: Vec<u64> = database
        .prepare("SELECT id FROM owners ORDER BY id")?
        .query_map([], |r| Ok(r.get::<_, i64>(0)? as u64))?
        .collect::<rusqlite::Result<_>>()?;
    // Check every output owner's actual source working set before emitting any Arrow.
    for &destination in &destinations {
        let source_ids: Vec<u64> = database
            .prepare("SELECT DISTINCT source FROM strips WHERE owner=?")?
            .query_map([destination as i64], |r| Ok(r.get::<_, i64>(0)? as u64))?
            .collect::<rusqlite::Result<_>>()?;
        let allowance = source_ids.iter().try_fold(fixed_allowance, |n, id| {
            n.checked_add(allowances[id])
                .context("discovery owner allocation overflow")
        })?;
        crate::memory::max_concurrent_tasks(1, allowance)?;
    }
    let mut populated = 0;
    for destination in destinations {
        if scope.is_some_and(|scope| !scope.contains_square(destination)) {
            continue;
        }
        let mut selected = database.prepare(
            "SELECT source,id,start,count,area FROM strips WHERE owner=? ORDER BY source,start",
        )?;
        let selected: Vec<_> = selected
            .query_map([destination as i64], read_group)?
            .collect::<rusqlite::Result<_>>()?;
        populated += usize::from(!selected.is_empty());
        let mut line_batches = Vec::new();
        let mut area_batches = Vec::new();
        let mut start = 0;
        while start < selected.len() {
            let source_owner = selected[start].0;
            let mut end = start + 1;
            while end < selected.len() && selected[end].0 == source_owner {
                end += 1;
            }
            let source = batches::load(source, source_owner)?;
            let lines: Vec<u32> = selected[start..end]
                .iter()
                .flat_map(|(_, group)| group.start..group.start + group.count)
                .map(u32::try_from)
                .collect::<std::result::Result<_, _>>()?;
            let areas: Vec<u32> = selected[start..end]
                .iter()
                .filter_map(|(_, group)| group.area)
                .map(u32::try_from)
                .collect::<std::result::Result<_, _>>()?;
            line_batches.push(take_record_batch(&source.lines, &UInt32Array::from(lines))?);
            area_batches.push(take_record_batch(&source.areas, &UInt32Array::from(areas))?);
            start = end;
        }
        let directory = output.join(crate::spatial::square_path(destination));
        for (name, schema, batches) in [
            (
                SYNTH_LINES_FILE,
                crate::arrow_schemas::synth_airport_lines_schema(),
                line_batches,
            ),
            (
                SYNTH_AREAS_FILE,
                crate::arrow_schemas::synth_airport_areas_schema(),
                area_batches,
            ),
        ] {
            let batch = concat_batches(&schema, &batches)?;
            crate::arrow_io::write_record_batches(&directory.join(name), &schema, &[batch])?;
        }
    }
    database.execute("INSERT INTO state VALUES(1)", [])?;
    database.close().map_err(|(_, error)| error)?;
    File::open(database_path)?.sync_all()?;
    File::open(output)?.sync_all()?;
    Ok(populated)
}

fn index_disk_guard(output: &Path) -> Result<()> {
    use std::os::fd::AsRawFd;
    let file = File::open(output)?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    ensure!(
        unsafe { libc::fstatvfs(file.as_raw_fd(), stats.as_mut_ptr()) } == 0,
        "read finalization disk space"
    );
    let stats = unsafe { stats.assume_init() };
    let protected =
        crate::arrow_io::spill_receipt_watermark(output)?.map_or(0, |(_, minimum)| minimum);
    ensure!(
        stats
            .f_bavail
            .saturating_mul(stats.f_frsize)
            .saturating_sub(protected)
            >= 3 * INDEX_BYTES,
        "discovery index and rollback journal exceed available reservation"
    );
    Ok(())
}

/// Promote only completed canonical sidecars; unrelated prepared layers remain untouched.
pub(crate) fn promote_ground_discovery(source: &Path, output: &Path) -> Result<()> {
    for (_, directory) in crate::spatial::square_directories(source)? {
        let relative = directory.strip_prefix(source)?;
        let target = output.join(relative);
        crate::arrow_io::create_directory_all_synced(&target)?;
        for name in [SYNTH_LINES_FILE, SYNTH_AREAS_FILE] {
            std::fs::rename(directory.join(name), target.join(name))?;
        }
        File::open(&target)?.sync_all()?;
        File::open(&directory)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;

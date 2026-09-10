//! Preserve prepared coordinates and all non-name Arrow values through canonical ownership.
use super::*;
use arrow::array::{Array, StringArray, UInt16Array, UInt64Array};
use arrow::compute::concat_batches;
use std::sync::Arc;

pub(super) struct Source {
    pub lines: RecordBatch,
    pub areas: RecordBatch,
}

pub(super) fn load(root: &Path, owner: u64) -> Result<Source> {
    let directory = root.join(crate::spatial::square_path(owner));
    Ok(Source {
        lines: read(
            &directory.join(SYNTH_LINES_FILE),
            &crate::arrow_schemas::synth_airport_lines_schema(),
        )?,
        areas: read(
            &directory.join(SYNTH_AREAS_FILE),
            &crate::arrow_schemas::synth_airport_areas_schema(),
        )?,
    })
}

fn read(path: &Path, expected: &arrow::datatypes::Schema) -> Result<RecordBatch> {
    let (schema, batches) = crate::arrow_io::read_record_batches(path)?;
    ensure!(
        schema.fields() == expected.fields() && schema.metadata() == expected.metadata(),
        "invalid discovered geometry schema: {}",
        path.display()
    );
    let batch = concat_batches(&Arc::new(schema), &batches)?;
    ensure!(
        batch
            .columns()
            .iter()
            .all(|column| column.null_count() == 0),
        "null discovered geometry"
    );
    let mut columns = batch.columns().to_vec();
    let index = batch.schema().index_of("name")?;
    columns[index] = Arc::new(StringArray::from_iter_values(std::iter::repeat_n(
        DISCOVERED_AIRSTRIP_NAME,
        batch.num_rows(),
    )));
    Ok(RecordBatch::try_new(batch.schema(), columns)?)
}

pub(super) fn ids(batch: &RecordBatch) -> &UInt64Array {
    batch
        .column_by_name("osm_id")
        .unwrap()
        .as_any()
        .downcast_ref()
        .unwrap()
}

pub(super) fn groups(source: &Source) -> Result<Vec<Group>> {
    let mut area_indices = std::collections::HashMap::new();
    for (index, id) in ids(&source.areas).values().iter().copied().enumerate() {
        ensure!(
            area_indices.insert(id, index).is_none(),
            "duplicate discovered area identity in one source"
        );
    }
    let ids = ids(&source.lines);
    let segments = source
        .lines
        .column_by_name("segment_idx")
        .unwrap()
        .as_any()
        .downcast_ref::<UInt16Array>()
        .unwrap();
    let mut groups = Vec::new();
    let mut start = 0;
    while start < ids.len() {
        let id = ids.value(start);
        ensure!(
            segments.value(start) == 0,
            "discovered strip must start at segment zero"
        );
        let mut end = start + 1;
        while end < ids.len() && segments.value(end) != 0 {
            ensure!(
                ids.value(end) == id && usize::from(segments.value(end)) == end - start,
                "discovered strip has missing or conflicting segment identity"
            );
            end += 1;
        }
        let key = source
            .lines
            .column_by_name("airport_key")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        ensure!(
            !key.value(start).is_empty()
                && (start..end).all(|row| key.value(row) == key.value(start)),
            "discovered strip has ambiguous airport identity"
        );
        if let Some(&area) = area_indices.get(&id) {
            let area_key = source
                .areas
                .column_by_name("airport_key")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            ensure!(
                area_key.value(area) == key.value(start),
                "discovered area and strip disagree on airport identity"
            );
        }
        groups.push(Group {
            id,
            start,
            count: end - start,
            area: area_indices.remove(&id),
        });
        start = end;
    }
    ensure!(
        area_indices.is_empty(),
        "discovered area has no strip geometry"
    );
    Ok(groups)
}

pub(super) fn same_strip(left: &Source, a: Group, right: &Source, b: Group) -> bool {
    a.count == b.count
        && left.lines.slice(a.start, a.count) == right.lines.slice(b.start, b.count)
        && match (a.area, b.area) {
            (Some(a), Some(b)) => left.areas.slice(a, 1) == right.areas.slice(b, 1),
            (None, None) => true,
            _ => false,
        }
}

pub(super) fn source_allowance(root: &Path, owner: u64) -> Result<u64> {
    let directory = root.join(crate::spatial::square_path(owner));
    let mut bytes = 0u128;
    let mut rows = 0u128;
    for name in [SYNTH_LINES_FILE, SYNTH_AREAS_FILE] {
        let facts = crate::arrow_io::inspect_ipc_allocation(&directory.join(name))?;
        bytes += u128::from(facts.file_bytes);
        rows += facts.rows as u128;
    }
    let buckets = (rows * 8)
        .div_ceil(7)
        .max(4)
        .checked_next_power_of_two()
        .context("discovery index capacity overflow")?;
    // Source IPC, concatenation, selected arrays and final concatenation coexist.
    // Group/selection vectors include old+new growth; area lookup includes both hash tables.
    let containers = 6 * rows * std::mem::size_of::<Group>() as u128
        + 4 * rows * std::mem::size_of::<u32>() as u128
        + 2 * buckets * (std::mem::size_of::<(u64, usize)>() + 1) as u128;
    u64::try_from(2 * (6 * bytes + containers)).context("discovery finalization allowance overflow")
}

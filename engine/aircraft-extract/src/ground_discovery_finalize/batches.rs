//! Preserve prepared coordinates and all non-name Arrow values through canonical ownership.
use super::*;
use arrow::array::{Array, StringArray, UInt16Array, UInt64Array};
use arrow::compute::concat_batches;
use std::sync::Arc;

/// One source owner's discovered lines with the display name canonicalized.
pub(super) fn load(root: &Path, owner: u64) -> Result<RecordBatch> {
    read(
        &root
            .join(crate::spatial::square_path(owner))
            .join(SYNTH_LINES_FILE),
        &crate::arrow_schemas::synth_airport_lines_schema(),
    )
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

pub(super) fn groups(lines: &RecordBatch) -> Result<Vec<Group>> {
    let ids = ids(lines);
    let segments = lines
        .column_by_name("segment_idx")
        .unwrap()
        .as_any()
        .downcast_ref::<UInt16Array>()
        .unwrap();
    let key = lines
        .column_by_name("airport_key")
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
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
        ensure!(
            !key.value(start).is_empty()
                && (start..end).all(|row| key.value(row) == key.value(start)),
            "discovered strip has ambiguous airport identity"
        );
        groups.push(Group {
            id,
            start,
            count: end - start,
        });
        start = end;
    }
    Ok(groups)
}

pub(super) fn same_strip(left: &RecordBatch, a: Group, right: &RecordBatch, b: Group) -> bool {
    a.count == b.count && left.slice(a.start, a.count) == right.slice(b.start, b.count)
}

pub(super) fn source_allowance(root: &Path, owner: u64) -> Result<u64> {
    let directory = root.join(crate::spatial::square_path(owner));
    let facts = crate::arrow_io::inspect_ipc_allocation(&directory.join(SYNTH_LINES_FILE))?;
    let bytes = u128::from(facts.file_bytes);
    let rows = facts.rows as u128;
    let buckets = (rows * 8)
        .div_ceil(7)
        .max(4)
        .checked_next_power_of_two()
        .context("discovery index capacity overflow")?;
    // Source IPC, concatenation, selected arrays and final concatenation coexist.
    // Group/selection vectors include old+new growth; the identity index keeps two hash tables.
    let containers = 6 * rows * std::mem::size_of::<Group>() as u128
        + 4 * rows * std::mem::size_of::<u32>() as u128
        + 2 * buckets * (std::mem::size_of::<(u64, usize)>() + 1) as u128;
    u64::try_from(2 * (6 * bytes + containers)).context("discovery finalization allowance overflow")
}

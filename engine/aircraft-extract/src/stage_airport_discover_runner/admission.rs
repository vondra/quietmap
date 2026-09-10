//! Data-derived clustering and bounded writer allocation allowance before sidecar mutation.
use super::*;
use std::mem::size_of;

pub(super) fn working_set_allowance(
    candidates: usize,
    decoded_batch_bytes: usize,
    input_bytes: u64,
    shared_bytes: u64,
) -> Result<u64> {
    let vertices = candidates as u128;
    let clusters = vertices / DBSCAN_MIN_SAMPLES as u128;
    // Hashbrown's 7/8 load and power-of-two buckets; count old and new tables
    // during growth. Every cell may own a minimum-capacity four-index Vec.
    let buckets = (vertices * 8)
        .div_ceil(7)
        .max(4)
        .checked_next_power_of_two()
        .context("ground hash table capacity overflow")?;
    let hash = 2 * buckets * (size_of::<((i32, i32), Vec<usize>)>() + 1) as u128 + 64;
    let query_cache = 2 * buckets * (size_of::<(u32, u32)>() + 1) as u128 + 64;
    let cell_indices = 6 * vertices * size_of::<usize>() as u128;
    let labels = vertices * (size_of::<Option<usize>>() + 2 * size_of::<bool>()) as u128;
    // Candidate + grid coordinates; both queue vectors during growth; member
    // collection during growth + PCA points. Overlap is deliberately overcharged.
    let vectors = vertices
        * (2 * size_of::<(f32, f32)>() + 6 * size_of::<usize>() + 4 * size_of::<(f32, f32)>())
            as u128;
    let strips = 3
        * clusters
        * (size_of::<DiscoveredStrip>() + size_of::<(DiscoveredStrip, Option<&AirportArea>)>())
            as u128;
    // Input IPC and decoded callsigns coexist. Local lines and the leg-hit Vec
    // are bounded by the complete source line count, including capacity growth.
    let input = 2 * u128::from(input_bytes) + decoded_batch_bytes as u128;
    // Allocator bookkeeping/fragmentation is reserved separately from container
    // capacities; the fixed 1 GiB also covers runtime, schema and input-path state.
    let allowance = 2 * (hash + query_cache + cell_indices + labels + vectors + strips + input)
        + u128::from(shared_bytes)
        + (1u128 << 30);
    u64::try_from(allowance).context("ground allocation allowance overflow")
}

pub(super) fn shared_allocation_allowance(
    index: &AerodromeIndex,
    lines: &[AirportLineRow],
) -> Result<u64> {
    let lines = lines.len() as u128
        * (2 * size_of::<AirportLineRow>()
            + 3 * size_of::<AirportLineSegment>()
            + 3 * size_of::<crate::stage_2c::airport_traffic::LegIntersection>()) as u128;
    let writer =
        crate::synth_airport_io::writer_allocation_allowance(index.maximum_airport_key_bytes());
    u64::try_from(2 * lines + u128::from(writer) + u128::from(index.allocation_allowance()))
        .context("ground shared allocation allowance overflow")
}

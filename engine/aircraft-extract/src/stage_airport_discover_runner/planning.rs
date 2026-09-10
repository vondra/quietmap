//! Read-only discovery admission uses the producer's exact validation and polygon predicate.
use super::*;
use std::path::PathBuf;

pub struct GroundDiscoveryInput {
    pub path: PathBuf,
    pub rows: usize,
    pub candidate_vertices_upper_bound: usize,
    pub decoded_batch_bytes: usize,
    pub input_bytes: u64,
    pub allocation_bytes: u64,
    pub(super) extent: Extent,
}

pub fn plan_ground_discovery(
    segments_by_square_dir: &Path,
    aerodrome_index: &AerodromeIndex,
    airport_lines_global: &[AirportLineRow],
    scope: Option<&ScopeBbox>,
) -> Result<BTreeMap<u64, GroundDiscoveryInput>> {
    let active: BTreeMap<_, _> =
        crate::shuffle::list_square_shards(segments_by_square_dir, "ground.arrow", scope)?
            .into_iter()
            .collect();
    let mut inputs = BTreeMap::new();
    let shared_allocation =
        admission::shared_allocation_allowance(aerodrome_index, airport_lines_global)?;
    let mut largest_allocation = shared_allocation;
    for (&square, shard) in &active {
        let input_bytes = std::fs::metadata(shard)?.len();
        let validation_allowance = admission::working_set_allowance(
            0,
            crate::arrow_io::SEGMENT_READ_CHUNK_ROWS * std::mem::size_of::<FlightSegment>()
                + usize::try_from(input_bytes)?,
            input_bytes,
            shared_allocation,
        )?;
        crate::memory::max_concurrent_tasks(1, validation_allowance)?;
        let mut extent = Extent::empty(square);
        let mut rows = 0usize;
        let mut candidate_bound = 0usize;
        let mut decoded_batch_bytes = 0usize;
        crate::arrow_io::for_each_segment_batch(shard, |segments| {
            rows = rows
                .checked_add(segments.len())
                .context("ground row count overflow")?;
            decoded_batch_bytes = decoded_batch_bytes.max(
                segments.capacity() * std::mem::size_of::<FlightSegment>()
                    + segments
                        .iter()
                        .map(|segment| segment.callsign.capacity())
                        .sum::<usize>(),
            );
            for segment in segments {
                if outside_known_aerodromes(&segment, aerodrome_index) {
                    candidate_bound = candidate_bound
                        .checked_add(2)
                        .context("candidate count overflow")?;
                }
                extent.include(segment.start_lat, segment.start_lon);
                extent.include(segment.end_lat, segment.end_lon);
            }
            Ok(())
        })
        .with_context(|| format!("validate {}", shard.display()))?;
        let allowance = admission::working_set_allowance(
            candidate_bound,
            decoded_batch_bytes,
            input_bytes,
            shared_allocation,
        )?;
        if allowance > largest_allocation {
            largest_allocation = allowance;
            eprintln!("{} [stage1.5] largest allocation: {} B at {} ({} rows, {} candidate vertices before line snap, {} decoded batch bytes)",
                crate::progress::ts(), allowance, square_path(square), rows, candidate_bound, decoded_batch_bytes);
        }
        inputs.insert(
            square,
            GroundDiscoveryInput {
                path: shard.clone(),
                extent,
                rows,
                candidate_vertices_upper_bound: candidate_bound,
                decoded_batch_bytes,
                input_bytes,
                allocation_bytes: allowance,
            },
        );
    }
    Ok(inputs)
}

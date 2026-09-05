//! Copy finalized cruise rows to receiver support cells without aggregating publication copies.

use super::*;
use crate::arrow_io::{read_record_batches, required_column, write_record_batches};
use arrow::array::{Float64Array, UInt8Array};
use arrow::compute::interleave_record_batch;

pub(super) fn scatter_finalized_cruise(
    buckets: impl Iterator<Item = CruiseBucket>,
    directory: &Path,
    part_id: &AtomicU64,
    n_days: u16,
    scope: Option<&ScopeBbox>,
) -> Result<u64> {
    let mut copies: HashMap<u64, Vec<CruiseBucket>> = HashMap::new();
    let mut buffered_bytes = 0;
    let mut canonical_rows = 0;
    for bucket in buckets {
        canonical_rows += 1;
        let (lon, lat) = grid::cruise::cruise_centroid(bucket.cruise_cell_id);
        let support =
            noise_compute::emission::aircraft::cruise_support_cells(lat, lon, bucket.rep_len_m)
                .context("invalid finalized cruise support")?;
        let row_bytes = std::mem::size_of::<CruiseBucket>()
            + bucket
                .top_candidates
                .iter()
                .map(|candidate| {
                    std::mem::size_of::<CruiseTopCandidate>() + candidate.callsign.len()
                })
                .sum::<usize>();
        for square in support.iter().map(|square| grid::square_id(square) as u64) {
            if scope.is_some_and(|scope| !scope.contains_square(square)) {
                continue;
            }
            copies.entry(square).or_default().push(bucket.clone());
            buffered_bytes += row_bytes;
            if buffered_bytes >= SPILL_TRIGGER_BYTES {
                flush_copies(&mut copies, directory, part_id, n_days)?;
                buffered_bytes = 0;
            }
        }
    }
    flush_copies(&mut copies, directory, part_id, n_days)?;
    Ok(canonical_rows)
}

fn flush_copies(
    copies: &mut HashMap<u64, Vec<CruiseBucket>>,
    directory: &Path,
    part_id: &AtomicU64,
    n_days: u16,
) -> Result<()> {
    for (square, rows) in std::mem::take(copies) {
        let id = part_id.fetch_add(1, Ordering::Relaxed);
        let path = directory
            .join(square_path(square))
            .join(format!("part_{id:016x}.arrow"));
        write_cruise(&path, &rows, n_days)?;
    }
    Ok(())
}

pub(super) fn gather_finalized_cruise(directory: &Path, prepared: &Path) -> Result<(usize, u64)> {
    let destinations = crate::spatial::square_directories(directory)?;
    let mut inputs = Vec::with_capacity(destinations.len());
    let mut largest_bytes = 0u64;
    for (square, path) in destinations {
        let parts = list_spill_parts(&path)?;
        let mut bytes = 0;
        for part in &parts {
            bytes += part.metadata()?.len();
        }
        largest_bytes = largest_bytes.max(bytes);
        inputs.push((square, parts));
    }
    // One destination's input buffers, row references, and copied output are
    // the gather working set; use the existing measured-byte concurrency policy.
    let workers = crate::memory::max_concurrent_days(
        rayon::current_num_threads(),
        (largest_bytes as f64 * 2.0) / 1_000_000_000.0,
    );
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .build()?;
    let written_rows = AtomicU64::new(0);
    pool.install(|| {
        inputs
            .par_iter()
            .try_for_each(|(square, parts)| -> Result<()> {
                let mut batches = Vec::new();
                let mut indexed_rows = Vec::new();
                for path in parts {
                    for batch in read_record_batches(path)?.1 {
                        let lon = required_column::<Float64Array>(&batch, "lon")?;
                        let lat = required_column::<Float64Array>(&batch, "lat")?;
                        let class = required_column::<UInt8Array>(&batch, "class")?;
                        let fl = required_column::<UInt8Array>(&batch, "fl_bin")?;
                        let period = required_column::<UInt8Array>(&batch, "period")?;
                        for row in 0..batch.num_rows() {
                            let key = (
                                grid::cruise::cruise_cell_id(lat.value(row), lon.value(row)),
                                class.value(row),
                                fl.value(row),
                                period.value(row),
                            );
                            indexed_rows.push((key, (batches.len(), row)));
                        }
                        if let Some(first) = batches.first() {
                            let first: &arrow::record_batch::RecordBatch = first;
                            anyhow::ensure!(
                                batch.schema() == first.schema(),
                                "cruise copy schemas disagree"
                            );
                        }
                        batches.push(batch);
                    }
                }
                indexed_rows.sort_unstable_by_key(|(key, _)| *key);
                anyhow::ensure!(
                    indexed_rows.windows(2).all(|pair| pair[0].0 != pair[1].0),
                    "duplicate finalized cruise key in {}",
                    square_path(*square)
                );
                if indexed_rows.is_empty() {
                    return Ok(());
                }
                let indices: Vec<_> = indexed_rows.into_iter().map(|(_, index)| index).collect();
                let refs: Vec<_> = batches.iter().collect();
                let output = interleave_record_batch(&refs, &indices)?;
                write_record_batches(
                    &prepared.join(square_path(*square)).join("cruise.arrow"),
                    output.schema().as_ref(),
                    std::slice::from_ref(&output),
                )?;
                written_rows.fetch_add(output.num_rows() as u64, Ordering::Relaxed);
                Ok(())
            })
    })?;
    Ok((inputs.len(), written_rows.load(Ordering::Relaxed)))
}

#[cfg(test)]
mod tests {
    use super::super::tests::cruise;
    use super::*;

    #[test]
    fn native_fold_copies_final_values_once_across_long_polar_and_seam_destinations() {
        for (start, end, receiver, scoped) in [
            ([49.0, 14.25], [51.0, 14.25], [50.0, 15.83], false),
            ([49.0, 14.25], [51.0, 14.25], [50.0, 15.83], true),
            (
                [80.178_71, 0.0],
                [80.18, 0.002],
                [80.05804856215623, 0.0],
                false,
            ),
            ([0.0, 179.99], [0.0, -179.99], [0.001, -180.0], false),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let mut first = cruise(42, start[0], start[1], end[0], end[1]);
            first.callsign = "COPY42".into();
            first.aircraft_type = *b"B738";
            first.source_id = 2;
            let mut second = first.clone();
            second.flight_id = 43;
            second.callsign = "COPY43".into();
            let segments = [first.clone(), first, second];
            let day = directory.path().join("segments.arrow");
            crate::arrow_io::write_segments(&day, &segments).unwrap();
            let scope = scoped.then(|| ScopeBbox::parse("50,15.83,50,15.83").unwrap());
            let prepared = directory.path().join("prepared");
            let written = run_stage_2b(&[day], &prepared, 12, scope.as_ref(), false).unwrap();
            let mut canonical = HashMap::new();
            for segment in &segments {
                process_segment(segment, &mut canonical, NpdLuts::shared());
            }
            if scoped {
                assert!(
                    canonical
                        .keys()
                        .all(|&owner| !scope.unwrap().contains_square(owner)),
                    "scope must exclude every canonical owner while keeping reached destinations"
                );
            }
            let mut expected: HashMap<u64, Vec<CruiseBucket>> = HashMap::new();
            let mut canonical_length = 0.0f64;
            for map in canonical.into_values() {
                for (key, accum) in map {
                    let bucket = accum.finalize(key);
                    assert_eq!(bucket.unique_count, 2);
                    assert_eq!(bucket.top_candidates.len(), 2);
                    assert_eq!(bucket.top_candidates[0].callsign, "COPY42");
                    canonical_length += f64::from(bucket.sum_length_m);
                    let (lon, lat) = grid::cruise::cruise_centroid(bucket.cruise_cell_id);
                    for square in noise_compute::emission::aircraft::cruise_support_cells(
                        lat,
                        lon,
                        bucket.rep_len_m,
                    )
                    .unwrap()
                    .iter()
                    {
                        let id = grid::square_id(square) as u64;
                        if scope.as_ref().is_none_or(|scope| scope.contains_square(id)) {
                            expected.entry(id).or_default().push(bucket.clone());
                        }
                    }
                }
            }
            let original_length = segments.iter().map(|s| f64::from(s.length_m)).sum::<f64>();
            assert!((canonical_length - original_length).abs() <= original_length * 1e-6);
            assert!(expected.contains_key(
                &(grid::square_id(grid::square_of(receiver[0], receiver[1])) as u64)
            ));
            assert_eq!(written, expected.len());
            assert_eq!(
                crate::spatial::square_directories(&prepared).unwrap().len(),
                expected.len()
            );
            for (square, mut rows) in expected {
                rows.sort_unstable_by_key(|r| (r.cruise_cell_id, r.class, r.fl_bin, r.period));
                let reference = directory.path().join("reference.arrow");
                write_cruise(&reference, &rows, 12).unwrap();
                assert_eq!(
                    read_record_batches(&prepared.join(square_path(square)).join("cruise.arrow"))
                        .unwrap(),
                    read_record_batches(&reference).unwrap(),
                    "all final columns and stamps at {}",
                    square_path(square)
                );
            }
        }
    }
}

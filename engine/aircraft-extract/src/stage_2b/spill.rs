//! Cruise accumulator spill and reconstruction without finalizing weighted means.

use super::*;

pub(super) fn list_spill_parts(dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("read_dir {}", dir.display())),
    };
    let mut out = Vec::new();
    for entry in read {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("arrow") {
            out.push(path);
        }
    }
    out.sort_unstable();
    Ok(out)
}

/// One day worker's buffered spill rows. Flushes drain the accumulator
/// into per-bucket buffers; past the cap the largest buffer reaches disk,
/// the rest at day end. A few chunk files per (day, bucket) instead of one
/// file per flush — hundreds of thousands of parts, not tens of millions —
/// and part names sort in deterministic fold order.
pub(super) struct SpillBuffers {
    day: usize,
    buffers: HashMap<u64, (Vec<CruiseSpillRow>, usize)>,
    bytes: usize,
    chunks: HashMap<u64, u32>,
}

impl SpillBuffers {
    pub(super) fn new(day: usize) -> Self {
        Self {
            day,
            buffers: HashMap::new(),
            bytes: 0,
            chunks: HashMap::new(),
        }
    }

    pub(super) fn bytes(&self) -> usize {
        self.bytes
    }

    /// Drain the accumulator into the per-bucket buffers. Takes `&mut` and
    /// drains via `std::mem::take` so callsign Strings move (no per-fid
    /// clone — ~10–100 fids per bucket × millions of buckets at global
    /// scope makes the clone cost real). Keys sort before consuming, so
    /// rows enter each buffer in fold order; sorting keys (24 B) beats
    /// sorting rows (100 B plus heap data) severalfold.
    pub(super) fn buffer_flush(
        &mut self,
        local: &mut HashMap<u64, HashMap<CruiseKey, CruiseAccum>>,
    ) {
        let mut drained = std::mem::take(local);
        let mut squares: Vec<u64> = drained.keys().copied().collect();
        squares.sort_unstable();
        for square in squares {
            let mut by_key = drained.remove(&square).expect("drained square present");
            let mut keys: Vec<CruiseKey> = by_key.keys().copied().collect();
            keys.sort_unstable();
            let bucket = spill_bucket(square);
            let (rows, bytes) = self.buffers.entry(bucket).or_default();
            for key in keys {
                let accum = by_key.remove(&key).expect("drained key present");
                let row = spill_row_consume(square, key, accum);
                let row_bytes = spill_row_bytes(&row);
                *bytes += row_bytes;
                self.bytes += row_bytes;
                rows.push(row);
            }
        }
    }

    /// Write the largest buffer past the cap. Rows arrived in sorted-key
    /// order per flush and chunks sequence flushes, so the fold merges in
    /// the same order on every run.
    pub(super) fn write_largest_buffer(
        &mut self,
        spill_dir: &Path,
        files_written: &AtomicU64,
    ) -> Result<()> {
        // Smallest bucket wins byte ties: HashMap iteration order must not
        // leak into chunk sequencing.
        let bucket = self
            .buffers
            .iter()
            .filter(|(_, (_, bytes))| *bytes > 0)
            .max_by_key(|(bucket, (_, bytes))| (*bytes, std::cmp::Reverse(**bucket)))
            .map(|(bucket, _)| *bucket)
            .context("spill buffer over cap without rows")?;
        self.write_buffer(spill_dir, bucket, files_written)
    }

    /// Write every non-empty buffer, buckets ascending.
    pub(super) fn write_all_buffers(
        &mut self,
        spill_dir: &Path,
        files_written: &AtomicU64,
    ) -> Result<()> {
        let mut buckets: Vec<u64> = self
            .buffers
            .iter()
            .filter(|(_, (rows, _))| !rows.is_empty())
            .map(|(bucket, _)| *bucket)
            .collect();
        buckets.sort_unstable();
        for bucket in buckets {
            self.write_buffer(spill_dir, bucket, files_written)?;
        }
        Ok(())
    }

    fn write_buffer(
        &mut self,
        spill_dir: &Path,
        bucket: u64,
        files_written: &AtomicU64,
    ) -> Result<()> {
        let chunk = *self.chunks.entry(bucket).or_insert(0);
        let (rows, bytes) = self.buffers.remove(&bucket).context("missing spill buffer")?;
        write_cruise_spill(&spill_part_path(spill_dir, bucket, self.day, chunk), &rows)?;
        self.chunks.insert(bucket, chunk + 1);
        self.bytes -= bytes;
        files_written.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

/// Overcounting size estimate: top callsigns charge 64 B each (struct plus
/// a long callsign) so the cap trips early rather than late, without
/// walking every candidate string on the hot path.
fn spill_row_bytes(row: &CruiseSpillRow) -> usize {
    128 + row.fid_set.len() * size_of::<u64>() + row.top_candidates.len() * 64
}

pub(super) fn spill_row_consume(square: u64, key: CruiseKey, accum: CruiseAccum) -> CruiseSpillRow {
    // fid_set: sort ascending for deterministic on-disk bytes.
    let mut fid_set: Vec<u64> = accum.fid_set.into_iter().collect();
    fid_set.sort_unstable();
    // top_candidates: fid ascending for deterministic on-disk bytes.
    // The fold replays rows through the re-entrant cap-K logic (a total
    // order, so arrival order cannot change the surviving set); sorting
    // here only stabilizes the spill bytes, never the fold values.
    let mut top_candidates: Vec<CruiseTopCandidate> = accum.top.into_values().collect();
    top_candidates.sort_unstable_by_key(|c| c.flight_id);
    CruiseSpillRow {
        square,
        cruise_cell_id: key.cruise_cell_id,
        class: key.class,
        fl_bin: key.fl_bin,
        period: key.period,
        rep_profile_idx: accum.rep_profile_idx,
        source_id: accum.source_id,
        origin: accum.origin,
        sum_length_m: accum.sum_length_m,
        weight: accum.weight,
        rep_alt_m: accum.rep_alt_m,
        rep_speed_kt: accum.rep_speed_kt,
        heading_bin: key.heading_bin,
        secondary_only: key.secondary_only,
        fid_set,
        top_candidates,
    }
}



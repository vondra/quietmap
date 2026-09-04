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

/// Rough byte estimate for a worker's thread-local accumulator —
/// per-bucket and per-fid overheads dominate; HashMap fill-factor
/// constant absorbed into the per-bucket multiplier. Checked at
/// SIZE_CHECK_INTERVAL so per-call cost amortises.
pub(super) fn estimate_worker_bytes(
    by_square: &HashMap<u64, HashMap<CruiseKey, CruiseAccum>>,
) -> usize {
    let mut n_buckets = 0usize;
    let mut n_fids = 0usize;
    let mut n_top_entries = 0usize;
    for inner in by_square.values() {
        n_buckets += inner.len();
        for accum in inner.values() {
            n_fids += accum.fid_set.len();
            n_top_entries += accum.top.len();
        }
    }
    // 200 B per (z9, CruiseKey) bucket = CruiseAccum + 2× HashMap entries
    // (outer + inner). 24 B per fid (HashSet<u64> entry: 8 B u64 + 16 B
    // hash table overhead). TOP_ENTRY_BYTES per top entry (capped at K).
    n_buckets * 200 + n_fids * 24 + n_top_entries * TOP_ENTRY_BYTES
}

/// Consume the worker's accumulator into spill files. Takes `&mut` and
/// drains via `std::mem::take` so callsign Strings move (no per-fid
/// clone — ~10–100 fids per bucket × millions of buckets at global
/// scope makes the clone cost real).
pub(super) fn flush_to_spill(
    local: &mut HashMap<u64, HashMap<CruiseKey, CruiseAccum>>,
    spill_dir: &Path,
    part_id: &AtomicU64,
) -> Result<()> {
    let drained = std::mem::take(local);
    let mut by_bucket: HashMap<u64, Vec<CruiseSpillRow>> = HashMap::new();
    for (square, by_key) in drained {
        let bucket = spill_bucket(square);
        let dst = by_bucket.entry(bucket).or_default();
        for (key, accum) in by_key {
            dst.push(spill_row_consume(square, key, accum));
        }
    }
    for (bucket, rows) in by_bucket {
        let id = part_id.fetch_add(1, Ordering::Relaxed);
        write_cruise_spill(&spill_part_path(spill_dir, bucket, id), &rows)?;
    }
    Ok(())
}

pub(super) fn spill_row_consume(square: u64, key: CruiseKey, accum: CruiseAccum) -> CruiseSpillRow {
    // fid_set: sort ascending for deterministic on-disk bytes.
    let mut fid_set: Vec<u64> = accum.fid_set.into_iter().collect();
    fid_set.sort_unstable();
    // top_candidates: keep insertion order — merge re-emits via the
    // re-entrant cap-K logic so the destination accumulator computes
    // the true top-K after consuming all spill parts.
    let top_candidates: Vec<CruiseTopCandidate> = accum.top.into_values().collect();
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
        rep_len_m: accum.rep_len_m,
        rep_len_w: accum.rep_len_w,
        fid_set,
        top_candidates,
    }
}

pub(super) fn accum_from_spill(row: CruiseSpillRow) -> CruiseAccum {
    let mut fid_set: std::collections::HashSet<u64> =
        std::collections::HashSet::with_capacity(row.fid_set.len());
    for fid in row.fid_set {
        fid_set.insert(fid);
    }
    let mut top: HashMap<u64, CruiseTopCandidate> =
        HashMap::with_capacity(row.top_candidates.len());
    let mut top_min_lmax = f32::INFINITY;
    for cand in row.top_candidates {
        if cand.peak_lmax_25m_db < top_min_lmax {
            top_min_lmax = cand.peak_lmax_25m_db;
        }
        top.insert(cand.flight_id, cand);
    }
    if top.is_empty() {
        top_min_lmax = f32::NEG_INFINITY;
    }
    CruiseAccum {
        sum_length_m: row.sum_length_m,
        rep_len_m: row.rep_len_m,
        rep_len_w: row.rep_len_w,
        rep_alt_m: row.rep_alt_m,
        rep_speed_kt: row.rep_speed_kt,
        weight: row.weight,
        rep_profile_idx: row.rep_profile_idx,
        fid_set,
        top,
        top_min_lmax,
        source_id: row.source_id,
        origin: row.origin,
    }
}

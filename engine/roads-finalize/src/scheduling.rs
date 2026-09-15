//! Bound parallel owner batches by their complete immutable Arrow halos and the process memory limit.

use crate::road_path;
use grid::Square;
use std::{collections::HashMap, path::Path};

pub(super) fn memory_budget() -> Result<u64, String> {
    let memory = std::fs::read_to_string("/proc/meminfo").map_err(|e| e.to_string())?;
    let mut budget = memory.lines().find_map(|line| line.strip_prefix("MemAvailable:"))
        .and_then(|value| value.split_whitespace().next()?.parse::<u64>().ok())
        .ok_or("missing available host memory")? * 1024;
    let groups = std::fs::read_to_string("/proc/self/cgroup").map_err(|e| e.to_string())?;
    if let Some(relative) = groups.lines().find_map(|line| line.strip_prefix("0::")) {
        let root = Path::new("/sys/fs/cgroup");
        let mut directory = root.join(relative.trim_start_matches('/'));
        loop {
            if let Ok(raw) = std::fs::read_to_string(directory.join("memory.max")) {
                if let Ok(limit) = raw.trim().parse::<u64>() { budget = budget.min(limit); }
            }
            if directory == root || !directory.pop() { break; }
        }
    }
    Ok(budget)
}

pub(super) fn allowances(year: &Path, squares: &[Square]) -> Result<Vec<u64>, String> {
    let sizes = squares.iter().map(|square| {
        std::fs::metadata(road_path(year, *square)).map(|meta| ((square.x, square.y), meta.len()))
            .map_err(|e| e.to_string())
    }).collect::<Result<HashMap<_, _>, _>>()?;
    Ok(squares.iter().map(|square| {
        let halo_bytes: u64 = grid::ring_squares(*square, 0.0).iter()
            .map(|neighbor| sizes.get(&(neighbor.x, neighbor.y)).copied().unwrap_or(0)).sum();
        // Resident input, concatenation, road/string copies, spatial index and
        // split/reblocked output coexist. The 154 MB three-owner sample peaked at
        // 698 MB; 16x plus a 512 MiB minimum reserves room for denser allocations.
        halo_bytes.saturating_mul(16).max(512 * 1024 * 1024)
    }).collect())
}

pub(super) fn batch_end(allowances: &[u64], start: usize, workers: usize, budget: u64) -> usize {
    let mut end = start;
    let mut reserved = 0_u64;
    while end < allowances.len() && end - start < workers {
        let next = reserved.saturating_add(allowances[end]);
        // An owner fitting the full process limit may use the reserve alone.
        if end > start && next > budget { break; }
        reserved = next;
        end += 1;
    }
    end
}

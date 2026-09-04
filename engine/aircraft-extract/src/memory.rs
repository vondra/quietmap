//! Shared host/cgroup memory budget for every aircraft pipeline stage.

/// How many days to run through Stage 0/1 concurrently, sized to the host's
/// RAM. Within-day work is already rayon-parallel over flights, so a single
/// day saturates every core; extra concurrency only overlaps each day's serial
/// I/O prefix/suffix, at the cost of holding that many days' flight + segment
/// working sets in RAM at once. Auto-scaling off total RAM keeps the extract
/// OOM-free on any host (laptop → 256 GB server) with zero manual tuning.
///
/// `peak_per_day_gb` is the effective RAM cost per concurrent day. For full
/// days it was calibrated from the 2026-05 global TTM extract: 4 concurrent
/// dense days OOM-killed a 110 GB cgroup at the segment-write peak, i.e.
/// ~28 GB per concurrent day once the shared DEM tile cache + per-day segment
/// accumulation are amortized in (the per-day segment set alone is ~16 GB; 28
/// folds in the fixed shared-cache share so the linear K model stays safely
/// below the limit). GA-filtered passes use a lower estimate — see
/// `ClassFilterArg::stage01_peak_per_day_gb`.
pub fn max_concurrent_days(num_days: usize, peak_per_day_gb: f64) -> usize {
    // Effective budget = min(host RAM, this process's cgroup memory limit) so a
    // container or `systemd-run -p MemoryMax=…` scope caps concurrency too —
    // sizing off host RAM alone re-OOMs inside a smaller cgroup.
    let total_gb = available_memory_bytes() as f64 / 1_000_000_000.0;
    // Budget 60%: leaves headroom for the OS, the shared raster cache, and the
    // parent process while staying well clear of the OOM boundary.
    let k = (total_gb * 0.60 / peak_per_day_gb).floor() as usize;
    k.clamp(1, num_days.max(1))
}

/// Total physical RAM in bytes from `/proc/meminfo` (Linux). Falls back to a
/// conservative 16 GB if it can't be read, so the cap stays safe off-Linux.
fn host_ram_bytes() -> u64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|kb| kb.parse::<u64>().ok())
        })
        .map(|kb| kb * 1024)
        .unwrap_or(16 * 1024 * 1024 * 1024)
}

/// This process's cgroup-v2 `memory.max` in bytes, if it is a real numeric
/// limit (not "max"). Resolves the cgroup path from `/proc/self/cgroup`
/// (v2 single line `0::<path>`). Returns None on cgroup v1 or no limit →
/// caller falls back to host RAM.
fn cgroup_memory_limit_bytes() -> Option<u64> {
    let cg = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let rel = cg.lines().find_map(|l| l.strip_prefix("0::"))?.trim();
    let raw = std::fs::read_to_string(format!("/sys/fs/cgroup{rel}/memory.max")).ok()?;
    raw.trim().parse::<u64>().ok()
}

/// Memory budget for concurrency sizing: the smaller of host RAM and this
/// process's cgroup limit, so the day-concurrency cap is OOM-safe in
/// containers and `systemd-run -p MemoryMax=…` scopes, not just on bare metal.
fn available_memory_bytes() -> u64 {
    let host = host_ram_bytes();
    cgroup_memory_limit_bytes().map_or(host, |lim| host.min(lim))
}

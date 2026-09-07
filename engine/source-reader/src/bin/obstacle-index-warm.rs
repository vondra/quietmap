//! Write every prepared cell's edge table (`structures.edges`) beside its
//! `structures.arrow` — the world build's last step, and the promote step
//! after a kernel change — so no popup pays a cold WKB parse. Idempotent and
//! resumable: a cell whose file is current is skipped. Standalone binary.
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use h3o::CellIndex;
use rayon::prelude::*;
use source_reader::structure_store::warm_cell_index;

/// A dense metro cell's build holds its whole Arrow table plus the builder in
/// memory — multi-GB transients — so builds run a few at a time to bound the
/// sweep's peak; small cells finish in milliseconds either way.
const BUILD_THREADS: usize = 4;
/// One progress line per this many builds: a cold world is ~120 k cells.
const PROGRESS_EVERY: usize = 500;

fn usage() -> ! {
    eprintln!("Usage: obstacle-index-warm <prepared_h3r4_dir>");
    std::process::exit(2);
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        usage();
    }
    let h3r4_dir = PathBuf::from(&args[1]);

    // Largest first: those are the cold clicks that hurt, and a long build
    // started early leaves the pool packed with small ones instead of idling.
    let mut cells: Vec<(u64, CellIndex)> = std::fs::read_dir(&h3r4_dir)
        .map_err(|e| format!("read {}: {e}", h3r4_dir.display()))?
        .flatten()
        .filter_map(|entry| {
            let cell: CellIndex = entry.file_name().to_str()?.parse().ok()?;
            let arrow = entry.path().join("structures.arrow");
            Some((std::fs::metadata(arrow).ok()?.len(), cell))
        })
        .collect();
    cells.sort_by_key(|(arrow_bytes, _)| std::cmp::Reverse(*arrow_bytes));

    let started = Instant::now();
    let built = AtomicUsize::new(0);
    let current = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    let built_bytes = AtomicU64::new(0);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(BUILD_THREADS)
        .build()
        .map_err(|e| e.to_string())?;
    pool.install(|| {
        cells.par_iter().for_each(|(arrow_bytes, cell)| {
            let t0 = Instant::now();
            match warm_cell_index(&h3r4_dir, *cell) {
                Ok(true) => {
                    let n = built.fetch_add(1, Ordering::Relaxed) + 1;
                    built_bytes.fetch_add(*arrow_bytes, Ordering::Relaxed);
                    if n.is_multiple_of(PROGRESS_EVERY) || t0.elapsed().as_secs() >= 5 {
                        println!(
                            "built {n}: cell={cell} arrow_bytes={arrow_bytes} in {:.0} ms, {:.0} s elapsed",
                            t0.elapsed().as_secs_f64() * 1000.0,
                            started.elapsed().as_secs_f64(),
                        );
                    }
                }
                Ok(false) => {
                    current.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => {
                    failed.fetch_add(1, Ordering::Relaxed);
                    eprintln!("failed cell={cell}: {e}");
                }
            }
        });
    });

    let failed = failed.load(Ordering::Relaxed);
    println!(
        "obstacle-index-warm: {} cells, built {} ({:.1} GB of Arrow), current {}, failed {failed}, {:.0} s",
        cells.len(),
        built.load(Ordering::Relaxed),
        built_bytes.load(Ordering::Relaxed) as f64 / 1e9,
        current.load(Ordering::Relaxed),
        started.elapsed().as_secs_f64(),
    );
    if failed > 0 {
        return Err(format!("{failed} cells failed to build"));
    }
    Ok(())
}

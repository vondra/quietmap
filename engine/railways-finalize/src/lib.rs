//! `railways-finalize PREPARED_YEAR` — split each square's interval files onto its
//! source pieces and write `rail_traffic_contract=1`.

mod encode;
pub mod horns;
mod merge;
mod parallel_tracks;
// Reuse the serving contract without linking source-reader's Node addon feature.
#[path = "../../source-reader/src/rail_traffic.rs"]
pub mod rail_traffic;
mod sources;
mod split;
mod square_intervals;
mod topology;
mod write;
mod yards;

use grid::Square;
use rayon::prelude::*;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

pub use write::finalize_square;

pub fn list_squares(prepared_year: &Path) -> std::io::Result<Vec<Square>> {
    let mut out = Vec::new();
    let z9 = prepared_year.join("z9");
    if !z9.is_dir() {
        return Ok(out);
    }
    for x in std::fs::read_dir(&z9)? {
        let x = x?;
        let Ok(xi) = x.file_name().to_string_lossy().parse::<u16>() else {
            continue;
        };
        for y in std::fs::read_dir(x.path())? {
            let y = y?;
            if let Ok(yi) = y.file_name().to_string_lossy().parse::<u16>() {
                out.push(Square { x: xi, y: yi });
            }
        }
    }
    Ok(out)
}

#[derive(Debug, Default)]
pub struct FinalizeYearStats {
    pub squares: usize,
    pub rewritten: usize,
    pub skipped_final: usize,
    pub rows_in: usize,
    pub rows_out: usize,
}

pub fn finalize_year(prepared_year: &Path) -> Result<FinalizeYearStats, String> {
    let squares = list_squares(prepared_year).map_err(|e| e.to_string())?;
    let rewritten = AtomicUsize::new(0);
    let skipped = AtomicUsize::new(0);
    let rows_in = AtomicUsize::new(0);
    let rows_out = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    squares
        .par_iter()
        .try_for_each(|square| -> Result<(), String> {
            let receipt = finalize_square(prepared_year, *square)?;
            if let Some(receipt) = receipt {
                rows_in.fetch_add(receipt.rows_in, Ordering::Relaxed);
                rows_out.fetch_add(receipt.rows_out, Ordering::Relaxed);
                if receipt.rewritten {
                    rewritten.fetch_add(1, Ordering::Relaxed);
                } else {
                    skipped.fetch_add(1, Ordering::Relaxed);
                }
            }
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if n.is_multiple_of(10_000) {
                eprintln!("railways-finalize: {n}/{} squares", squares.len());
            }
            Ok(())
        })?;
    Ok(FinalizeYearStats {
        squares: squares.len(),
        rewritten: rewritten.load(Ordering::Relaxed),
        skipped_final: skipped.load(Ordering::Relaxed),
        rows_in: rows_in.load(Ordering::Relaxed),
        rows_out: rows_out.load(Ordering::Relaxed),
    })
}

#[cfg(test)]
mod tests;

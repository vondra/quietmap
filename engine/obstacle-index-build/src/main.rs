//! Pipeline step `obstacle-index`: write `structures.qoix` beside every
//! `structures.arrow` of a prepared year directory, in parallel over squares.

use rayon::prelude::*;
use source_reader::square_obstacle_index::write_square_obstacle_index;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

fn squares(prepared_year_dir: &Path) -> std::io::Result<Vec<grid::Square>> {
    let mut out = Vec::new();
    for x in std::fs::read_dir(prepared_year_dir.join("z9"))? {
        let x = x?;
        let Ok(xi) = x.file_name().to_string_lossy().parse::<u16>() else {
            continue;
        };
        for y in std::fs::read_dir(x.path())? {
            let y = y?;
            if let Ok(yi) = y.file_name().to_string_lossy().parse::<u16>() {
                out.push(grid::Square { x: xi, y: yi });
            }
        }
    }
    Ok(out)
}

fn main() -> Result<(), String> {
    let prepared_year_dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("usage: obstacle-index-build <prepared_year_dir>")?,
    )
    .canonicalize()
    .map_err(|e| e.to_string())?;
    let squares = squares(&prepared_year_dir).map_err(|e| e.to_string())?;
    let (indexed, written, edges, done) = (
        AtomicUsize::new(0),
        AtomicUsize::new(0),
        AtomicUsize::new(0),
        AtomicUsize::new(0),
    );
    squares
        .par_iter()
        .try_for_each(|square| -> Result<(), String> {
            let dir = prepared_year_dir.join(grid::square_name(*square));
            if let Some(receipt) = write_square_obstacle_index(&dir, *square)? {
                indexed.fetch_add(1, Ordering::Relaxed);
                written.fetch_add(usize::from(receipt.written), Ordering::Relaxed);
                edges.fetch_add(receipt.edge_count, Ordering::Relaxed);
            }
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 10_000 == 0 {
                eprintln!("obstacle-index: {n}/{} squares", squares.len());
            }
            Ok(())
        })?;
    println!(
        "{{\"squares\":{},\"indexed\":{},\"written\":{},\"edges\":{}}}",
        squares.len(),
        indexed.load(Ordering::Relaxed),
        written.load(Ordering::Relaxed),
        edges.load(Ordering::Relaxed)
    );
    Ok(())
}

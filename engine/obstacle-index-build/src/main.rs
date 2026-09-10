//! Build every square's obstacle index for a prepared year directory into `<generation>/obstacle-index`.

use rayon::prelude::*;
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
            .ok_or("usage: obstacle_index_build <prepared_year_dir>")?,
    )
    .canonicalize()
    .map_err(|e| e.to_string())?;
    let data_dir = prepared_year_dir
        .parent()
        .ok_or("prepared year dir has no parent")?
        .to_path_buf();
    let root = source_reader::structure_store::index_cache_root(&data_dir)
        .ok_or("obstacle index cache is disabled (QM_OBSTACLE_INDEX_CACHE=0)")?;
    let (superseded, orphans) = source_reader::structure_store::sweep_index_dir(&root);
    let squares = squares(&prepared_year_dir).map_err(|e| e.to_string())?;
    let (indexed, edges, done) = (AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0));
    squares.par_iter().try_for_each(|square| -> Result<(), String> {
        if let Some(count) =
            source_reader::structure_store::ensure_square_index(&prepared_year_dir, &data_dir, *square)?
        {
            indexed.fetch_add(1, Ordering::Relaxed);
            edges.fetch_add(count, Ordering::Relaxed);
        }
        let n = done.fetch_add(1, Ordering::Relaxed) + 1;
        if n % 10_000 == 0 {
            eprintln!("obstacle-index: {n}/{} squares", squares.len());
        }
        Ok(())
    })?;
    println!(
        "{{\"squares\":{},\"indexed\":{},\"edges\":{},\"superseded_removed\":{superseded},\"orphans_removed\":{orphans}}}",
        squares.len(),
        indexed.load(Ordering::Relaxed),
        edges.load(Ordering::Relaxed)
    );
    Ok(())
}

//! Pipeline step `railways-finalize`: split each square's clipped interval evidence onto
//! native railway pieces and stamp `rail_traffic_contract=1`.

use railways_finalize::finalize_year;
use std::path::PathBuf;

fn main() -> Result<(), String> {
    let prepared_year_dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("usage: railways-finalize <prepared_year_dir>")?,
    )
    .canonicalize()
    .map_err(|e| e.to_string())?;
    let stats = finalize_year(&prepared_year_dir)?;
    println!(
        "{{\"squares\":{},\"rewritten\":{},\"skipped_final\":{},\"rows_in\":{},\"rows_out\":{}}}",
        stats.squares, stats.rewritten, stats.skipped_final, stats.rows_in, stats.rows_out
    );
    Ok(())
}

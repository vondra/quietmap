//! Pipeline step `railways-finalize`: split each square's clipped interval evidence onto
//! native railway pieces and stamp `rail_traffic_contract=1`.
//!
//! Subcommand `append-horns` runs after the finalize pass: it matches the FRA
//! and Transport Canada crossing inventories to finalized tracks and appends
//! horn approach rows (rail_type 6).

use railways_finalize::{finalize_year, horns::append_horns_year};
use std::path::PathBuf;

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|cmd| cmd == "append-horns") {
        let mut prepared: Option<PathBuf> = None;
        let mut fra: Option<PathBuf> = None;
        let mut tc: Option<PathBuf> = None;
        let mut rest = args[2..].iter();
        while let Some(arg) = rest.next() {
            match arg.as_str() {
                "--fra" => {
                    fra = Some(PathBuf::from(
                        rest.next().ok_or("append-horns --fra needs a CSV path")?,
                    ));
                }
                "--tc" => {
                    tc = Some(PathBuf::from(
                        rest.next().ok_or("append-horns --tc needs a CSV path")?,
                    ));
                }
                other if prepared.is_none() && !other.starts_with("--") => {
                    prepared = Some(PathBuf::from(other));
                }
                other => return Err(format!("append-horns: unexpected {other}")),
            }
        }
        let prepared = prepared.ok_or(
            "usage: railways-finalize append-horns <prepared_year_dir> --fra <csv> --tc <csv>",
        )?;
        let fra = fra.ok_or("append-horns: missing --fra <csv>")?;
        let tc = tc.ok_or("append-horns: missing --tc <csv>")?;
        let stats = append_horns_year(&prepared, &fra, &tc)?;
        println!(
            "{{\"fra_rows\":{},\"tc_rows\":{},\"sounding\":{},\"silent_zone\":{},\"squares_touched\":{},\"horns_appended\":{},\"crossings_unmatched\":{},\"crossings_skipped_present\":{}}}",
            stats.fra_rows,
            stats.tc_rows,
            stats.sounding,
            stats.silent_zone,
            stats.squares_touched,
            stats.horns_appended,
            stats.crossings_unmatched,
            stats.crossings_skipped_present
        );
        return Ok(());
    }
    let prepared_year_dir = PathBuf::from(
        args.get(1)
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

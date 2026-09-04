//! Surface-layer stream worker — road, rail, industrial, building and airport
//! ground ops from one preparation, at one wave's zoom.
//!
//! Cells arrive one per stdin line and each one answers on stderr with `start`,
//! then `done` or `fail`; stdout stays empty:
//!
//!   printf '841e309ffffffff tiles=4414,2786,4\n843e191ffffffff layers=road,rail\n' | \
//!     NOISE_GPU_PREPARED=/…/prepared relevant-source-surface \
//!       --stream --zoom 13 --output /…/tiles

use std::io::BufReader;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use clap::Parser;
use relevant_source_gpu::cell_stream::cell_requests;
use relevant_source_gpu::relevant_source_runner::{
    run_relevant_source_stream, RelevantSourceRunConfiguration,
};

/// The prepared root every GPU engine of the fleet is handed by its worker.
const PREPARED_ROOT_VARIABLE: &str = "NOISE_GPU_PREPARED";

/// Paint the surface layers of the H3 R4 cells arriving on stdin.
///
/// The lifecycle lines — `start <cell> <unix_ms>` when the card starts a cell,
/// then `done <cell> <statistics>` or `fail <cell> <message>` — are written to
/// STDERR, one per cell, and STDOUT stays empty. That is this painter's stated
/// contract, not a habit inherited from another binary: a supervisor can read
/// the protocol off one stream, and everything the engine's libraries print
/// arrives on that same stream in order with it.
#[derive(Debug, Parser)]
struct Arguments {
    /// Tile root: every painted tile lands at `<output>/<layer>/{z}/{x}/{y}.bin`.
    #[arg(long)]
    output: PathBuf,
    /// Web-Mercator zoom of the painted tiles. Both launcher-supported zooms
    /// use the same exact ray cadence.
    #[arg(long)]
    zoom: u8,
    /// Read cells from stdin, optionally followed by `layers=<csv>` and/or
    /// `tiles=x,y,side`, one cell per line.
    #[arg(long)]
    stream: bool,
}

fn main() -> Result<ExitCode> {
    if std::env::args().skip(1).eq(["--resource-limits"]) {
        println!(
            "{{\"maximum_tile_bytes\":{}}}",
            tile_painter::wire_hm3::maximum_encoded_tile_bytes()
        );
        return Ok(ExitCode::SUCCESS);
    }
    let arguments = Arguments::parse();
    if !arguments.stream {
        bail!("--stream is this painter's only mode: cells arrive one per stdin line");
    }
    if !matches!(arguments.zoom, 12 | 13) {
        bail!(
            "--zoom {} is not a supported launcher zoom (12 or 13)",
            arguments.zoom
        );
    }
    let prepared = std::env::var_os(PREPARED_ROOT_VARIABLE).with_context(|| {
        format!("{PREPARED_ROOT_VARIABLE} must name the prepared root this painter reads")
    })?;
    let dataset_year = std::env::var("DATA_YEAR")
        .ok()
        .filter(|year| !year.is_empty());
    let configuration = RelevantSourceRunConfiguration::for_prepared_root(
        PathBuf::from(prepared),
        arguments.output,
        arguments.zoom,
        dataset_year.as_deref(),
    )?;
    let failed_cells = run_relevant_source_stream(
        &configuration,
        cell_requests(BufReader::new(std::io::stdin())),
    )?;
    if failed_cells > 0 {
        eprintln!("relevant-source-surface: {failed_cells} cell(s) failed");
    }
    // The count itself, saturated: any failure is non-zero, and a supervisor
    // that reads the status learns how many cells it must re-queue.
    Ok(ExitCode::from(
        u8::try_from(failed_cells).unwrap_or(u8::MAX),
    ))
}

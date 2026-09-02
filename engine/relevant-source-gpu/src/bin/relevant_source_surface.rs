//! Surface-layer stream worker — road, rail, industrial, building and airport
//! ground ops from one preparation, at one wave's zoom.
//!
//! Cells arrive one per stdin line and each one answers with `start`, then
//! `done` or `fail`, on stderr:
//!
//!   printf '841e309ffffffff\n843e191ffffffff layers=road,rail\n' | \
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

#[derive(Debug, Parser)]
struct Arguments {
    /// Tile root: every painted tile lands at `<output>/<layer>/{z}/{x}/{y}.bin`.
    #[arg(long)]
    output: PathBuf,
    /// Web-Mercator zoom of the painted tiles: 12 for W1, 13 for W2.
    #[arg(long)]
    zoom: u8,
    /// Read cells from stdin: `<r4hex>` or `<r4hex> layers=<csv>`, one per line.
    #[arg(long)]
    stream: bool,
}

fn main() -> Result<ExitCode> {
    let arguments = Arguments::parse();
    if !arguments.stream {
        bail!("--stream is this painter's only mode: cells arrive one per stdin line");
    }
    if !matches!(arguments.zoom, 12 | 13) {
        bail!(
            "--zoom {} is neither W1 (12) nor W2 (13); the cadence contract is defined for those two",
            arguments.zoom
        );
    }
    let prepared = std::env::var_os(PREPARED_ROOT_VARIABLE).with_context(|| {
        format!("{PREPARED_ROOT_VARIABLE} must name the prepared root this painter reads")
    })?;
    let configuration = RelevantSourceRunConfiguration::for_prepared_root(
        PathBuf::from(prepared),
        arguments.output,
        arguments.zoom,
    );
    let failed_cells = run_relevant_source_stream(
        &configuration,
        cell_requests(BufReader::new(std::io::stdin())),
    )?;
    if failed_cells > 0 {
        eprintln!("relevant-source-surface: {failed_cells} cell(s) failed");
        return Ok(ExitCode::FAILURE);
    }
    Ok(ExitCode::SUCCESS)
}

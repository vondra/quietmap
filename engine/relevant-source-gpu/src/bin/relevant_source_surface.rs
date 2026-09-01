//! Fixed W1 road/rail command for the persisted relevant-source block architecture.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use relevant_source_gpu::relevant_source_runner::{
    run_relevant_source_w1, RelevantSourceW1Configuration,
};
use relevant_source_gpu::source_frame::BLOCK_COUNT;

#[derive(Debug, Parser)]
struct Arguments {
    #[arg(long)]
    prepared_dir: PathBuf,
    #[arg(long)]
    h3r4_dir: PathBuf,
    #[arg(long)]
    output: PathBuf,
    /// One hexadecimal H3 R4 cell per line; every owned W1 tile is painted.
    #[arg(long)]
    regions_file: PathBuf,
}

fn main() -> Result<()> {
    let arguments = Arguments::parse();
    let regions = read_regions(&arguments.regions_file)?;
    let measurement = run_relevant_source_w1(&RelevantSourceW1Configuration {
        prepared_directory: arguments.prepared_dir,
        h3r4_directory: arguments.h3r4_dir,
        output_directory: arguments.output,
        regions,
    })?;
    let attempted_pairs = measurement.attempted_pairs();
    let gpu_seconds = measurement.gpu_seconds();
    let gpu_nanoseconds_per_pair = if attempted_pairs == 0 {
        0.0
    } else {
        gpu_seconds * 1.0e9 / attempted_pairs as f64
    };
    eprintln!(
        "relevant-source-w1 wall_s={:.6} cpu_s={:.6} gpu_s={:.6} gpu_ns_per_pair={:.3} \
         source_load_s={:.6} raster_receiver_s={:.6} host_tile_s={:.6}",
        measurement.wall_seconds,
        measurement.cpu_seconds,
        gpu_seconds,
        gpu_nanoseconds_per_pair,
        measurement.source_load_seconds,
        measurement.raster_and_receiver_seconds,
        measurement.host_tile_seconds,
    );
    print_layer("road", measurement.road);
    print_layer("rail", measurement.rail);
    Ok(())
}

fn print_layer(
    name: &str,
    measurement: relevant_source_gpu::relevant_source_runner::LayerMeasurement,
) {
    let blocks = measurement.tiles * BLOCK_COUNT as u64;
    let relevant_per_block = if blocks == 0 {
        0.0
    } else {
        measurement.relevant_source_references as f64 / blocks as f64
    };
    eprintln!(
        "relevant-source-layer name={name} loaded_sources={} tiles={} corner_pairs={} \
         pixel_pairs={} relevant_per_block={:.3} corner_gpu_s={:.6} paint_gpu_s={:.6} bytes={}",
        measurement.loaded_sources,
        measurement.tiles,
        measurement.corner_pairs,
        measurement.pixel_pairs,
        relevant_per_block,
        measurement.corner_gpu_milliseconds / 1000.0,
        measurement.paint_gpu_milliseconds / 1000.0,
        measurement.output_bytes,
    );
}

fn read_regions(path: &PathBuf) -> Result<Vec<u64>> {
    let text =
        fs::read_to_string(path).with_context(|| format!("read region list {}", path.display()))?;
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            u64::from_str_radix(line, 16).with_context(|| format!("invalid H3 R4 cell {line:?}"))
        })
        .collect()
}

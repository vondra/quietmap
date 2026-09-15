//! Stage eight generation-validated HM3 PMTiles archives from the existing owner authority.
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
struct Arguments {
    #[arg(long)]
    authority: PathBuf,
    #[arg(long)]
    generation: String,
    #[arg(long)]
    output: PathBuf,
    #[arg(long)]
    build: String,
}

fn main() -> anyhow::Result<()> {
    let args = Arguments::parse();
    tile_painter::heatmap_pack::pack(&args.authority, &args.generation, &args.output, &args.build)
}

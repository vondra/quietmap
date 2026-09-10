//! Validate one self-contained distributed corner artifact before two-copy acknowledgment.
use anyhow::{ensure, Context, Result};
use clap::{Parser, ValueEnum};
use tile_painter::{
    corner_directory::validate_owner_result, corner_store::CornerGeneration, edge_bundle,
};

#[derive(Clone, Copy, ValueEnum)]
enum Phase {
    Edge,
    Owner,
}

#[derive(Parser)]
struct Arguments {
    #[arg(long)]
    generation: String,
    #[arg(long)]
    phase: Phase,
    #[arg(long)]
    owner: String,
    #[arg(long)]
    epoch: u64,
    #[arg(long)]
    artifact: std::path::PathBuf,
    #[arg(long)]
    publish_root: Option<std::path::PathBuf>,
}

fn parse_generation(value: &str) -> Result<CornerGeneration> {
    ensure!(
        value.len() == 64,
        "generation must be 32-byte lowercase hex"
    );
    let mut bytes = [0_u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        let pair = &value[index * 2..index * 2 + 2];
        ensure!(
            pair.bytes()
                .all(|value| value.is_ascii_digit() || (b'a'..=b'f').contains(&value)),
            "generation must be lowercase hex"
        );
        *byte = u8::from_str_radix(pair, 16)?;
    }
    Ok(CornerGeneration(bytes))
}

fn main() -> Result<()> {
    let args = Arguments::parse();
    let generation = parse_generation(&args.generation)?;
    let owner = grid::parse_square_name(&args.owner).context("invalid z9 owner")?;
    let receipt = match args.phase {
        Phase::Edge => {
            let bundle = edge_bundle::read(&args.artifact, generation)?;
            ensure!(
                bundle.owner == owner && bundle.epoch == args.epoch,
                "edge artifact belongs to another owner or epoch"
            );
            bundle.receipt
        }
        Phase::Owner => {
            validate_owner_result(&args.artifact, generation, owner)?;
            let (_, receipt) = edge_bundle::read_receipt(&args.artifact, generation, owner)?;
            receipt
        }
    };
    if let Some(root) = args.publish_root {
        ensure!(
            matches!(args.phase, Phase::Owner),
            "only complete owner results publish a generation receipt"
        );
        receipt.publish(&root)?;
    }
    Ok(())
}

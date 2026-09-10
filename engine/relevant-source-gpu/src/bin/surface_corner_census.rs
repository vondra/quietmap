//! Measure at most four real canonical vertices and SQLite payloads without painting any tile.
use anyhow::{ensure, Context, Result};
use clap::Parser;
use grid::surface_corner::SurfaceCorner;
use relevant_source_gpu::{
    cuda_bridge::RelevantSourceCuda,
    input_manifest::{file_digest, parse_digest, InputManifest},
    surface_gpu::SurfaceGpu,
    surface_scene::SurfaceScene,
};
use std::path::PathBuf;
use tile_painter::{
    corner_store::CornerStore,
    generation_receipt::{GenerationReceipt, SURFACE_CODE_DIGEST},
};

#[derive(Parser)]
struct Arguments {
    #[arg(long)]
    prepared_year: PathBuf,
    #[arg(long)]
    raster_root: PathBuf,
    #[arg(long)]
    raster_catalog_sha256: String,
    #[arg(long)]
    input_manifest: PathBuf,
    #[arg(long)]
    input_manifest_sha256: String,
    /// Global block-edge x,y; repeat up to four times within one canonical owner.
    #[arg(long, required = true)]
    corner: Vec<String>,
    /// New SQLite measurement file; never creates HM3 output or completion receipts.
    #[arg(long)]
    output: PathBuf,
}
fn main() -> Result<()> {
    let args = Arguments::parse();
    ensure!(
        !args.corner.is_empty() && args.corner.len() <= 4,
        "one to four corners required"
    );
    ensure!(
        !args.output.try_exists()?,
        "census output must be a new file"
    );
    let vertices: Vec<_> = args
        .corner
        .iter()
        .map(|value| -> Result<_> {
            let (x, y) = value.split_once(',').context("corner must be x,y")?;
            SurfaceCorner::new(x.parse()?, y.parse()?).context("invalid canonical corner")
        })
        .collect::<Result<_>>()?;
    let owner = vertices[0].owner();
    ensure!(
        vertices.iter().all(|vertex| vertex.owner() == owner),
        "all census vertices need one owner"
    );
    let manifest = InputManifest::open(
        &args.input_manifest,
        parse_digest(&args.input_manifest_sha256)?,
    )?;
    let rasters_digest = file_digest(&args.raster_root.join(raster_reader::catalog::CATALOG_FILE))?;
    ensure!(
        rasters_digest == parse_digest(&args.raster_catalog_sha256)?,
        "raster catalog digest mismatch"
    );
    let receipt = GenerationReceipt {
        sources: manifest.digest,
        rasters: rasters_digest,
        code: SURFACE_CODE_DIGEST,
        producer: file_digest(&std::env::current_exe()?)?,
    };
    let rasters = raster_reader::RealRasters::new(&args.raster_root);
    let scene = SurfaceGpu::upload(SurfaceScene::load(
        owner,
        &args.prepared_year,
        &manifest,
        &rasters,
    )?)?;
    let cuda = RelevantSourceCuda::initialize()?;
    let mut store = CornerStore::open(&args.output, receipt.generation(), owner)?;
    let energy = store.resolve(&vertices, |_, missing| {
        scene.evaluate_corners(&cuda, missing)
    })?;
    let mut sparse_pair_count = 0usize;
    let mut candidate_pair_count = 0usize;
    for (vertex, entries) in vertices.iter().zip(&energy) {
        let candidates: [usize; 5] = std::array::from_fn(|layer| {
            entries
                .0
                .iter()
                .filter(|entry| entry.layer as usize == layer)
                .count()
        });
        let nonzero: [usize; 5] = std::array::from_fn(|layer| {
            entries
                .0
                .iter()
                .filter(|entry| {
                    entry.layer as usize == layer && entry.periods.iter().any(|value| *value > 0.0)
                })
                .count()
        });
        candidate_pair_count += candidates.iter().sum::<usize>();
        sparse_pair_count += nonzero.iter().sum::<usize>();
        let totals = store.read(*vertex)?.context("produced corner absent")?;
        println!(
            "corner={:?} candidates={candidates:?} nonzero={nonzero:?} mean_period_power={:?}",
            vertex.coordinates(),
            totals.0
        );
    }
    drop(store);
    let database = rusqlite::Connection::open_with_flags(
        &args.output,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let payload: (u64, u64, u64) = database.query_row("SELECT (SELECT sum(length(totals)) FROM corners),(SELECT sum(length(energy)) FROM staging),(SELECT count(*) FROM sources)", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    println!("sqlite_bytes={} permanent_totals_bytes={} staging_bytes={} dictionary_sources={} candidate_pairs={candidate_pair_count} nonzero_pairs={sparse_pair_count} sparse_pair_payload_bytes={}", std::fs::metadata(&args.output)?.len(), payload.0, payload.1, payload.2, sparse_pair_count * 16);
    Ok(())
}

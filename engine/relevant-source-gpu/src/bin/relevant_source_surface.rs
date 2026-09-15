//! Produce canonical surface corners or one complete z9 seven-layer noise result.
use anyhow::{ensure, Context, Result};
use clap::{Parser, Subcommand};
use grid::surface_corner::{
    owner_dependency_owners, owner_edge_corners, tile_corners, SurfaceCorner,
};
use relevant_source_gpu::{
    airborne_field::AirborneScene,
    cruise_field::CruiseField,
    cuda_bridge::RelevantSourceCuda,
    input_manifest::{file_digest, parse_digest, InputManifest},
    paint_tile::paint_tile,
    surface_gpu::SurfaceGpu,
    surface_scene::SurfaceScene,
    tile_receivers::TileReceivers,
};
use std::{collections::BTreeMap, path::PathBuf};
use tile_painter::{
    corner_directory::CornerDirectory,
    corner_store::{CornerEnergy, CornerStore},
    durable_directory, edge_bundle,
    generation_receipt::{GenerationReceipt, SURFACE_CODE_DIGEST},
    hm3::encode_all_period_powers,
};

#[derive(Parser)]
struct Arguments {
    #[arg(long)]
    prepared_year: PathBuf,
    #[arg(long)]
    raster_root: PathBuf,
    /// SQLite input_files(relative_path TEXT PRIMARY KEY, sha256 BLOB NOT NULL).
    #[arg(long)]
    input_manifest: PathBuf,
    /// SHA256 anchored when the source manifest was published.
    #[arg(long)]
    input_manifest_sha256: String,
    /// Canonical compute unit, e.g. z9/276/173.
    #[arg(long)]
    square: String,
    #[command(subcommand)]
    phase: Phase,
}

#[derive(Subcommand)]
enum Phase {
    /// Compute only the canonical north/west owner boundary artifact.
    Edge {
        #[arg(long)]
        epoch: u64,
        #[arg(long)]
        result: PathBuf,
    },
    /// Import exact dependency bundles and publish all 256 tiles with seven layers and total.
    Owner {
        #[arg(long = "edge-bundle", required = true)]
        edge_bundles: Vec<PathBuf>,
        #[arg(long)]
        work: PathBuf,
        #[arg(long)]
        result: PathBuf,
    },
}

fn main() -> Result<()> {
    let args = Arguments::parse();
    let owner = grid::parse_square_name(&args.square)
        .filter(|square| square.x < 512 && square.y < 512)
        .context("invalid z9 owner")?;
    let manifest = InputManifest::open(
        &args.input_manifest,
        parse_digest(&args.input_manifest_sha256)?,
    )?;
    let receipt = GenerationReceipt {
        sources: manifest.digest,
        code: SURFACE_CODE_DIGEST,
        producer: file_digest(&std::env::current_exe()?)?,
    };
    let rasters = raster_reader::RealRasters::new(&args.raster_root);
    let cuda = RelevantSourceCuda::initialize()?;
    match args.phase {
        Phase::Edge { epoch, result } => produce_edge(
            owner,
            epoch,
            &result,
            receipt,
            &args.prepared_year,
            &manifest,
            &rasters,
            &cuda,
        ),
        Phase::Owner {
            edge_bundles,
            work,
            result,
        } => paint_owner(
            owner,
            &edge_bundles,
            &work,
            &result,
            receipt,
            &args.prepared_year,
            &manifest,
            &rasters,
            &cuda,
        ),
    }
}

/// The card holds the scene only when a source exists; a source-less owner still
/// loads and checks its rasters, then paints silence on the host.
fn scene(
    owner: grid::Square,
    prepared_year: &std::path::Path,
    manifest: &InputManifest,
    rasters: &raster_reader::RealRasters,
) -> Result<Option<SurfaceGpu>> {
    let scene = SurfaceScene::load(owner, prepared_year, manifest, rasters)?;
    (!scene.sources.is_empty())
        .then(|| SurfaceGpu::upload(scene))
        .transpose()
}

fn evaluate_corners(
    scene: Option<&SurfaceGpu>,
    cuda: &RelevantSourceCuda,
    corners: &[SurfaceCorner],
) -> Result<Vec<CornerEnergy>> {
    match scene {
        Some(scene) if !scene.host.sources.is_empty() => scene.evaluate_corners(cuda, corners),
        _ => Ok(vec![CornerEnergy(Vec::new()); corners.len()]),
    }
}

fn produce_edge(
    owner: grid::Square,
    epoch: u64,
    result: &std::path::Path,
    receipt: GenerationReceipt,
    prepared_year: &std::path::Path,
    manifest: &InputManifest,
    rasters: &raster_reader::RealRasters,
    cuda: &RelevantSourceCuda,
) -> Result<()> {
    durable_directory::create_dir_all(result.parent().context("edge result has no parent")?)?;
    let vertices = owner_edge_corners(owner);
    let mut store = CornerStore::open(result, receipt.generation(), owner)?;
    let scene = scene(owner, prepared_year, manifest, rasters)?;
    for corners in vertices.chunks(grid::surface_corner::CORNER_COUNT) {
        store.resolve(corners, |canonical_owner, missing| {
            ensure!(canonical_owner == owner, "edge vertex owner changed");
            evaluate_corners(scene.as_ref(), cuda, missing)
        })?;
    }
    edge_bundle::seal(&mut store, epoch, receipt)?;
    eprintln!(
        "{} edge_vertices={} epoch={epoch}",
        grid::square_name(owner),
        vertices.len()
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn paint_owner(
    owner: grid::Square,
    edge_bundles: &[PathBuf],
    work: &std::path::Path,
    result: &std::path::Path,
    receipt: GenerationReceipt,
    prepared_year: &std::path::Path,
    manifest: &InputManifest,
    rasters: &raster_reader::RealRasters,
    cuda: &RelevantSourceCuda,
) -> Result<()> {
    let corner_root = work.join("corners");
    receipt.publish(&corner_root)?;
    let directory = CornerDirectory::new(&corner_root, receipt.generation());
    let expected = owner_dependency_owners(owner);
    let mut imported = BTreeMap::new();
    for path in edge_bundles {
        let (dependency_owner, epoch) = directory.import_edge_bundle(path)?;
        ensure!(
            imported
                .insert((dependency_owner.x, dependency_owner.y), epoch)
                .is_none(),
            "duplicate edge owner bundle"
        );
    }
    ensure!(
        imported.keys().copied().collect::<Vec<_>>()
            == expected
                .iter()
                .map(|owner| (owner.x, owner.y))
                .collect::<Vec<_>>(),
        "owner edge bundle set is incomplete or contains another owner"
    );

    let ((x0, x1), (y0, y1)) = grid::owned_z13(owner);
    let mut pending = Vec::new();
    for y in y0..y1 {
        for x in x0..x1 {
            if let Some(committed) = directory.committed(x, y)? {
                directory.release(committed)?;
            } else {
                pending.push((x, y));
            }
        }
    }
    if !pending.is_empty() {
        let scene =
            SurfaceGpu::upload(SurfaceScene::load(owner, prepared_year, manifest, rasters)?)?;
        let airborne = AirborneScene::load(owner, prepared_year, manifest, rasters)?;
        let cruise = CruiseField::load(owner, prepared_year, manifest, rasters)?;
        let mut produced = 0usize;
        for (x, y) in pending {
            let vertices = tile_corners(x, y).expect("owned z13 tile");
            let corners = directory.resolve(&vertices, |canonical_owner, missing| {
                ensure!(
                    canonical_owner == owner,
                    "foreign edge bundle was not imported"
                );
                let values = evaluate_corners(Some(&scene), cuda, missing)?;
                produced += missing.len();
                Ok(values)
            })?;
            let receivers = TileReceivers::prepare(&scene.host, x, y)?;
            let surface = paint_tile(cuda, &scene, x, y, &receivers, &corners)?;
            let airborne_power = airborne.period_powers(&scene.host, &receivers)?;
            let cruise_power = cruise.period_powers(&scene.host, &receivers)?;
            let tiles = encode_all_period_powers(
                [
                    &surface[0],
                    &surface[1],
                    &surface[2],
                    &surface[3],
                    &surface[4],
                    &airborne_power,
                    &cruise_power,
                ],
                &receivers.indoor_attenuation,
            )?;
            let committed = directory.write(x, y, &tiles)?;
            directory.release(committed)?;
            eprintln!(
                "{} painted_z13={x}/{y} canonical_corners_produced={produced}",
                grid::square_name(owner)
            );
        }
    }
    directory.publish_owner_result(owner, result)?;
    eprintln!(
        "{} owner_complete edge_epochs={imported:?}",
        grid::square_name(owner)
    );
    Ok(())
}

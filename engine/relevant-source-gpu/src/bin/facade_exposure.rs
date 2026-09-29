//! The façade-exposure stage of the prepared build: one z9 square's `facade_exposure.arrow`.
use anyhow::{Context, Result};
use clap::Parser;
use relevant_source_gpu::{
    airborne_field::AirborneScene,
    building_exposure_table::{physics_generation_stamp, source_layer_order},
    cruise_field::CruiseField,
    cuda_bridge::RelevantSourceCuda,
    facade_exposure::facade_exposure_rows,
    input_manifest::{parse_digest, InputManifest},
    surface_gpu::SurfaceGpu,
    surface_scene::SurfaceScene,
};
use square_store::facade_exposure_contract::{self as contract, FacadeExposureRow};
use std::{collections::HashMap, io::Write, path::PathBuf, time::Instant};
use tile_painter::{generation_receipt::hex_digest, hm3::SOURCE_LAYERS};

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
    /// Where to write the square's facade_exposure.arrow (its prepared square directory).
    #[arg(long)]
    output: PathBuf,
}

fn main() -> Result<()> {
    let args = Arguments::parse();
    let started = Instant::now();
    let owner = grid::parse_square_name(&args.square)
        .filter(|square| square.x < 512 && square.y < 512)
        .context("invalid z9 owner")?;
    let manifest = InputManifest::open(
        &args.input_manifest,
        parse_digest(&args.input_manifest_sha256)?,
    )?;
    let rasters = raster_reader::RealRasters::new(&args.raster_root);
    let cuda = RelevantSourceCuda::initialize()?;
    let structures = format!("z9/{}/{}/structures.arrow", owner.x, owner.y);
    let (structures_bytes, structures_digest) = manifest
        .read_arrow(&args.prepared_year, &structures)?
        .with_context(|| format!("{structures} is required, including when empty"))?;
    let footprints = source_reader::structure_store::enclosed_building_footprints(
        &structures_bytes,
        std::path::Path::new(&structures),
    )
    .map_err(anyhow::Error::msg)?;
    let scene = SurfaceGpu::upload(SurfaceScene::load(
        owner,
        &args.prepared_year,
        &manifest,
        &rasters,
    )?)?;
    let airborne = AirborneScene::load(owner, &args.prepared_year, &manifest, &rasters)?;
    let cruise = CruiseField::load(owner, &args.prepared_year, &manifest, &rasters)?;
    let load_seconds = started.elapsed().as_secs_f64();
    let (rows, receipt) = facade_exposure_rows(&cuda, &scene, &airborne, &cruise, &footprints)?;

    let metadata = HashMap::from(
        [
            (contract::CONTRACT_KEY, contract::CONTRACT.to_string()),
            ("grid", square_store::store::GRID_CONTRACT_Z30.to_string()),
            (contract::LAYER_ORDER_KEY, source_layer_order()),
            (contract::STRUCTURES_SHA256_KEY, hex_digest(structures_digest)),
            (contract::STRUCTURES_BYTES_KEY, structures_bytes.len().to_string()),
            (contract::PHYSICS_GENERATION_KEY, physics_generation_stamp()),
            (contract::INPUT_MANIFEST_SHA256_KEY, hex_digest(manifest.digest)),
            ("buildings", receipt.buildings.to_string()),
            ("facade_receivers", receipt.receivers.to_string()),
            (
                "buildings_without_exposed_facade",
                receipt.buildings_without_exposed_facade.to_string(),
            ),
        ]
        .map(|(key, value)| (key.to_string(), value)),
    );
    let (rows, bboxes): (Vec<FacadeExposureRow>, Vec<[f64; 4]>) = rows.into_iter().unzip();
    let columns = contract::columns(&rows, SOURCE_LAYERS.len()).map_err(anyhow::Error::msg)?;
    let (schema, batches) = arrow_batching::blocked_by_z14_cell(
        contract::schema(SOURCE_LAYERS.len(), metadata),
        columns,
        &bboxes,
    )?;
    let temporary = args.output.with_extension("arrow.partial");
    {
        let mut file = std::fs::File::create(&temporary)?;
        let mut writer = arrow::ipc::writer::FileWriter::try_new(&mut file, &schema)?;
        for batch in &batches {
            writer.write(batch)?;
        }
        writer.finish()?;
        drop(writer);
        file.flush()?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, &args.output)?;
    eprintln!(
        "{} facade_exposure buildings={} receivers={} without_exposed_facade={} \
         load_s={load_seconds:.1} placement_s={:.1} surface_s={:.1} airborne_s={:.1} \
         cruise_s={:.1} total_s={:.1}",
        grid::square_name(owner),
        receipt.buildings,
        receipt.receivers,
        receipt.buildings_without_exposed_facade,
        receipt.placement_seconds,
        receipt.surface_seconds,
        receipt.airborne_seconds,
        receipt.cruise_seconds,
        started.elapsed().as_secs_f64(),
    );
    Ok(())
}

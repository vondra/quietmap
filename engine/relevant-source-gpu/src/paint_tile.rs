//! Reuse saved corner bytes by immutable source identity, then paint five surface layers.
use crate::{
    cuda_bridge::{DeviceBuffer, RelevantSourceCuda},
    relevance_partition::build_relevant_source_partition,
    source_frame::*,
    surface_gpu::SurfaceGpu,
    tile_receivers::TileReceivers,
    tile_source_incidence::{build_tile_source_incidence, TileMetricLattice, TileSourceIncidence},
};
use anyhow::{ensure, Context, Result};
use std::collections::BTreeMap;
use tile_painter::corner_store::CornerEnergy;

pub fn paint_tile(
    cuda: &RelevantSourceCuda,
    scene: &SurfaceGpu,
    x: u32,
    y: u32,
    corners: &[CornerEnergy],
) -> Result<Vec<tile_painter::hm3::EncodedHm3>> {
    ensure!(
        corners.len() == CORNER_COUNT,
        "incomplete saved tile corners"
    );
    let receivers = TileReceivers::prepare(&scene.host, x, y)?;
    let device_sources: Vec<_> = scene
        .host
        .sources
        .iter()
        .map(|source| source.device)
        .collect();
    let lattice = TileMetricLattice::for_tile(&scene.host.frame, 13, x, y);
    let mut base = build_tile_source_incidence(&device_sources, &lattice);
    receivers.admit_enclosed_sources(&device_sources, &mut base);
    let local_ids: BTreeMap<_, _> = scene
        .host
        .sources
        .iter()
        .enumerate()
        .map(|(index, source)| ((source.layer, source.identity), index as u32))
        .collect();
    let dx = DeviceBuffer::from_slice(&receivers.x)?;
    let dy = DeviceBuffer::from_slice(&receivers.y)?;
    let altitude = DeviceBuffer::from_slice(&receivers.altitude)?;
    let reflection = DeviceBuffer::from_slice(&receivers.reflection)?;
    let floor = DeviceBuffer::from_slice(&receivers.floor)?;
    let mut tiles = Vec::new();
    for (layer, source_id) in tile_painter::hm3::SURFACE_SOURCE_IDS
        .into_iter()
        .enumerate()
    {
        let mut incidence = TileSourceIncidence {
            corner_offsets: vec![0],
            corner_source_indices: Vec::new(),
            local_source_indices_by_block: base
                .local_source_indices_by_block
                .iter()
                .map(|indices| {
                    indices
                        .iter()
                        .copied()
                        .filter(|index| scene.host.sources[*index as usize].layer == layer as u8)
                        .collect()
                })
                .collect(),
        };
        let mut energies = Vec::new();
        for corner in corners {
            let mut pairs = Vec::new();
            for energy in corner.0.iter().filter(|source| source.layer == layer as u8) {
                let index = *local_ids
                    .get(&(energy.layer, energy.source))
                    .context("saved corner source absent from complete paint scene")?;
                pairs.push((index, energy.periods));
            }
            pairs.sort_by_key(|pair| pair.0);
            for (index, energy) in pairs {
                incidence.corner_source_indices.push(index);
                energies.push(energy);
            }
            incidence.corner_offsets.push(energies.len().try_into()?);
        }
        let weights = [0, 1, 2].map(|period| {
            let mut db = [f64::NEG_INFINITY; 3];
            db[period] = 0.0;
            10.0_f64.powf(noise_compute::periods::compute_lden(db[0], db[1], db[2]) / 10.0)
        });
        let mut partition = build_relevant_source_partition(&incidence, &energies, weights)?;
        receivers.clear_enclosed_background(&mut partition);
        let background: Vec<_> = partition
            .background_corner_energy
            .iter()
            .flatten()
            .flatten()
            .copied()
            .collect();
        let (energy, _) = cuda.paint_tile(
            &scene.pointers(),
            &floor,
            &DeviceBuffer::from_slice(&partition.block_offsets)?,
            &DeviceBuffer::from_slice(&partition.relevant_source_indices)?,
            &DeviceBuffer::from_slice(&background)?,
            &dx,
            &dy,
            &altitude,
            &reflection,
        )?;
        tiles.push(tile_painter::hm3::encode_period_power(
            &energy,
            source_id,
            &receivers.indoor_attenuation,
        )?);
    }
    Ok(tiles)
}

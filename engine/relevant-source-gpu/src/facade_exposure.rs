//! The façade-exposure stage: every §2.8 façade receiver of a square's buildings, exact, on the GPU.
//!
//! Per enclosed building the square owns, the receivers of
//! `noise_compute::facade_receivers` are evaluated with the painter's kernels —
//! every source within its reach, no relevance partition, no corner background —
//! plus the airborne and cruise fields, and the receiver with the highest
//! all-source Lden becomes the building's row of `facade_exposure.arrow`
//! (ties: the lowest canonical index). The popup recomputes that receiver; the
//! painter paints its layer powers into the building's pixels.
use crate::{
    airborne_field::AirborneScene,
    cruise_field::CruiseField,
    cuda_bridge::{DeviceBuffer, RelevantSourceCuda},
    facade_exposure_choice::*,
    receiver_points::ReceiverPoints,
    source_frame::*,
    surface_gpu::SurfaceGpu,
};
use anyhow::{ensure, Result};
use grid::poly::GridPolygons;
use noise_compute::{
    facade_receivers::{exposed_facade_receivers, FacadeReceiverPosition},
    propagation::obstacle_index::FootprintKey,
};
use rayon::prelude::*;
use square_store::facade_exposure_contract::{ChosenFacadeReceiver, FacadeExposureRow};
use std::time::Instant;
use tile_painter::hm3::{SOURCE_LAYERS, SURFACE_LAYERS};


/// What one square's stage run did, for its log line and schema metadata.
#[derive(Clone, Copy, Debug, Default)]
pub struct FacadeExposureReceipt {
    pub buildings: usize,
    pub receivers: usize,
    pub buildings_without_exposed_facade: usize,
    pub placement_seconds: f64,
    pub surface_seconds: f64,
    pub airborne_seconds: f64,
    pub cruise_seconds: f64,
}

/// One building's row and its footprint's bbox in degrees `[min_lat, min_lon, max_lat, max_lon]`.
pub type FacadeExposureRowWithBbox = (FacadeExposureRow, [f64; 4]);

/// One row per footprint (id order) and the receipt.
pub fn facade_exposure_rows(
    cuda: &RelevantSourceCuda,
    scene: &SurfaceGpu,
    airborne: &AirborneScene,
    cruise: &CruiseField,
    footprints: &[(u32, GridPolygons)],
) -> Result<(Vec<FacadeExposureRowWithBbox>, FacadeExposureReceipt)> {
    let owner = scene.host.owner;
    let mut receipt = FacadeExposureReceipt {
        buildings: footprints.len(),
        ..Default::default()
    };
    let started = Instant::now();
    // Morton order of the first vertex keeps each kernel block's receivers together.
    let mut order: Vec<usize> = (0..footprints.len()).collect();
    order.sort_by_key(|&i| morton_key(footprints[i].1[0][0][0]));
    let obstacles = &scene.host.obstacles;
    let receivers_by_footprint: Vec<Vec<FacadeReceiverPosition>> = order
        .par_iter()
        .map(|&i| exposed_facade_receivers(&footprints[i].1, obstacles))
        .collect();
    let points: Vec<_> = order
        .iter()
        .zip(&receivers_by_footprint)
        .flat_map(|(&i, receivers)| {
            let key = FootprintKey {
                square_y: owner.y,
                square_x: owner.x,
                id: footprints[i].0,
            };
            receivers.iter().map(move |receiver| {
                let (lat, lon) = receiver.latitude_longitude();
                (lat, lon, Some(key))
            })
        })
        .collect();
    let receivers = ReceiverPoints::prepare(&scene.host, &points)?;
    receipt.receivers = receivers.len();
    receipt.placement_seconds = started.elapsed().as_secs_f64();

    let started = Instant::now();
    let surface = evaluate_receivers_exactly(cuda, scene, &receivers)?;
    receipt.surface_seconds = started.elapsed().as_secs_f64();
    let started = Instant::now();
    let airborne_power = airborne.period_powers(&scene.host, &receivers)?;
    receipt.airborne_seconds = started.elapsed().as_secs_f64();
    let started = Instant::now();
    let cruise_power = cruise.period_powers(&scene.host, &receivers)?;
    receipt.cruise_seconds = started.elapsed().as_secs_f64();
    let layers: [&[f32]; SOURCE_LAYERS.len()] = [
        &surface[0],
        &surface[1],
        &surface[2],
        &surface[3],
        &surface[4],
        &airborne_power,
        &cruise_power,
        &surface[5],
    ];

    let mut rows = Vec::with_capacity(footprints.len());
    let mut first_receiver = 0;
    for (&footprint, positions) in order.iter().zip(&receivers_by_footprint) {
        let (id, polygons) = &footprints[footprint];
        let evaluated = first_receiver..first_receiver + positions.len();
        first_receiver = evaluated.end;
        let chosen = noisiest_receiver(&layers, evaluated.clone()).map(|(best, runner_up)| {
            let receiver = best - evaluated.start;
            ChosenFacadeReceiver {
                canonical_index: receiver as u32,
                gx: positions[receiver].gx,
                gy: positions[receiver].gy,
                ground_altitude_m: receivers.altitude[best]
                    - noise_compute::constants::DEFAULT_RECEIVER_HEIGHT as f32,
                outward_bearing_deg: positions[receiver].outward_bearing_deg,
                layer_period_power: layers
                    .iter()
                    .flat_map(|plane| plane[best * PERIOD_COUNT..(best + 1) * PERIOD_COUNT].to_vec())
                    .collect(),
                total_lden_db: all_source_lden(&layers, best) as f32,
                runner_up_lden_db: runner_up.map(|lden| lden as f32),
            }
        });
        receipt.buildings_without_exposed_facade += usize::from(chosen.is_none());
        rows.push((
            FacadeExposureRow {
                footprint_id: *id,
                facade_points: positions.len() as u32,
                chosen,
            },
            footprint_bbox(polygons),
        ));
    }
    rows.sort_by_key(|(row, _)| row.footprint_id);
    Ok((rows, receipt))
}

/// Surface-layer mean powers (`SURFACE_LAYERS` order, receiver × period) at
/// arbitrary receivers: the painter's kernel with, per block, every source
/// whose reach touches the block's receivers and no interpolated background.
pub fn evaluate_receivers_exactly(
    cuda: &RelevantSourceCuda,
    scene: &SurfaceGpu,
    receivers: &ReceiverPoints,
) -> Result<[Vec<f32>; SURFACE_LAYERS.len()]> {
    let count = receivers.len();
    let mut planes: [Vec<f32>; SURFACE_LAYERS.len()] =
        std::array::from_fn(|_| vec![0.0; count * PERIOD_COUNT]);
    let sources = &scene.host.sources;
    if count == 0 || sources.is_empty() {
        return Ok(planes);
    }
    let background = DeviceBuffer::from_slice(&vec![0.0_f32; BLOCK_COUNT * 4 * PERIOD_COUNT])?;
    for first in (0..count).step_by(RECEIVERS_PER_LAUNCH) {
        let launched = RECEIVERS_PER_LAUNCH.min(count - first);
        // Unused slots repeat the last receiver and get no sources.
        let receiver_of_slot = |slot: usize| first + slot.min(launched - 1);
        let mut by_pixel: [Vec<f32>; 5] = std::array::from_fn(|_| vec![0.0; RECEIVERS_PER_LAUNCH]);
        for slot in 0..RECEIVERS_PER_LAUNCH {
            let (pixel, receiver) = (pixel_of_slot(slot), receiver_of_slot(slot));
            for (column, values) in by_pixel.iter_mut().zip([
                &receivers.x,
                &receivers.y,
                &receivers.altitude,
                &receivers.reflection,
                &receivers.floor,
            ]) {
                column[pixel] = values[receiver];
            }
        }
        let bounds: Vec<Option<[f32; 4]>> = (0..BLOCK_COUNT)
            .map(|block| {
                let slots = block * RECEIVERS_PER_BLOCK..((block + 1) * RECEIVERS_PER_BLOCK).min(launched);
                slots.clone().next().map(|_| {
                    slots.fold(
                        [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY],
                        |[left, bottom, right, top], slot| {
                            let (x, y) = (receivers.x[first + slot], receivers.y[first + slot]);
                            [left.min(x), bottom.min(y), right.max(x), top.max(y)]
                        },
                    )
                })
            })
            .collect();
        let launch_bounds = bounds.iter().flatten().fold(
            [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY],
            |a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])],
        );
        let reaches = |source: &DeviceLineSource, [left, bottom, right, top]: [f32; 4]| {
            let reach = source.max_distance_m;
            source.start_x_m.max(source.end_x_m) + reach >= left
                && source.start_x_m.min(source.end_x_m) - reach <= right
                && source.start_y_m.max(source.end_y_m) + reach >= bottom
                && source.start_y_m.min(source.end_y_m) - reach <= top
        };
        let candidates: Vec<u32> = (0..sources.len() as u32)
            .filter(|&i| reaches(&sources[i as usize].device, launch_bounds))
            .collect();
        let block_sources: Vec<Vec<u32>> = bounds
            .par_iter()
            .map(|bound| match bound {
                None => Vec::new(),
                Some(bound) => candidates
                    .iter()
                    .copied()
                    .filter(|&i| reaches(&sources[i as usize].device, *bound))
                    .collect(),
            })
            .collect();
        let [x, y, altitude, reflection, floor] = by_pixel.each_ref().map(|column| column.as_slice());
        let (x, y) = (DeviceBuffer::from_slice(x)?, DeviceBuffer::from_slice(y)?);
        let altitude = DeviceBuffer::from_slice(altitude)?;
        let reflection = DeviceBuffer::from_slice(reflection)?;
        let floor = DeviceBuffer::from_slice(floor)?;
        for (layer, plane) in planes.iter_mut().enumerate() {
            let mut offsets = vec![0_u32];
            let mut indices = Vec::new();
            for list in &block_sources {
                indices.extend(list.iter().filter(|&&i| sources[i as usize].layer == layer as u8));
                offsets.push(indices.len().try_into()?);
            }
            let (energy, _) = cuda.paint_tile(
                &scene.pointers(),
                &floor,
                &DeviceBuffer::from_slice(&offsets)?,
                &DeviceBuffer::from_slice(&indices)?,
                &background,
                &x,
                &y,
                &altitude,
                &reflection,
            )?;
            ensure!(
                energy.iter().all(|value| value.is_finite() && *value >= 0.0),
                "invalid façade receiver energy"
            );
            for slot in 0..launched {
                let pixel = pixel_of_slot(slot);
                plane[(first + slot) * PERIOD_COUNT..(first + slot + 1) * PERIOD_COUNT]
                    .copy_from_slice(&energy[pixel * PERIOD_COUNT..(pixel + 1) * PERIOD_COUNT]);
            }
        }
    }
    Ok(planes)
}

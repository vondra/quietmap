//! Facade-aware z13 receivers and conservative exact-source admission for enclosed blocks.
use crate::{
    source_frame::*, surface_scene::SurfaceScene, tile_source_incidence::TileSourceIncidence,
};
use anyhow::{ensure, Result};
use noise_compute::{
    constants::{DEFAULT_RECEIVER_HEIGHT, ENCLOSURE_RADIUS_M},
    propagation::obstacle_index::enclosure_db,
    types::RasterSampler,
};
use rayon::prelude::*;

pub struct TileReceivers {
    pub x: Vec<f32>,
    pub y: Vec<f32>,
    pub altitude: Vec<f32>,
    pub reflection: Vec<f32>,
    pub floor: Vec<f32>,
    pub indoor_attenuation: Vec<f32>,
    exact_blocks: Vec<bool>,
    bounds: Vec<[f32; 4]>,
}

impl TileReceivers {
    pub fn prepare(scene: &SurfaceScene, tile_x: u32, tile_y: u32) -> Result<Self> {
        ensure!(
            tile_x / 16 == u32::from(scene.owner.x) && tile_y / 16 == u32::from(scene.owner.y),
            "receiver tile outside scene owner"
        );
        let values: Vec<_> = (0..TILE_PIXEL_SIDE * TILE_PIXEL_SIDE)
            .into_par_iter()
            .map(|pixel| {
                let row = pixel / TILE_PIXEL_SIDE;
                let col = pixel % TILE_PIXEL_SIDE;
                let world_pixels = 8192.0 * TILE_PIXEL_SIDE as f64;
                let lon = (f64::from(tile_x) * TILE_PIXEL_SIDE as f64 + col as f64 + 0.5)
                    / world_pixels
                    * 360.0
                    - 180.0;
                let lat = (std::f64::consts::PI
                    * (1.0
                        - 2.0 * (f64::from(tile_y) * TILE_PIXEL_SIDE as f64 + row as f64 + 0.5)
                            / world_pixels))
                    .sinh()
                    .atan()
                    .to_degrees();
                let (lat, lon, enclosed) = source_reader::structure_store::locate_facade_receiver(
                    &scene.obstacles,
                    lat,
                    lon,
                );
                let [x, y] = scene.frame.encode(lat, lon);
                let altitude = (scene.raster.elevation(lat, lon) + DEFAULT_RECEIVER_HEIGHT) as f32;
                let reflection =
                    enclosure_db(&scene.obstacles, lat, lon, ENCLOSURE_RADIUS_M) as f32;
                let floor =
                    noise_compute::compute::aircraft_v6::airport_traffic::popup_pixel_floor_m(lat)
                        as f32;
                let moved_to_facade = enclosed.is_some();
                let delta = enclosed
                    .and_then(|value| value.effective_class.delta_db())
                    .unwrap_or(0.0) as f32;
                (x, y, altitude, reflection, floor, delta, moved_to_facade)
            })
            .collect();
        let mut result = Self {
            x: Vec::new(),
            y: Vec::new(),
            altitude: Vec::new(),
            reflection: Vec::new(),
            floor: Vec::new(),
            indoor_attenuation: Vec::new(),
            exact_blocks: vec![false; BLOCK_COUNT],
            bounds: vec![
                [
                    f32::INFINITY,
                    f32::INFINITY,
                    f32::NEG_INFINITY,
                    f32::NEG_INFINITY
                ];
                BLOCK_COUNT
            ],
        };
        for (pixel, (x, y, altitude, reflection, floor, delta, moved_to_facade)) in
            values.into_iter().enumerate()
        {
            ensure!(
                [x, y, altitude, reflection, floor, delta]
                    .iter()
                    .all(|value| value.is_finite()),
                "invalid facade receiver raster"
            );
            result.x.push(x);
            result.y.push(y);
            result.altitude.push(altitude);
            result.reflection.push(reflection);
            result.floor.push(floor);
            result.indoor_attenuation.push(delta);
            let block = (pixel / TILE_PIXEL_SIDE / BLOCK_PIXEL_SIDE) * BLOCKS_PER_TILE_SIDE
                + (pixel % TILE_PIXEL_SIDE / BLOCK_PIXEL_SIDE);
            result.exact_blocks[block] |= moved_to_facade;
            let bounds = &mut result.bounds[block];
            bounds[0] = bounds[0].min(x);
            bounds[1] = bounds[1].min(y);
            bounds[2] = bounds[2].max(x);
            bounds[3] = bounds[3].max(y);
        }
        Ok(result)
    }

    pub fn clear_enclosed_background(
        &self,
        partition: &mut crate::relevance_partition::RelevantSourcePartition,
    ) {
        for (block, background) in partition.background_corner_energy.iter_mut().enumerate() {
            if self.exact_blocks[block] {
                *background = [[0.0; PERIOD_COUNT]; 4];
            }
        }
    }

    pub fn admit_enclosed_sources(
        &self,
        sources: &[DeviceLineSource],
        incidence: &mut TileSourceIncidence,
    ) {
        incidence
            .local_source_indices_by_block
            .par_iter_mut()
            .enumerate()
            .for_each(|(block, indices)| {
                if !self.exact_blocks[block] {
                    return;
                }
                let [left, bottom, right, top] = self.bounds[block];
                indices.extend(sources.iter().enumerate().filter_map(|(index, source)| {
                    let reach = source.max_distance_m;
                    (source.start_x_m.max(source.end_x_m) + reach >= left
                        && source.start_x_m.min(source.end_x_m) - reach <= right
                        && source.start_y_m.max(source.end_y_m) + reach >= bottom
                        && source.start_y_m.min(source.end_y_m) - reach <= top)
                        .then_some(index as u32)
                }));
                indices.sort_unstable();
                indices.dedup();
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn facade_blocks_keep_every_reachable_source_and_no_interpolated_residual() {
        let receivers = TileReceivers {
            x: vec![],
            y: vec![],
            altitude: vec![],
            reflection: vec![],
            floor: vec![],
            indoor_attenuation: vec![],
            exact_blocks: (0..BLOCK_COUNT).map(|block| block == 0).collect(),
            bounds: vec![[0.0, 0.0, 100.0, 100.0]; BLOCK_COUNT],
        };
        let mut sources = vec![DeviceLineSource::default(); 40];
        for (index, source) in sources.iter_mut().enumerate() {
            source.start_x_m = 200.0 + index as f32;
            source.end_x_m = source.start_x_m;
            source.max_distance_m = 200.0;
        }
        sources[39].start_x_m = 1000.0;
        sources[39].end_x_m = 1000.0;
        let mut incidence = TileSourceIncidence {
            corner_offsets: vec![],
            corner_source_indices: vec![],
            local_source_indices_by_block: vec![vec![]; BLOCK_COUNT],
        };
        receivers.admit_enclosed_sources(&sources, &mut incidence);
        assert_eq!(
            incidence.local_source_indices_by_block[0],
            (0..39).collect::<Vec<_>>()
        );
        assert!(incidence.local_source_indices_by_block[1].is_empty());
        let mut partition = crate::relevance_partition::RelevantSourcePartition {
            block_offsets: vec![],
            relevant_source_indices: vec![],
            background_corner_energy: vec![[[5.0; PERIOD_COUNT]; 4]; BLOCK_COUNT],
        };
        receivers.clear_enclosed_background(&mut partition);
        assert_eq!(
            partition.background_corner_energy[0],
            [[0.0; PERIOD_COUNT]; 4]
        );
        assert_eq!(
            partition.background_corner_energy[1],
            [[5.0; PERIOD_COUNT]; 4]
        );
    }
}

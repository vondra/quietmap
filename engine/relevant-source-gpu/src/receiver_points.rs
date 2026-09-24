//! Receiver positions and the per-receiver inputs every surface, airborne and cruise launch reads.
use crate::{source_frame::PERIOD_COUNT, surface_scene::SurfaceScene};
use anyhow::{ensure, Result};
use noise_compute::{
    constants::{DEFAULT_RECEIVER_HEIGHT, ENCLOSURE_RADIUS_M},
    propagation::obstacle_index::{enclosure_db, FootprintKey},
    types::RasterSampler,
};
use rayon::prelude::*;

/// Receivers in a scene's metric frame, 4 m above the ground, with their density bonus.
#[derive(Clone, Debug, Default)]
pub struct ReceiverPoints {
    pub x: Vec<f32>,
    pub y: Vec<f32>,
    pub altitude: Vec<f32>,
    pub reflection: Vec<f32>,
    pub floor: Vec<f32>,
}

impl ReceiverPoints {
    /// Receivers at `(lat, lon)`; a façade receiver names its own building, whose
    /// probes the density bonus ignores (CNOSSOS §2.8).
    pub fn prepare(
        scene: &SurfaceScene,
        points: &[(f64, f64, Option<FootprintKey>)],
    ) -> Result<Self> {
        let values: Vec<[f32; 5]> = points
            .par_iter()
            .map(|&(lat, lon, own_footprint)| {
                let [x, y] = scene.frame.encode(lat, lon);
                [
                    x,
                    y,
                    (scene.raster.elevation(lat, lon) + DEFAULT_RECEIVER_HEIGHT) as f32,
                    enclosure_db(&scene.obstacles, lat, lon, ENCLOSURE_RADIUS_M, own_footprint)
                        as f32,
                    noise_compute::compute::aircraft_v6::airport_traffic::popup_pixel_floor_m(lat)
                        as f32,
                ]
            })
            .collect();
        ensure!(
            values.iter().flatten().all(|value| value.is_finite()),
            "invalid receiver raster"
        );
        let column = |i: usize| values.iter().map(|value| value[i]).collect();
        Ok(Self {
            x: column(0),
            y: column(1),
            altitude: column(2),
            reflection: column(3),
            floor: column(4),
        })
    }

    pub fn len(&self) -> usize {
        self.x.len()
    }

    pub fn is_empty(&self) -> bool {
        self.x.is_empty()
    }

    /// The receivers at `indices`, in that order.
    pub fn select(&self, indices: &[usize]) -> Self {
        let pick = |values: &[f32]| indices.iter().map(|&i| values[i]).collect();
        Self {
            x: pick(&self.x),
            y: pick(&self.y),
            altitude: pick(&self.altitude),
            reflection: pick(&self.reflection),
            floor: pick(&self.floor),
        }
    }
}

/// Scatter per-receiver period powers of `indices` back into a full plane.
pub fn scatter_period_powers(indices: &[usize], powers: &[f32], plane: &mut [f32]) {
    for (selected, &receiver) in indices.iter().enumerate() {
        plane[receiver * PERIOD_COUNT..(receiver + 1) * PERIOD_COUNT]
            .copy_from_slice(&powers[selected * PERIOD_COUNT..(selected + 1) * PERIOD_COUNT]);
    }
}

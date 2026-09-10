//! Device-resident canonical scenes and bounded durable-corner production.
use crate::{
    cuda_bridge::{DeviceBuffer, DeviceScenePointers, RelevantSourceCuda},
    obstacle_transfer::{
        DeviceObstacleEdgeEndpoints, DeviceObstacleGrid, DeviceRasterGeometry,
        FlattenedObstacleGeometry,
    },
    source_frame::{DeviceLineSource, PERIOD_COUNT},
    surface_scene::SurfaceScene,
};
use anyhow::{ensure, Result};
use grid::surface_corner::SurfaceCorner;
use noise_compute::{constants::ENCLOSURE_RADIUS_M, propagation::obstacle_index::enclosure_db};
use tile_painter::corner_store::{CornerEnergy, SourceEnergy};

pub struct SurfaceGpu {
    pub host: SurfaceScene,
    sources: DeviceBuffer<DeviceLineSource>,
    raster: DeviceBuffer<raster_reader::FusedPixel>,
    grids: DeviceBuffer<DeviceObstacleGrid>,
    starts: DeviceBuffer<u32>,
    references: DeviceBuffer<u32>,
    endpoints: DeviceBuffer<DeviceObstacleEdgeEndpoints>,
    heights: DeviceBuffer<f32>,
    buildings: DeviceBuffer<u8>,
    maximum_heights: DeviceBuffer<f32>,
}

impl SurfaceGpu {
    pub fn upload(host: SurfaceScene) -> Result<Self> {
        let flat = FlattenedObstacleGeometry::from_set(&host.frame, &host.obstacles);
        let sources = host
            .sources
            .iter()
            .map(|source| source.device)
            .collect::<Vec<_>>();
        ensure!(sources.len() <= u32::MAX as usize, "too many scene sources");
        let value = Self {
            sources: DeviceBuffer::from_slice(&sources)?,
            raster: DeviceBuffer::from_slice(host.raster.pixels())?,
            grids: DeviceBuffer::from_slice(&flat.grids)?,
            starts: DeviceBuffer::from_slice(&flat.cell_starts)?,
            references: DeviceBuffer::from_slice(&flat.edge_references)?,
            endpoints: DeviceBuffer::from_slice(&flat.edge_endpoints)?,
            heights: DeviceBuffer::from_slice(&flat.edge_height_m)?,
            buildings: DeviceBuffer::from_slice(&flat.edge_is_building)?,
            maximum_heights: DeviceBuffer::from_slice(&flat.cell_maximum_heights)?,
            host,
        };
        Ok(value)
    }

    pub(crate) fn pointers(&self) -> DeviceScenePointers {
        DeviceScenePointers {
            sources: self.sources.as_ptr(),
            raster_pixels: self.raster.as_ptr(),
            obstacle_grids: self.grids.as_ptr(),
            obstacle_cell_starts: self.starts.as_ptr(),
            obstacle_edge_references: self.references.as_ptr(),
            obstacle_edge_endpoints: self.endpoints.as_ptr(),
            obstacle_edge_height_m: self.heights.as_ptr(),
            obstacle_cell_maximum_heights: self.maximum_heights.as_ptr(),
            obstacle_edge_is_building: self.buildings.as_ptr(),
            source_count: self.host.sources.len() as u32,
            obstacle_grid_count: self.grids.element_count() as u32,
            pixel_floor_m: 0.0,
            raster_geometry: DeviceRasterGeometry::for_grid(&self.host.frame, &self.host.raster),
        }
    }

    /// At most 64 MiB of worst-case pair output plus source indexes per launch.
    pub fn evaluate_corners(
        &self,
        cuda: &RelevantSourceCuda,
        corners: &[SurfaceCorner],
    ) -> Result<Vec<CornerEnergy>> {
        ensure!(
            corners
                .iter()
                .all(|corner| corner.owner() == self.host.owner),
            "corner frame owner mismatch"
        );
        let pair_bytes = std::mem::size_of::<[f32; PERIOD_COUNT]>() + std::mem::size_of::<u32>();
        let pair_limit = 64 * 1024 * 1024 / pair_bytes;
        let mut result = Vec::with_capacity(corners.len());
        let mut next = 0;
        while next < corners.len() {
            let mut offsets = vec![0_u32];
            let mut indices = Vec::new();
            let mut xs = Vec::new();
            let mut ys = Vec::new();
            let mut reflections = Vec::new();
            let mut floors = Vec::new();
            while next < corners.len() && xs.len() < grid::surface_corner::CORNER_COUNT {
                let corner = corners[next];
                let [lat, lon] = corner.latitude_longitude();
                let [x, y] = self.host.frame.encode(lat, lon);
                let candidates: Vec<_> = self
                    .host
                    .sources
                    .iter()
                    .enumerate()
                    .filter_map(|(index, source)| {
                        (crate::tile_source_incidence::point_to_segment_distance_squared(
                            [x, y],
                            &source.device,
                        ) <= source.device.max_distance_m.powi(2))
                        .then_some(index as u32)
                    })
                    .collect();
                ensure!(
                    candidates.len() <= pair_limit,
                    "one vertex exceeds bounded source-pair capacity"
                );
                if indices.len() + candidates.len() > pair_limit {
                    break;
                }
                indices.extend(candidates);
                xs.push(x);
                ys.push(y);
                reflections
                    .push(enclosure_db(&self.host.obstacles, lat, lon, ENCLOSURE_RADIUS_M) as f32);
                floors.push(
                    noise_compute::compute::aircraft_v6::airport_traffic::popup_pixel_floor_m(lat)
                        as f32,
                );
                offsets.push(indices.len().try_into()?);
                next += 1;
            }
            let (energy, _) = cuda.evaluate_corners(
                &self.pointers(),
                &DeviceBuffer::from_slice(&floors)?,
                &DeviceBuffer::from_slice(&offsets)?,
                &DeviceBuffer::from_slice(&indices)?,
                &DeviceBuffer::from_slice(&xs)?,
                &DeviceBuffer::from_slice(&ys)?,
                &DeviceBuffer::from_slice(&reflections)?,
            )?;
            for range in offsets.windows(2) {
                let mut entries = Vec::new();
                for pair in range[0] as usize..range[1] as usize {
                    let source = &self.host.sources[indices[pair] as usize];
                    entries.push(SourceEnergy {
                        layer: source.layer,
                        source: source.identity,
                        periods: energy[pair],
                    });
                }
                result.push(CornerEnergy(entries));
            }
        }
        Ok(result)
    }
}

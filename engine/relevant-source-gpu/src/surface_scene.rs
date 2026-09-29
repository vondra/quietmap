//! One canonical z9 frame with complete source, structure and native raster support.
use crate::{
    input_manifest::InputManifest,
    native_sources::{load_sources, SurfaceSource},
    source_frame::{RegionMetricFrame, MAXIMUM_PROFILE_RAY_M},
};
use anyhow::{ensure, Context, Result};
use grid::{bounds::BoundedSquares, Square};
use noise_compute::propagation::obstacle_index::ObstacleSet;
use raster_reader::{tile_bbox::TileBbox, FusedGrid, RealRasters};
use std::path::Path;

pub struct SurfaceScene {
    pub owner: Square,
    pub frame: RegionMetricFrame,
    pub sources: Vec<SurfaceSource>,
    pub obstacles: ObstacleSet,
    pub raster: FusedGrid,
    /// Per-square meteorology windows for the per-receiver weather upload.
    pub weather: raster_reader::meteorology::WeatherCache,
}

pub fn scene_bounds(owner: Square) -> [f64; 4] {
    let bbox = TileBbox::from_xyz(9, u32::from(owner.x), u32::from(owner.y));
    let halo = f64::from(MAXIMUM_PROFILE_RAY_M);
    let latitude_margin = halo / grid::geo::M_PER_DEG_LAT;
    let south = (bbox.south_lat - latitude_margin).max(-90.0);
    let north = (bbox.north_lat + latitude_margin).min(90.0);
    let longitude_margin =
        halo / grid::geo::m_per_deg_lon(south.abs().max(north.abs()).to_radians());
    [
        south,
        bbox.west_lon - longitude_margin,
        north,
        bbox.east_lon + longitude_margin,
    ]
}

impl SurfaceScene {
    pub fn load(
        owner: Square,
        prepared: &Path,
        manifest: &InputManifest,
        rasters: &RealRasters,
    ) -> Result<Self> {
        ensure!(owner.x < 512 && owner.y < 512, "invalid scene owner");
        let [south, west, north, east] = scene_bounds(owner);
        let squares: Vec<_> = BoundedSquares::from_degrees(south, west, north, east)
            .context("invalid scene support")?
            .iter()
            .collect();
        let frame = RegionMetricFrame::for_square(owner);
        let (sources, obstacles) = load_sources(prepared, manifest, &squares, &frame)?;
        let raster = FusedGrid::build(rasters, south, north, west, east);
        ensure!(
            raster
                .pixels()
                .iter()
                .all(|pixel| pixel.elevation.is_finite()),
            "surface scene has unavailable DEM, canopy or ground data"
        );
        Ok(Self {
            owner,
            frame,
            sources,
            obstacles,
            raster,
            weather: raster_reader::meteorology::WeatherCache::new(rasters.dem.root()),
        })
    }

    /// One receiver's weather: its own square's window sampled at the receiver (the popup
    /// samples the same window through `RealRasters::weather`, so the two agree).
    pub fn receiver_weather(
        &self,
        latitude: f64,
        longitude: f64,
    ) -> noise_compute::propagation::meteorology::Meteorology {
        self.weather.weather(latitude, longitude)
    }
}

//! Canonical airborne row preparation and exact receiver horizon packing for CUDA.
use anyhow::{ensure, Result};
use noise_compute::{
    compute::aircraft_v6::AirborneSegmentBatch,
    emission::aircraft as air,
    propagation::obstacle_index::{CrossingScratch, ObstacleSet},
    types::{RasterSampler, Receiver},
};

pub const AIRBORNE_REDUCTION_ROWS: usize = 8192;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct DeviceAirborneSource {
    pub endpoints: [f32; 4],
    /// start_alt, d_lon, sdy, sdz, dv, di_a, di_b, di_c, reach_sq, cuts,
    /// then the Eq. 4-3 power weight [11] and the helicopter correction [12].
    pub physical: [f32; 13],
    /// Installation, class, departure, period, secondary-only provenance
    /// (the index into the two-entry provenance weight table), power row.
    pub identity: [i32; 6],
}
impl DeviceAirborneSource {
    pub fn prepare(batch: &AirborneSegmentBatch<'_>, i: usize) -> Result<Option<Self>> {
        ensure!(
            batch.period[i] < 3
                && batch.speed_kt[i].is_finite()
                && batch.length_m[i].is_finite()
                && batch.length_m[i] >= 0.0
                && batch.length_m[i] <= air::AIRBORNE_SUB_SEGMENT_MAX_LENGTH_M,
            "invalid prepared airborne period, speed or length"
        );
        let segment = batch.segment(i);
        let terrain = batch.terrain(i);
        if air::is_ground_stale_with_terrain(&segment, &terrain) {
            return Ok(None);
        }
        let p = air::prepare_segment(&segment, terrain.start_elev - 30.0, terrain.end_elev - 30.0);
        let result = Self {
            endpoints: [
                segment.start_lat as f32,
                segment.start_lon as f32,
                segment.end_lat as f32,
                segment.end_lon as f32,
            ],
            physical: [
                p.start_alt_m as f32,
                p.d_lon as f32,
                p.sdy as f32,
                p.sdz as f32,
                p.dv as f32,
                p.di_a as f32,
                p.di_b as f32,
                p.di_c as f32,
                p.reach_sq as f32,
                p.terrain_start_cut_m as f32,
                p.terrain_end_cut_m as f32,
                p.power_w as f32,
                p.heli_db as f32,
            ],
            identity: [
                match p.inst {
                    air::Installation::Wing => 0,
                    air::Installation::Fuselage => 1,
                    air::Installation::Propeller => 2,
                },
                p.class_idx as i32,
                i32::from(p.is_departure),
                i32::from(segment.period),
                i32::from(batch.flags[i] & air::SEGMENT_FLAG_SECONDARY_ONLY != 0),
                i32::from(p.power_row),
            ],
        };
        ensure!(
            result
                .physical
                .iter()
                .chain(&result.endpoints)
                .all(|v| v.is_finite()),
            "nonfinite prepared airborne source"
        );
        Ok(Some(result))
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct DeviceAirborneReceiver {
    pub latitude: f64,
    pub longitude: f64,
    pub metres_per_longitude_degree: f64,
    pub altitude: f32,
    pub padding: u32,
}

pub struct ReceiverScreening {
    pub receiver: Receiver,
    pub device: DeviceAirborneReceiver,
    pub terrain: air::ReceiverHorizon,
    pub buildings: air::BuildingHorizon,
}
impl ReceiverScreening {
    pub fn build(
        lat: f64,
        lon: f64,
        altitude: f32,
        rasters: &dyn RasterSampler,
        obstacles: &ObstacleSet,
    ) -> Result<Self> {
        Self::build_with_dem(lat, lon, altitude, obstacles, |lat, lon| rasters.elevation(lat, lon))
    }

    /// Keep a receiver-local DEM tile handle for the terrain march and every roof
    /// edge instead of locking the shared LRU per sample: in central Prague the
    /// locked roof-edge lookups made building horizons 4.8x slower on four threads.
    pub fn build_cached(
        lat: f64, lon: f64, altitude: f32, rasters: &raster_reader::RealRasters, obstacles: &ObstacleSet,
    ) -> Result<Self> {
        let mut key = (i32::MIN, i32::MIN);
        let mut tile = None;
        Self::build_with_dem(lat, lon, altitude, obstacles, |lat, lon| {
            rasters.dem.sample_cached(lat, lon, &mut key, &mut tile)
        })
    }

    fn build_with_dem(
        lat: f64, lon: f64, altitude: f32, obstacles: &ObstacleSet,
        mut dem: impl FnMut(f64, f64) -> f64,
    ) -> Result<Self> {
        ensure!(
            lat.is_finite() && lon.is_finite() && altitude.is_finite(),
            "nonfinite airborne receiver"
        );
        let receiver = Receiver::new(
            lat,
            lon,
            f64::from(altitude) - noise_compute::constants::DEFAULT_RECEIVER_HEIGHT,
        );
        let finite = std::cell::Cell::new(true);
        let terrain = air::ReceiverHorizon::build(
            |lat, lon| {
                let elevation = dem(lat, lon);
                finite.set(finite.get() && elevation.is_finite());
                elevation
            },
            lat,
            lon,
            receiver.altitude_m(),
        );
        ensure!(finite.get(), "airborne horizon DEM unavailable");
        // Match the popup if a caller supplies an enclosed point; the painter
        // evaluates only outdoor pixel centres and façade receivers.
        let empty = ObstacleSet { indexes: vec![] };
        let screening_obstacles = if obstacles.enclosed_footprint_at(lat, lon).is_some() {
            &empty
        } else {
            obstacles
        };
        let buildings = air::BuildingHorizon::build(
            screening_obstacles,
            &mut dem,
            lat,
            lon,
            receiver.altitude_m(),
            &mut CrossingScratch::default(),
        );
        Ok(Self {
            receiver,
            terrain,
            buildings,
            device: DeviceAirborneReceiver {
                latitude: lat,
                longitude: lon,
                metres_per_longitude_degree: air::M_PER_DEG_LAT * lat.to_radians().cos().max(0.2),
                altitude,
                padding: 0,
            },
        })
    }
}

pub struct PackedScreening {
    pub terrain: Vec<u32>,
    pub terrain_max: Vec<f32>,
    pub buildings: Vec<u32>,
    pub building_max: Vec<u16>,
    pub global_max: Vec<u16>,
}
impl PackedScreening {
    pub fn new(receivers: &[ReceiverScreening]) -> Self {
        let mut result = Self {
            terrain: Vec::new(),
            terrain_max: Vec::new(),
            buildings: Vec::new(),
            building_max: Vec::new(),
            global_max: Vec::new(),
        };
        for sector in 0..air::HORIZON_SECTORS {
            for band in 0..air::RECEIVER_HORIZON_BANDS {
                for rx in receivers {
                    let (tangent, range) = rx.terrain.packed_sectors()[sector][band];
                    result
                        .terrain
                        .push((u32::from(tangent as u16) << 16) | u32::from(range));
                }
            }
        }
        result
            .terrain_max
            .extend(receivers.iter().map(|rx| rx.terrain.max_sin_sq as f32));
        for sector in 0..air::BUILDING_LOCAL_HORIZON_SECTORS {
            for band in 0..air::BUILDING_LOCAL_HORIZON_BANDS {
                for rx in receivers {
                    let (tangent, range) = rx.buildings.packed_sectors()[sector][band];
                    result
                        .buildings
                        .push((u32::from(tangent) << 16) | u32::from(range));
                }
            }
        }
        for sector in 0..air::BUILDING_LOCAL_HORIZON_SECTORS {
            for rx in receivers {
                result
                    .building_max
                    .push(rx.buildings.packed_sector_maxima()[sector]);
            }
        }
        result.global_max.extend(receivers.iter().map(|rx| {
            rx.buildings
                .packed_sector_maxima()
                .iter()
                .copied()
                .filter(|value| *value != u16::MAX)
                .max_by(|a, b| {
                    f32::from_bits(u32::from(*a) << 16)
                        .total_cmp(&f32::from_bits(u32::from(*b) << 16))
                })
                .unwrap_or(u16::MAX)
        }));
        result
    }
}

#[cfg(test)]
#[path = "airborne_pack_tests.rs"]
mod tests;

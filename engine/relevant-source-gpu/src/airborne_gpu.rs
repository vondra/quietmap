//! Bounded receiver batches reuse the surface CUDA allocator and canonical CPU chord evaluator.
use super::AirborneScene;
use crate::{
    airborne_pack::{
        DeviceAirborneReceiver, DeviceAirborneSource, PackedScreening, ReceiverScreening,
        AIRBORNE_REDUCTION_ROWS,
    },
    cuda_bridge::{check_cuda, DeviceBuffer, RelevantSourceCuda},
    surface_scene::SurfaceScene,
    receiver_points::ReceiverPoints,
};
use anyhow::{ensure, Result};
use noise_compute::{compute::aircraft_v6::airborne, emission::aircraft as air};
use rayon::prelude::*;
use source_reader::aircraft_v6::AirborneRowAccum;

#[repr(C)]
struct DeviceAirborneScreen {
    terrain: *const u32,
    terrain_max: *const f32,
    buildings: *const u32,
    building_max: *const u16,
    global_max: *const u16,
    records: u32,
}
unsafe extern "C" {
    fn relevant_source_cuda_airborne(
        sources: *const DeviceAirborneSource,
        source_count: u32,
        receivers: *const DeviceAirborneReceiver,
        receiver_count: u32,
        npd: *const f32,
        weights: *const f32,
        screen: *const DeviceAirborneScreen,
        days: f32,
        partial: *mut f32,
        output: *mut f32,
    ) -> std::ffi::c_int;
}

impl AirborneScene<'_> {
    pub fn period_powers(
        &self,
        scene: &SurfaceScene,
        receivers: &ReceiverPoints,
    ) -> Result<Vec<f32>> {
        ensure!(
            scene.owner == self.owner,
            "airborne scene belongs to another owner"
        );
        let count = receivers.x.len();
        ensure!(
            receivers.y.len() == count && receivers.altitude.len() == count,
            "incomplete airborne receivers"
        );
        let points: Vec<_> = receivers
            .x
            .iter()
            .zip(&receivers.y)
            .map(|(&x, &y)| scene.frame.decode(x, y))
            .collect();
        ensure!(
            points.iter().flatten().all(|v| v.is_finite())
                && receivers.altitude.iter().all(|v| v.is_finite()),
            "nonfinite airborne receivers"
        );
        let mut output = vec![0.0f32; count * 3];
        if count == 0 || (self.independent.is_empty() && self.chords.is_empty()) {
            return Ok(output);
        }
        let _cuda = RelevantSourceCuda::initialize()?;
        // A union box only removes rows outside EVERY receiver's existing periodic envelope.
        let sources = self.selected_sources(&points)?;
        let device_sources = (!sources.is_empty())
            .then(|| DeviceBuffer::from_slice(&sources))
            .transpose()?;
        let npd = DeviceBuffer::from_slice(&air::NpdLuts::shared().sel_luts_flat_f32())?;
        let weights = DeviceBuffer::from_slice(&self.weights.as_array().map(|v| v as f32))?;
        let chords = AirborneRowAccum::new(&self.chords).map_err(anyhow::Error::msg)?;
        // 256 horizons bound staging memory independently of the requested tile's pixel count.
        for first in (0..count).step_by(256) {
            let end = (first + 256).min(count);
            let screening = (first..end)
                .into_par_iter()
                .map(|i| {
                    ReceiverScreening::build(
                        points[i][0],
                        points[i][1],
                        receivers.altitude[i],
                        self.rasters,
                        &scene.obstacles,
                    )
                })
                .collect::<Result<Vec<_>>>()?;
            if let Some(source) = &device_sources {
                let powers = gpu_powers(source, &screening, &npd, &weights, self.days)?;
                output[first * 3..end * 3].copy_from_slice(&powers);
            }
            let cpu = screening
                .par_iter()
                .map(|rx| {
                    let flights = airborne::scatter(
                        &rx.receiver,
                        chords.views(),
                        f64::from(self.days),
                        &self.weights,
                        &rx.terrain,
                        Some(&rx.buildings),
                        0,
                        None,
                    );
                    let mut powers = [0.0f64; 3];
                    // Stable flight order keeps repeated field evaluations deterministic.
                    let mut flights: Vec<_> = flights.into_iter().collect();
                    flights.sort_unstable_by_key(|(id, _)| *id);
                    for (_, flight) in flights {
                        for (period, power) in powers.iter_mut().enumerate() {
                            *power += flight.period_energy[period]
                                / (f64::from(self.days) * air::PERIOD_SECONDS[period]);
                        }
                    }
                    powers
                })
                .collect::<Vec<_>>();
            for (pixel, powers) in cpu.iter().enumerate() {
                for period in 0..3 {
                    output[(first + pixel) * 3 + period] += powers[period] as f32;
                }
            }
        }
        ensure!(
            output.iter().all(|v| v.is_finite() && *v >= 0.0),
            "invalid airborne mean power"
        );
        Ok(output)
    }

    fn selected_sources(&self, points: &[[f64; 2]]) -> Result<Vec<DeviceAirborneSource>> {
        let envelope = air::AirborneEnvelope::covering_receivers(points)
            .ok_or_else(|| anyhow::anyhow!("airborne receivers are empty"))?;
        let mut sources = Vec::new();
        for batch in &self.independent {
            let decoded =
                AirborneRowAccum::new(std::slice::from_ref(batch)).map_err(anyhow::Error::msg)?;
            let view = &decoded.views()[0];
            for i in 0..view.len() {
                let start = view.start_lat_lon(i);
                let end = view.end_lat_lon(i);
                if !envelope.intersects_segment(start, end) {
                    continue;
                }
                if let Some(source) = DeviceAirborneSource::prepare(view, i)? {
                    sources.push(source);
                }
            }
        }
        ensure!(
            sources.len() <= u32::MAX as usize,
            "airborne source count exceeds CUDA indexing"
        );
        Ok(sources)
    }
}

fn gpu_powers(
    sources: &DeviceBuffer<DeviceAirborneSource>,
    receivers: &[ReceiverScreening],
    npd: &DeviceBuffer<f32>,
    weights: &DeviceBuffer<f32>,
    days: u16,
) -> Result<Vec<f32>> {
    let packed = PackedScreening::new(receivers);
    let terrain = DeviceBuffer::from_slice(&packed.terrain)?;
    let terrain_max = DeviceBuffer::from_slice(&packed.terrain_max)?;
    let buildings = DeviceBuffer::from_slice(&packed.buildings)?;
    let building_max = DeviceBuffer::from_slice(&packed.building_max)?;
    let global_max = DeviceBuffer::from_slice(&packed.global_max)?;
    let points =
        DeviceBuffer::from_slice(&receivers.iter().map(|rx| rx.device).collect::<Vec<_>>())?;
    let screen = DeviceAirborneScreen {
        terrain: terrain.as_ptr(),
        terrain_max: terrain_max.as_ptr(),
        buildings: buildings.as_ptr(),
        building_max: building_max.as_ptr(),
        global_max: global_max.as_ptr(),
        records: receivers.len().try_into()?,
    };
    let parts = sources.element_count().div_ceil(AIRBORNE_REDUCTION_ROWS);
    let partial = DeviceBuffer::<f32>::uninitialized(
        receivers
            .len()
            .checked_mul(parts)
            .and_then(|n| n.checked_mul(3))
            .ok_or_else(|| anyhow::anyhow!("airborne reduction overflow"))?,
    )?;
    let output = DeviceBuffer::<f32>::uninitialized(receivers.len() * 3)?;
    check_cuda(unsafe {
        relevant_source_cuda_airborne(
            sources.as_ptr(),
            sources.element_count().try_into()?,
            points.as_ptr(),
            receivers.len().try_into()?,
            npd.as_ptr(),
            weights.as_ptr(),
            &screen,
            f32::from(days),
            partial.as_mut_ptr(),
            output.as_mut_ptr(),
        )
    })?;
    output.copy_to_vec()
}

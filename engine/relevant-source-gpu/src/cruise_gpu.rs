//! Cruise energy batches on the shared aircraft CUDA kernel and allocator.
use crate::cuda_bridge::{check_cuda, DeviceBuffer, RelevantSourceCuda};
use anyhow::{ensure, Result};
use noise_compute::emission::aircraft::{self as air, SegmentPrepared};

/// Receiver staging bound; 256 receivers need at most 1.5 MB of double partials.
const CRUISE_RECEIVER_BATCH: usize = 256;

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct DeviceCruiseSource {
    // Start latitude/longitude, centroid latitude/longitude, half length, density.
    geography: [f64; 6],
    physical: [f64; 12],
    identity: [i32; 4],
}
impl DeviceCruiseSource {
    fn from_bucket(bucket: &super::Bucket) -> Self {
        let prepared: &SegmentPrepared = &bucket.prepared;
        Self {
            geography: [
                prepared.start_lat,
                prepared.start_lon,
                bucket.lat,
                bucket.lon,
                bucket.half_length,
                bucket.density,
            ],
            physical: [
                prepared.start_alt_m,
                prepared.d_lon,
                prepared.sdy,
                prepared.sdz,
                prepared.dv,
                prepared.d_bar_m,
                prepared.di_a,
                prepared.di_b,
                prepared.di_c,
                prepared.reach_sq,
                prepared.terrain_start_cut_m,
                prepared.terrain_end_cut_m,
            ],
            identity: [
                match prepared.inst {
                    air::Installation::Wing => 0,
                    air::Installation::Fuselage => 1,
                    air::Installation::Propeller => 2,
                },
                prepared.class_idx as i32,
                i32::from(prepared.is_departure),
                bucket.period as i32,
            ],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct DeviceCruiseReceiver {
    latitude: f64,
    longitude: f64,
    model_metres_per_longitude_degree: f64,
    altitude: f64,
    gate_metres_per_longitude_degree: f64,
}

unsafe extern "C" {
    fn relevant_source_cuda_cruise(
        sources: *const DeviceCruiseSource,
        source_count: u32,
        receivers: *const DeviceCruiseReceiver,
        receiver_count: u32,
        npd: *const f64,
        partial: *mut f64,
        output: *mut f64,
    ) -> std::ffi::c_int;
}

struct CruiseSources {
    sources: DeviceBuffer<DeviceCruiseSource>,
    npd: DeviceBuffer<f64>,
}
impl CruiseSources {
    fn upload(sources: &[DeviceCruiseSource]) -> Result<Self> {
        ensure!(
            !sources.is_empty() && sources.len() <= u32::MAX as usize,
            "invalid cruise CUDA source count"
        );
        ensure!(
            sources.iter().all(|source| {
                source
                    .geography
                    .iter()
                    .chain(&source.physical)
                    .all(|value| value.is_finite())
                    && (0..3).contains(&source.identity[3])
            }),
            "invalid cruise CUDA source"
        );
        let _cuda = RelevantSourceCuda::initialize()?;
        Ok(Self {
            sources: DeviceBuffer::from_slice(sources)?,
            npd: DeviceBuffer::from_slice(&air::NpdLuts::shared().sel_luts_flat_f64())?,
        })
    }
    fn energies(&self, points: &[[f64; 2]], altitudes: &[f64]) -> Result<Vec<[f64; 3]>> {
        ensure!(
            points.len() == altitudes.len()
                && points
                    .iter()
                    .flatten()
                    .chain(altitudes.iter())
                    .all(|value| value.is_finite()),
            "invalid cruise CUDA receivers"
        );
        let mut result = vec![[0.0; 3]; points.len()];
        let parts = self
            .sources
            .element_count()
            .div_ceil(crate::airborne_pack::AIRBORNE_REDUCTION_ROWS);
        // Bounded receiver batches; no source × receiver materialization anywhere.
        for first in (0..points.len()).step_by(CRUISE_RECEIVER_BATCH) {
            let end = (first + CRUISE_RECEIVER_BATCH).min(points.len());
            let receivers: Vec<_> = (first..end)
                .map(|i| {
                    let [latitude, longitude] = points[i];
                    DeviceCruiseReceiver {
                        latitude,
                        longitude,
                        model_metres_per_longitude_degree: air::M_PER_DEG_LAT
                            * latitude.to_radians().cos().max(0.2),
                        altitude: altitudes[i],
                        gate_metres_per_longitude_degree: grid::geo::m_per_deg_lon(
                            latitude.to_radians(),
                        ),
                    }
                })
                .collect();
            let device_receivers = DeviceBuffer::from_slice(&receivers)?;
            let partial = DeviceBuffer::<f64>::uninitialized(
                receivers
                    .len()
                    .checked_mul(parts)
                    .and_then(|count| count.checked_mul(3))
                    .ok_or_else(|| anyhow::anyhow!("cruise reduction overflow"))?,
            )?;
            let output = DeviceBuffer::<f64>::uninitialized(receivers.len() * 3)?;
            check_cuda(unsafe {
                relevant_source_cuda_cruise(
                    self.sources.as_ptr(),
                    self.sources.element_count().try_into()?,
                    device_receivers.as_ptr(),
                    receivers.len().try_into()?,
                    self.npd.as_ptr(),
                    partial.as_mut_ptr(),
                    output.as_mut_ptr(),
                )
            })?;
            for (target, values) in result[first..end]
                .iter_mut()
                .zip(output.copy_to_vec()?.chunks_exact(3))
            {
                target.copy_from_slice(values);
            }
        }
        ensure!(
            result
                .iter()
                .flatten()
                .all(|value| value.is_finite() && *value >= 0.0),
            "invalid cruise CUDA energy"
        );
        Ok(result)
    }
}

/// One unnormalized Doc 29 evaluation of `buckets` at explicit receivers.
pub(super) fn evaluate(
    buckets: &[&super::Bucket],
    points: &[[f64; 2]],
    altitudes: &[f64],
) -> Result<Vec<[f64; 3]>> {
    let sources: Vec<_> = buckets
        .iter()
        .map(|bucket| DeviceCruiseSource::from_bucket(bucket))
        .collect();
    CruiseSources::upload(&sources)?.energies(points, altitudes)
}

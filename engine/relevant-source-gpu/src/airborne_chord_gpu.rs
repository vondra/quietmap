//! CUDA ownership for scene-constant chord geometry and bounded receiver batches.
use super::{DeviceAirborneScreen, UploadedScreen};
use crate::{
    airborne_chords::{ChordSource, ChordSources},
    airborne_pack::{DeviceAirborneReceiver, AIRBORNE_REDUCTION_ROWS},
    cuda_bridge::{check_cuda, DeviceBuffer},
};
use anyhow::Result;
use noise_compute::emission::aircraft as air;

unsafe extern "C" {
    fn relevant_source_cuda_airborne_chords(
        sources: *const ChordSource,
        rows: u32,
        ranges: *const u32,
        predecessors: *const u32,
        members: *const u32,
        receivers: *const DeviceAirborneReceiver,
        receiver_count: u32,
        npd: *const f64,
        weights: *const f64,
        screen: *const DeviceAirborneScreen,
        days: f64,
        levels: *mut f64,
        heads: *mut u32,
        accepted: *mut u32,
        partial: *mut f64,
        output: *mut f32,
    ) -> std::ffi::c_int;
}

pub(super) struct DeviceChords {
    sources: DeviceBuffer<ChordSource>,
    ranges: DeviceBuffer<u32>,
    predecessors: DeviceBuffer<u32>,
    members: DeviceBuffer<u32>,
    npd: DeviceBuffer<f64>,
    weights: DeviceBuffer<f64>,
}
impl DeviceChords {
    pub fn new(chords: &ChordSources, weights: &air::ProvenanceWeights) -> Result<Self> {
        Ok(Self {
            sources: DeviceBuffer::from_slice(&chords.sources)?,
            ranges: DeviceBuffer::from_slice(&chords.ranges)?,
            predecessors: DeviceBuffer::from_slice(&chords.predecessors)?,
            members: DeviceBuffer::from_slice(&chords.members)?,
            npd: DeviceBuffer::from_slice(&air::NpdLuts::shared().device_luts_flat_f64())?,
            weights: DeviceBuffer::from_slice(weights.as_array())?,
        })
    }

    pub fn powers(&self, receivers: &UploadedScreen, days: u16) -> Result<Vec<f32>> {
        let rows = self.sources.element_count();
        let count = receivers.points.element_count();
        let pairs = rows
            .checked_mul(count)
            .ok_or_else(|| anyhow::anyhow!("chord pair overflow"))?;
        let levels = DeviceBuffer::<f64>::uninitialized(
            pairs
                .checked_mul(2)
                .ok_or_else(|| anyhow::anyhow!("chord level overflow"))?,
        )?;
        let heads = DeviceBuffer::<u32>::uninitialized(pairs)?;
        let accepted = DeviceBuffer::<u32>::uninitialized(count)?;
        let parts = rows.div_ceil(AIRBORNE_REDUCTION_ROWS);
        let partial = DeviceBuffer::<f64>::uninitialized(count * parts * 3)?;
        let output = DeviceBuffer::<f32>::uninitialized(count * 3)?;
        check_cuda(unsafe {
            relevant_source_cuda_airborne_chords(
                self.sources.as_ptr(),
                rows.try_into()?,
                self.ranges.as_ptr(),
                self.predecessors.as_ptr(),
                self.members.as_ptr(),
                receivers.points.as_ptr(),
                count.try_into()?,
                self.npd.as_ptr(),
                self.weights.as_ptr(),
                &receivers.pointers(),
                f64::from(days),
                levels.as_mut_ptr(),
                heads.as_mut_ptr(),
                accepted.as_mut_ptr(),
                partial.as_mut_ptr(),
                output.as_mut_ptr(),
            )
        })?;
        output.copy_to_vec()
    }
}

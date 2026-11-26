//! Reusable device and pinned host scratch slots and output buffers for pair evaluations.

use crate::device::{PAIR_RAY_BYTES, PAIR_SUMS, driver};
use crate::stream::PinnedBuf;
use cudarc::driver::{CudaDevice, CudaSlice, DeviceRepr, DeviceSlice};
use std::sync::{Arc, Mutex, PoisonError};

/// Bounds on retained per-slot scratch so a rare dense batch does not keep hundreds of megabytes
/// pinned on the card or host for later squares.
const MIN_INITIAL_RAYS_PER_PAIR: usize = 2;
/// Cold evaluation slots start at 4 rays/pair (192 MB at 2M pairs, matching the previous painter)
/// so a sparse batch does not fail an upfront allocation before reaching the overflow retry.
const INITIAL_RAYS_PER_PAIR: usize = 4;
const MAX_INITIAL_RAYS_PER_PAIR: usize = 6;
/// Small batches (corners, sparse passes) keep the slot's existing density estimate so one tiny
/// single-ray launch does not trigger an overflow retry on the next 2M-pair road launch.
const RAYS_ESTIMATE_MIN_PAIRS: usize = 65_536;
/// Retained ray-buffer ceiling per eval slot (12M rays = 288 MB, 6 rays/pair at 2M pairs).
const MAX_RETAINED_RAY_BYTES: usize = 12_000_000 * PAIR_RAY_BYTES;
/// Retained pool limits: matches the concurrent turns plus unpacking headroom for outputs.
const MAX_RETAINED_EVAL_SLOTS: usize = crate::turns::EVALUATIONS_ON_THE_CARD;
const MAX_RETAINED_OUTPUT_SLOTS: usize = crate::turns::EVALUATIONS_ON_THE_CARD + 2;
const MAX_RETAINED_OUTPUT_PAIRS: usize = 2_000_000;

/// Reusable device and pinned host buffers for one concurrent `evaluate_pairs` launch.
pub(crate) struct EvalSlot {
    pub(crate) device_xy: Option<CudaSlice<f32>>,
    pub(crate) device_own: Option<CudaSlice<u64>>,
    pub(crate) device_point_of_receiver: Option<CudaSlice<u32>>,
    pub(crate) point_receivers: Option<CudaSlice<u8>>,
    pub(crate) receiver_failed: Option<CudaSlice<u32>>,
    pub(crate) device_receiver_of_pair: Option<CudaSlice<u32>>,
    pub(crate) device_candidates: Option<CudaSlice<u32>>,
    pub(crate) device_band_weights: Option<CudaSlice<f32>>,
    pub(crate) ray_counts: Option<CudaSlice<u32>>,
    pub(crate) ray_offsets: Option<CudaSlice<u32>>,
    pub(crate) block_sums: Option<CudaSlice<u32>>,
    pub(crate) total_rays: Option<CudaSlice<u32>>,
    pub(crate) failed: Option<CudaSlice<u32>>,
    pub(crate) pair_bands: Option<CudaSlice<f32>>,
    pub(crate) sums: Option<CudaSlice<f32>>,
    pub(crate) rays: Option<CudaSlice<u8>>,
    pub(crate) rays_per_pair: usize,
    pub(crate) host_xy: PinnedBuf<f32>,
    pub(crate) host_own: PinnedBuf<u64>,
    pub(crate) host_point_of_receiver: PinnedBuf<u32>,
    pub(crate) host_receiver_of_pair: PinnedBuf<u32>,
    pub(crate) host_candidates: PinnedBuf<u32>,
    pub(crate) host_band_weights: PinnedBuf<f32>,
    pub(crate) host_total: PinnedBuf<u32>,
}

/// Updates a slot's initial rays-per-pair estimate from the last completed launch without latching
/// to dense-batch outliers (relaxes by at most 1 ray/pair per full batch).
pub(crate) fn next_rays_per_pair(previous: usize, total_rays: usize, pair_count: usize) -> usize {
    if pair_count < RAYS_ESTIMATE_MIN_PAIRS {
        return previous.clamp(MIN_INITIAL_RAYS_PER_PAIR, MAX_INITIAL_RAYS_PER_PAIR);
    }
    total_rays
        .div_ceil(pair_count)
        .max(previous.saturating_sub(1))
        .clamp(MIN_INITIAL_RAYS_PER_PAIR, MAX_INITIAL_RAYS_PER_PAIR)
}

/// Ensures `buf` holds at least `need` elements, dropping any smaller allocation before allocating
/// its replacement so growth does not hold old and new device buffers simultaneously.
pub(crate) fn ensure_device<'a, T: DeviceRepr>(
    device: &Arc<CudaDevice>,
    buf: &'a mut Option<CudaSlice<T>>,
    need: usize,
) -> Result<&'a mut CudaSlice<T>, String> {
    let want = need.max(1);
    if buf.as_ref().is_none_or(|existing| existing.len() < want) {
        *buf = None;
        // SAFETY: callers always write the prefix read by any kernel on the forked stream created
        // after this allocation.
        *buf = Some(driver(unsafe { device.alloc::<T>(want) })?);
    }
    Ok(buf.as_mut().expect("device buffer allocated"))
}

/// Drops a retained device buffer when it grew past `max_retained` elements.
pub(crate) fn trim_device<T>(buf: &mut Option<CudaSlice<T>>, max_retained: usize) {
    if buf
        .as_ref()
        .is_some_and(|existing| existing.len() > max_retained)
    {
        *buf = None;
    }
}

impl EvalSlot {
    pub(crate) fn new(device: &Arc<CudaDevice>) -> Self {
        Self {
            device_xy: None,
            device_own: None,
            device_point_of_receiver: None,
            point_receivers: None,
            receiver_failed: None,
            device_receiver_of_pair: None,
            device_candidates: None,
            device_band_weights: None,
            ray_counts: None,
            ray_offsets: None,
            block_sums: None,
            total_rays: None,
            failed: None,
            pair_bands: None,
            sums: None,
            rays: None,
            rays_per_pair: INITIAL_RAYS_PER_PAIR,
            host_xy: PinnedBuf::new(device),
            host_own: PinnedBuf::new(device),
            host_point_of_receiver: PinnedBuf::new(device),
            host_receiver_of_pair: PinnedBuf::new(device),
            host_candidates: PinnedBuf::new(device),
            host_band_weights: PinnedBuf::new(device),
            host_total: PinnedBuf::new(device),
        }
    }

    fn trim_oversized(&mut self) {
        trim_device(&mut self.rays, MAX_RETAINED_RAY_BYTES);
    }
}

pub(crate) struct EvalSlotGuard<'a> {
    pub(crate) pool: &'a Mutex<Vec<EvalSlot>>,
    pub(crate) slot: Option<EvalSlot>,
}

impl Drop for EvalSlotGuard<'_> {
    fn drop(&mut self) {
        if let Some(mut slot) = self.slot.take() {
            slot.trim_oversized();
            let mut pool = self.pool.lock().unwrap_or_else(PoisonError::into_inner);
            if pool.len() < MAX_RETAINED_EVAL_SLOTS {
                pool.push(slot);
            }
        }
    }
}

pub(crate) type OutputPool = Arc<Mutex<Vec<(PinnedBuf<f32>, PinnedBuf<u32>)>>>;

/// Pinned host results of one `evaluate_pairs` launch; returns its buffers to the GPU pool on drop.
pub struct PairResults {
    pub(crate) sums: Option<PinnedBuf<f32>>,
    pub(crate) failed: Option<PinnedBuf<u32>>,
    pub(crate) pool: OutputPool,
    pub(crate) count: usize,
}

impl PairResults {
    pub fn sums(&self) -> &[[f32; PAIR_SUMS]] {
        let slice = self
            .sums
            .as_ref()
            .map_or(&[][..], |buf| buf.as_slice(self.count * PAIR_SUMS));
        slice.as_chunks::<PAIR_SUMS>().0
    }

    pub fn failed(&self) -> &[u32] {
        self.failed
            .as_ref()
            .map_or(&[][..], |buf| buf.as_slice(self.count))
    }
}

impl Drop for PairResults {
    fn drop(&mut self) {
        if let (Some(mut sums), Some(mut failed)) = (self.sums.take(), self.failed.take()) {
            sums.trim(PAIR_SUMS * MAX_RETAINED_OUTPUT_PAIRS);
            failed.trim(MAX_RETAINED_OUTPUT_PAIRS);
            let mut pool = self.pool.lock().unwrap_or_else(PoisonError::into_inner);
            if pool.len() < MAX_RETAINED_OUTPUT_SLOTS {
                pool.push((sums, failed));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rays_per_pair_estimate_bounds_and_relaxes_after_dense_batches() {
        assert_eq!(INITIAL_RAYS_PER_PAIR, 4);
        assert_eq!(
            next_rays_per_pair(INITIAL_RAYS_PER_PAIR, 12_000_000, 1_000_000),
            MAX_INITIAL_RAYS_PER_PAIR
        );
        // Tiny batches do not disturb the full-launch density estimate.
        assert_eq!(next_rays_per_pair(6, 1_000, 1_000), 6);
        // Full single-ray batches relax smoothly down to MIN_INITIAL_RAYS_PER_PAIR.
        let mut est = MAX_INITIAL_RAYS_PER_PAIR;
        for _ in 0..8 {
            est = next_rays_per_pair(est, 2_000_000, 2_000_000);
        }
        assert_eq!(est, MIN_INITIAL_RAYS_PER_PAIR);
    }
}

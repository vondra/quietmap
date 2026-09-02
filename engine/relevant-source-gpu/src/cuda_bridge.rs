//! Safe ownership and timed launches around the fixed relevant-source CUDA C ABI.

use std::ffi::{c_char, c_int, c_void, CStr};
use std::marker::PhantomData;
use std::mem::{size_of, size_of_val};
use std::ptr::NonNull;

use anyhow::{bail, Result};
use raster_reader::FusedPixel;

use crate::obstacle_transfer::{DeviceBarrier, DeviceObstacleGrid, DeviceRasterGeometry};
use crate::source_frame::{DeviceLineSource, CORNER_COUNT, PERIOD_COUNT, TILE_PIXEL_SIDE};

unsafe extern "C" {
    fn relevant_source_cuda_error_string(status: c_int) -> *const c_char;
    fn relevant_source_cuda_initialize(compute_capability: *mut c_int) -> c_int;
    fn relevant_source_cuda_allocate(pointer: *mut *mut c_void, bytes: usize) -> c_int;
    fn relevant_source_cuda_free(pointer: *mut c_void) -> c_int;
    fn relevant_source_cuda_copy_to_device(
        destination: *mut c_void,
        source: *const c_void,
        bytes: usize,
    ) -> c_int;
    fn relevant_source_cuda_copy_to_host(
        destination: *mut c_void,
        source: *const c_void,
        bytes: usize,
    ) -> c_int;
    fn relevant_source_cuda_evaluate_corners(
        scene: *const DeviceScenePointers,
        corner_offsets: *const u32,
        corner_source_indices: *const u32,
        corner_x_m: *const f32,
        corner_y_m: *const f32,
        corner_reflection_db: *const f32,
        pair_period_energy: *mut f32,
        elapsed_milliseconds: *mut f32,
    ) -> c_int;
    fn relevant_source_cuda_paint_tile(
        scene: *const DeviceScenePointers,
        block_offsets: *const u32,
        relevant_source_indices: *const u32,
        background_energy: *const f32,
        receiver_x_m: *const f32,
        receiver_y_m: *const f32,
        receiver_altitude_m: *const f32,
        receiver_reflection_db: *const f32,
        output_period_energy: *mut f32,
        elapsed_milliseconds: *mut f32,
    ) -> c_int;
}

/// Device addresses and scalar geometry copied by value into each CUDA kernel.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct DeviceScenePointers {
    pub sources: *const DeviceLineSource,
    pub raster_pixels: *const FusedPixel,
    pub obstacle_grids: *const DeviceObstacleGrid,
    pub obstacle_cell_starts: *const u32,
    pub obstacle_edge_references: *const u32,
    pub obstacle_edge_values_xyxyh: *const f32,
    pub obstacle_cell_maximum_heights: *const f32,
    pub barriers: *const DeviceBarrier,
    pub source_count: u32,
    pub obstacle_grid_count: u32,
    pub barrier_count: u32,
    /// Non-zero: long rays take the surface heatmap's coarse-middle cadence.
    pub coarse_middle_cadence: u32,
    /// Half a pixel of this tile in metres: the ground-ops divergence floor.
    pub pixel_floor_m: f32,
    pub raster_geometry: DeviceRasterGeometry,
}

/// One allocation with element count retained for bounds-checked copies.
pub struct DeviceBuffer<T> {
    pointer: NonNull<c_void>,
    length: usize,
    marker: PhantomData<T>,
}

// A device allocation belongs to the process's CUDA context, not to a host thread:
// the cell producer uploads on its thread and the painter frees on its own.
unsafe impl<T: Send> Send for DeviceBuffer<T> {}

impl<T: Copy> DeviceBuffer<T> {
    pub fn from_slice(values: &[T]) -> Result<Self> {
        let buffer = Self::uninitialized(values.len())?;
        check_cuda(unsafe {
            relevant_source_cuda_copy_to_device(
                buffer.pointer.as_ptr(),
                values.as_ptr().cast(),
                size_of_val(values),
            )
        })?;
        Ok(buffer)
    }

    pub fn uninitialized(length: usize) -> Result<Self> {
        let mut pointer = std::ptr::null_mut();
        check_cuda(unsafe {
            relevant_source_cuda_allocate(&mut pointer, length.saturating_mul(size_of::<T>()))
        })?;
        Ok(Self {
            pointer: NonNull::new(pointer).expect("CUDA returned success with a null allocation"),
            length,
            marker: PhantomData,
        })
    }

    pub fn copy_to_vec(&self) -> Result<Vec<T>> {
        let mut values = Vec::<T>::with_capacity(self.length);
        check_cuda(unsafe {
            relevant_source_cuda_copy_to_host(
                values.as_mut_ptr().cast(),
                self.pointer.as_ptr(),
                self.length * size_of::<T>(),
            )
        })?;
        unsafe { values.set_len(self.length) };
        Ok(values)
    }

    pub fn as_ptr(&self) -> *const T {
        self.pointer.as_ptr().cast()
    }

    pub fn as_mut_ptr(&self) -> *mut T {
        self.pointer.as_ptr().cast()
    }

    pub fn element_count(&self) -> usize {
        self.length
    }
}

impl<T> Drop for DeviceBuffer<T> {
    fn drop(&mut self) {
        let status = unsafe { relevant_source_cuda_free(self.pointer.as_ptr()) };
        debug_assert_eq!(status, 0, "CUDA allocation release failed with {status}");
    }
}

/// The comma-separated SASS images embedded in the linked CUDA archive.
const COMPILED_CUDA_ARCHS: &str = env!("RELEVANT_SOURCE_CUDA_ARCHS");

/// Process-wide device initialization and the two architecture-specific launches.
pub struct RelevantSourceCuda;

impl RelevantSourceCuda {
    pub fn initialize() -> Result<Self> {
        let mut compute_capability: c_int = 0;
        check_cuda(unsafe { relevant_source_cuda_initialize(&mut compute_capability) })?;
        let card_arch = format!("sm_{compute_capability}");
        if !COMPILED_CUDA_ARCHS.split(',').any(|arch| arch == card_arch) {
            bail!(
                "relevant-source-cuda: this card is {card_arch}, but the SASS fatbin embeds \
                 {COMPILED_CUDA_ARCHS}; add it to FLEET_CUDA_ARCHS for a release, or build \
                 with NOISE_GPU_ARCH={card_arch} for this card alone"
            );
        }
        Ok(Self)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_corners(
        &self,
        scene: &DeviceScenePointers,
        corner_offsets: &DeviceBuffer<u32>,
        corner_source_indices: &DeviceBuffer<u32>,
        corner_x_m: &DeviceBuffer<f32>,
        corner_y_m: &DeviceBuffer<f32>,
        corner_reflection_db: &DeviceBuffer<f32>,
    ) -> Result<(Vec<[f32; PERIOD_COUNT]>, f32)> {
        if corner_offsets.element_count() != CORNER_COUNT + 1
            || corner_x_m.element_count() != CORNER_COUNT
            || corner_y_m.element_count() != CORNER_COUNT
            || corner_reflection_db.element_count() != CORNER_COUNT
        {
            bail!("corner launch dimensions are inconsistent");
        }
        let pair_energy = DeviceBuffer::<f32>::uninitialized(
            corner_source_indices.element_count() * PERIOD_COUNT,
        )?;
        let mut elapsed_milliseconds = 0.0;
        check_cuda(unsafe {
            relevant_source_cuda_evaluate_corners(
                scene,
                corner_offsets.as_ptr(),
                corner_source_indices.as_ptr(),
                corner_x_m.as_ptr(),
                corner_y_m.as_ptr(),
                corner_reflection_db.as_ptr(),
                pair_energy.as_mut_ptr(),
                &mut elapsed_milliseconds,
            )
        })?;
        let flat = pair_energy.copy_to_vec()?;
        let energy = flat
            .chunks_exact(PERIOD_COUNT)
            .map(|periods| [periods[0], periods[1], periods[2]])
            .collect();
        Ok((energy, elapsed_milliseconds))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn paint_tile(
        &self,
        scene: &DeviceScenePointers,
        block_offsets: &DeviceBuffer<u32>,
        relevant_source_indices: &DeviceBuffer<u32>,
        background_energy: &DeviceBuffer<f32>,
        receiver_x_m: &DeviceBuffer<f32>,
        receiver_y_m: &DeviceBuffer<f32>,
        receiver_altitude_m: &DeviceBuffer<f32>,
        receiver_reflection_db: &DeviceBuffer<f32>,
    ) -> Result<(Vec<f32>, f32)> {
        let pixel_count = TILE_PIXEL_SIDE * TILE_PIXEL_SIDE;
        if receiver_x_m.element_count() != TILE_PIXEL_SIDE
            || receiver_y_m.element_count() != TILE_PIXEL_SIDE
            || receiver_altitude_m.element_count() != pixel_count
            || receiver_reflection_db.element_count() != pixel_count
        {
            bail!("paint launch dimensions are inconsistent");
        }
        let output = DeviceBuffer::<f32>::uninitialized(pixel_count * PERIOD_COUNT)?;
        let mut elapsed_milliseconds = 0.0;
        check_cuda(unsafe {
            relevant_source_cuda_paint_tile(
                scene,
                block_offsets.as_ptr(),
                relevant_source_indices.as_ptr(),
                background_energy.as_ptr(),
                receiver_x_m.as_ptr(),
                receiver_y_m.as_ptr(),
                receiver_altitude_m.as_ptr(),
                receiver_reflection_db.as_ptr(),
                output.as_mut_ptr(),
                &mut elapsed_milliseconds,
            )
        })?;
        Ok((output.copy_to_vec()?, elapsed_milliseconds))
    }
}

fn check_cuda(status: c_int) -> Result<()> {
    if status == 0 {
        return Ok(());
    }
    let message = unsafe {
        let pointer = relevant_source_cuda_error_string(status);
        if pointer.is_null() {
            "unknown CUDA error".into()
        } else {
            CStr::from_ptr(pointer).to_string_lossy().into_owned()
        }
    };
    bail!("CUDA error {status}: {message}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_pointer_layout_matches_cuda() {
        assert_eq!(size_of::<DeviceScenePointers>(), 112);
    }
}

//! Default-stream host transfers and forked side streams on the card.
//!
//! cudarc allocates and frees device buffers through stream-ordered pools on the default stream
//! where the card supports them, and with classic `cuMemAlloc`/`cuMemFree` on cards without pools.
//! A side stream may use a buffer only once it follows that allocation, and a launch's buffers are
//! declared before its stream so that the stream synchronizes or joins before they are freed.
//!
//! Host upload sources passed to [`upload_default`] and [`ForkedStream::copy_prefix`] stay alive and
//! unchanged, and download destinations in [`download_default`] and [`ForkedStream::read_back`]
//! stay alive and reserved for CUDA to write, until a host-side `cuStreamSynchronize` confirms
//! completion (on normal return, early return or unwind). Whenever a host-side synchronize fails
//! to confirm completion of a transfer or of pending side-stream work, the process aborts at once
//! (`std::process::abort`, with no diagnostic write that could block or unwind) so no host or
//! device buffer can be freed while CUDA's use of it is unknown. An abort preserves every bundle
//! already installed (the worker finds an installed bundle whose completion line was lost); an
//! independent writer thread can still install an earlier finished square between error detection
//! and process termination, while the failing square itself is never handed to a writer. Errors
//! before anything is queued, copy errors whose follow-up synchronize succeeds, a fork that fails
//! its initial wait before carrying work, and stream teardown failures after completion is already
//! confirmed all return ordinary `Err` without aborting.
//!
//! cudarc orders fork and join through the device's one event, recorded on one stream and waited on
//! by the other in two calls. Another thread recording it between them orders the stream after that
//! thread's work instead: kernels could start before their buffers are theirs, or buffers be freed
//! under them (a review of r066, 2026-10-15; the painters and the preparer share the card). The
//! card's lock makes each record and wait one step.

use crate::device::{Gpu, driver};
use cudarc::driver::{
    CudaDevice, CudaFunction, CudaSlice, CudaStream, DevicePtr, DevicePtrMut, DeviceRepr,
    DeviceSlice, DriverError, LaunchAsync, LaunchConfig, ValidAsZeroBits, result, sys,
};
use std::any::Any;
use std::cell::Cell;
use std::marker::PhantomData;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, PoisonError};

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else {
        "driver unwrap panicked".to_string()
    }
}

/// Unconditionally synchronizes `stream` after a queued host<->device copy; aborts at once if
/// synchronize fails, otherwise surfaces `copy`'s own result.
///
/// # Safety
/// The device context must be current on this thread and `stream` must belong to it.
unsafe fn finish_host_transfer(
    stream: sys::CUstream,
    copy: Result<(), DriverError>,
) -> Result<(), String> {
    if unsafe { result::stream::synchronize(stream) }.is_err() {
        std::process::abort();
    }
    driver(copy)
}

/// Page-locked host buffer allocated through CUDA (`cuMemHostAlloc`) for direct DMA transfers.
/// Requires `ValidAsZeroBits` because [`Self::reserve`] zero-initializes the buffer before
/// exposing typed slices.
pub(crate) struct PinnedBuf<T: DeviceRepr + ValidAsZeroBits + Copy> {
    ptr: *mut T,
    capacity: usize,
    device: Arc<CudaDevice>,
}

// SAFETY: `PinnedBuf<T>` owns a portable page-locked host allocation (`CU_MEMHOSTALLOC_PORTABLE`)
// and `T: Copy + DeviceRepr + ValidAsZeroBits + Send` has no thread affinity.
unsafe impl<T: DeviceRepr + ValidAsZeroBits + Copy + Send> Send for PinnedBuf<T> {}

impl<T: DeviceRepr + ValidAsZeroBits + Copy> PinnedBuf<T> {
    pub(crate) fn new(device: &Arc<CudaDevice>) -> Self {
        Self {
            ptr: std::ptr::null_mut(),
            capacity: 0,
            device: Arc::clone(device),
        }
    }

    pub(crate) fn reserve(&mut self, len: usize) -> Result<(), String> {
        if len <= self.capacity {
            return Ok(());
        }
        self.try_free()?;
        let bytes = len
            .checked_mul(std::mem::size_of::<T>())
            .ok_or("pinned host buffer size overflow")?;
        driver(self.device.bind_to_thread())?;
        let mut raw: *mut std::ffi::c_void = std::ptr::null_mut();
        // SAFETY: context bound above; `raw` receives a page-locked allocation of `bytes` bytes.
        let status =
            unsafe { sys::lib().cuMemHostAlloc(&mut raw, bytes, sys::CU_MEMHOSTALLOC_PORTABLE) };
        driver(status.result())?;
        // SAFETY: `raw` points to `bytes` writable bytes; `T: ValidAsZeroBits` guarantees all-zero
        // bit patterns are valid values of `T`, pre-faulting pages and initializing `&mut [T]`.
        unsafe {
            std::ptr::write_bytes(raw.cast::<u8>(), 0, bytes);
        }
        self.ptr = raw.cast::<T>();
        self.capacity = len;
        Ok(())
    }

    pub(crate) fn copy_from_slice(&mut self, src: &[T]) -> Result<&[T], String> {
        self.reserve(src.len())?;
        if !src.is_empty() {
            // SAFETY: `self.ptr` holds at least `src.len()` initialized elements and cannot alias `src`.
            unsafe {
                std::ptr::copy_nonoverlapping(src.as_ptr(), self.ptr, src.len());
            }
        }
        Ok(self.as_slice(src.len()))
    }

    pub(crate) fn as_slice(&self, len: usize) -> &[T] {
        assert!(len <= self.capacity);
        if len == 0 {
            &[]
        } else {
            // SAFETY: `len <= self.capacity` and `reserve` zero-initialized all elements.
            unsafe { std::slice::from_raw_parts(self.ptr, len) }
        }
    }

    pub(crate) fn as_mut_slice(&mut self, len: usize) -> &mut [T] {
        assert!(len <= self.capacity);
        if len == 0 {
            &mut []
        } else {
            // SAFETY: exclusive `&mut self`, `len <= self.capacity`, all elements initialized.
            unsafe { std::slice::from_raw_parts_mut(self.ptr, len) }
        }
    }

    /// Releases the page-locked allocation when it grew beyond `max_retained` elements.
    /// Keeps the pointer intact if freeing fails so a later attempt can still release it.
    pub(crate) fn trim(&mut self, max_retained: usize) {
        if self.capacity > max_retained {
            let _ = self.try_free();
        }
    }

    fn try_free(&mut self) -> Result<(), String> {
        clear_after_free(&mut self.ptr, &mut self.capacity, |ptr| {
            driver(self.device.bind_to_thread())?;
            // SAFETY: `ptr` came from `cuMemHostAlloc` and no stream references it after sync.
            let status = unsafe { sys::lib().cuMemFreeHost(ptr.cast::<std::ffi::c_void>()) };
            driver(status.result())
        })
    }
}

/// Clears `(ptr, capacity)` only once `release` confirms the allocation was freed.
fn clear_after_free<T>(
    ptr: &mut *mut T,
    capacity: &mut usize,
    release: impl FnOnce(*mut T) -> Result<(), String>,
) -> Result<(), String> {
    if !ptr.is_null() {
        release(*ptr)?;
        *ptr = std::ptr::null_mut();
        *capacity = 0;
    }
    Ok(())
}

impl<T: DeviceRepr + ValidAsZeroBits + Copy> Drop for PinnedBuf<T> {
    fn drop(&mut self) {
        let _ = self.try_free();
    }
}

/// Copies the first `dst.len()` elements of `source` into `dst` on `stream`, keeping `dst` alive
/// until a host-side synchronize confirms completion (aborting at once if synchronize fails).
///
/// # Safety
/// The device context must be current on this thread and `stream` must belong to it.
unsafe fn dtoh_into_on_stream<T: DeviceRepr>(
    source: &CudaSlice<T>,
    dst: &mut [T],
    stream: sys::CUstream,
) -> Result<(), String> {
    if dst.is_empty() {
        return Ok(());
    }
    if dst.len() > source.len() {
        return Err("read-back destination larger than device buffer".into());
    }
    // SAFETY: `dst` stays borrowed across the async copy and unconditional synchronize.
    let copy = unsafe { result::memcpy_dtoh_async(dst, *source.device_ptr(), stream) };
    unsafe { finish_host_transfer(stream, copy) }
}

/// Copies `source` to the host on `stream`, keeping the destination `Vec` alive and reserved for
/// CUDA to write until a host-side synchronize confirms completion (aborting at once if
/// synchronize fails).
///
/// # Safety
/// The device context must be current on this thread and `stream` must belong to it.
unsafe fn dtoh_on_stream<T: DeviceRepr + Default + Clone>(
    source: &CudaSlice<T>,
    stream: sys::CUstream,
) -> Result<Vec<T>, String> {
    if source.is_empty() {
        return Ok(Vec::new());
    }
    let mut values = vec![T::default(); source.len()];
    unsafe { dtoh_into_on_stream(source, &mut values, stream) }?;
    Ok(values)
}

/// Allocates on the card and copies `values` on the default stream, keeping `values` alive and
/// unchanged until a host-side synchronize confirms completion (aborting at once if synchronize
/// fails). Errors before anything is queued (allocation) and copy errors whose follow-up
/// synchronize succeeds return normally.
pub(crate) fn upload_default<T: DeviceRepr>(
    device: &Arc<CudaDevice>,
    values: &[T],
) -> Result<CudaSlice<T>, String> {
    // `device.alloc` binds the primary context to this thread before allocating.
    let mut dst = driver(unsafe { device.alloc(values.len()) })?;
    if values.is_empty() {
        return Ok(dst);
    }
    let stream = *device.cu_stream();
    // SAFETY: `dst` holds `values.len()` elements; `values` remains borrowed across both the copy
    // and the unconditional synchronize below, which aborts on failure before `values` can drop.
    let copy = unsafe { result::memcpy_htod_async(*dst.device_ptr_mut(), values, stream) };
    unsafe { finish_host_transfer(stream, copy) }?;
    Ok(dst)
}

/// Copies `source` to the host on the default stream, keeping the destination `Vec` alive and
/// reserved for CUDA to write until a host-side synchronize confirms completion (aborting at once
/// if synchronize fails). Errors before anything is queued (context bind) and copy errors whose
/// follow-up synchronize succeeds return normally.
pub(crate) fn download_default<T: DeviceRepr + Default + Clone>(
    device: &Arc<CudaDevice>,
    source: &CudaSlice<T>,
) -> Result<Vec<T>, String> {
    if source.is_empty() {
        return Ok(Vec::new());
    }
    driver(device.bind_to_thread())?;
    // SAFETY: context bound above; `device.cu_stream()` is its default stream.
    unsafe { dtoh_on_stream(source, *device.cu_stream()) }
}

pub(crate) struct ForkedStream<'a, 'host> {
    /// `Some` while open; [`Self::close`] takes it first and keeps it alive while it confirms any
    /// pending work, then drops it.
    stream: Option<CudaStream>,
    device: &'a Arc<CudaDevice>,
    event: &'a Mutex<()>,
    /// True from any side-stream work (`copy`, `zero`, `launch`) until a host-side synchronize
    /// confirms it finished.
    pending_work: Cell<bool>,
    /// Ties every borrowed host source passed to [`Self::copy_prefix`] (invariant in `'host`) so Rust
    /// requires each one to outlive this stream and remain unchanged until a host-side synchronize
    /// confirms completion (in [`Self::read_back`] on the normal path, or in [`Drop`] while work
    /// is still pending).
    _host: PhantomData<Cell<&'host ()>>,
}

impl Gpu {
    /// A stream whose work follows the default stream's so far (the buffers just allocated there,
    /// and what a launch still copies or zeroes there). A failure during creation or its initial
    /// wait (including if cudarc's stream drop panics cleaning up the unused stream) returns `Err`
    /// without aborting, because the stream carried no work.
    pub(crate) fn fork<'a, 'host>(&'a self) -> Result<ForkedStream<'a, 'host>, String> {
        let _held = self.event.lock().unwrap_or_else(PoisonError::into_inner);
        let stream = match catch_unwind(AssertUnwindSafe(|| self.device.fork_default_stream())) {
            Ok(result) => driver(result)?,
            Err(payload) => {
                return Err(format!(
                    "cuda: stream fork cleanup failed ({})",
                    panic_message(payload)
                ));
            }
        };
        Ok(ForkedStream {
            stream: Some(stream),
            device: &self.device,
            event: &self.event,
            pending_work: Cell::new(false),
            _host: PhantomData,
        })
    }
}

impl<'host> ForkedStream<'_, 'host> {
    fn open_stream(&self) -> &CudaStream {
        self.stream.as_ref().expect("forked stream open")
    }

    /// Ensures the context is current on the first queued operation after creation or a completed
    /// synchronize, then marks side-stream work pending before submitting to CUDA.
    fn begin_work(&self) -> Result<(), String> {
        if !self.pending_work.get() {
            driver(self.device.bind_to_thread())?;
            self.pending_work.set(true);
        }
        Ok(())
    }

    /// Copies `values` into the start of `buffer` (`values.len() <= buffer.len()`) on this stream,
    /// in its order: a kernel launched on it after reads them.
    ///
    /// # Safety
    /// 1. The caller must not leak this stream (for example with `std::mem::forget`): `values` is
    ///    tied to `'host` so Rust keeps it alive and unchanged until a host-side synchronize
    ///    confirms completion (in [`Self::read_back`] or [`Drop`], aborting if synchronize fails).
    /// 2. `buffer` is only borrowed for this call, not tied to `'host`: the caller must declare
    ///    `buffer` before this stream so this stream synchronizes or joins before `buffer` drops.
    pub(crate) unsafe fn copy_prefix<T: DeviceRepr>(
        &self,
        values: &'host [T],
        buffer: &mut CudaSlice<T>,
    ) -> Result<(), String> {
        if values.len() > buffer.len() {
            return Err("values larger than destination device buffer".into());
        }
        if values.is_empty() {
            return Ok(());
        }
        self.begin_work()?;
        // SAFETY: `buffer` holds at least `values.len()` elements; `values` outlives this stream
        // and `buffer` is declared before it.
        driver(unsafe {
            result::memcpy_htod_async(*buffer.device_ptr_mut(), values, self.open_stream().stream)
        })
    }

    /// Zeroes the first `count` elements of `buffer` on this stream, in its order: a kernel
    /// launched on it after reads zeros. `buffer` must be declared before this stream so this
    /// stream synchronizes or joins before `buffer` drops.
    pub(crate) fn zero_prefix<T: DeviceRepr + ValidAsZeroBits>(
        &self,
        buffer: &mut CudaSlice<T>,
        count: usize,
    ) -> Result<(), String> {
        if count > buffer.len() {
            return Err("zero_prefix count exceeds buffer length".into());
        }
        let bytes = count * std::mem::size_of::<T>();
        if bytes == 0 {
            return Ok(());
        }
        self.begin_work()?;
        // SAFETY: the buffer's own bytes, which are valid as zeros.
        driver(unsafe {
            result::memset_d8_async(
                *buffer.device_ptr_mut(),
                0,
                bytes,
                self.open_stream().stream,
            )
        })
    }

    /// Launches `function` on this stream and marks side-stream work pending so teardown confirms
    /// completion if the caller returns or unwinds before [`Self::read_back`].
    ///
    /// # Safety
    /// Same preconditions as [`LaunchAsync::launch_on_stream`], and every device buffer passed in
    /// `params` must outlive this stream (be declared before it).
    pub(crate) unsafe fn launch<P>(
        &self,
        function: CudaFunction,
        config: LaunchConfig,
        params: P,
    ) -> Result<(), String>
    where
        CudaFunction: LaunchAsync<P>,
    {
        self.begin_work()?;
        // SAFETY: caller upholds kernel argument validity and buffer drop order.
        driver(unsafe { function.launch_on_stream(self.open_stream(), config, params) })
    }

    /// `source` read back once the stream's work so far is done. When work is pending on this
    /// stream, waiting on it with a synchronize before the copy has been observed to use more CPU
    /// time and finish squares faster than letting the copy wait on its own, though the driver
    /// does not guarantee spin-waiting (2026-11-19). Once the pre-wait succeeds, immediate
    /// follow-up read-backs with no intervening work skip the redundant pre-wait. If a synchronize
    /// fails to confirm completion, the process aborts at once before `values` can drop.
    pub(crate) fn read_back<T: DeviceRepr + Default + Clone>(
        &self,
        source: &CudaSlice<T>,
    ) -> Result<Vec<T>, String> {
        self.read_back_prefix(source, source.len())
    }

    /// Reads the first `count` elements of `source` into a newly allocated `Vec`.
    pub(crate) fn read_back_prefix<T: DeviceRepr + Default + Clone>(
        &self,
        source: &CudaSlice<T>,
        count: usize,
    ) -> Result<Vec<T>, String> {
        let mut values = vec![T::default(); count];
        self.read_back_into(source, &mut values)?;
        Ok(values)
    }

    /// Reads the first `dst.len()` elements of `source` into `dst` once the stream's work is done.
    pub(crate) fn read_back_into<T: DeviceRepr>(
        &self,
        source: &CudaSlice<T>,
        dst: &mut [T],
    ) -> Result<(), String> {
        let stream = self.open_stream().stream;
        if self.pending_work.get() {
            // SAFETY: a stream this card created and still holds on this thread's bound context.
            if unsafe { result::stream::synchronize(stream) }.is_err() {
                std::process::abort();
            }
            self.pending_work.set(false);
        }
        // SAFETY: this thread bound the context in `Gpu::fork`; `stream` belongs to it.
        unsafe { dtoh_into_on_stream(source, dst, stream) }
    }

    /// Joins and destroys the stream, returning any teardown failure as an ordinary error once all
    /// work on the stream has been confirmed finished.
    pub(crate) fn finish(mut self) -> Result<(), String> {
        self.close()
    }

    fn close(&mut self) -> Result<(), String> {
        let Some(stream) = self.stream.take() else {
            return Ok(());
        };
        if self.pending_work.get() {
            // SAFETY: a stream this card created and still holds; every host slice passed to
            // `copy` and every device buffer used on this stream outlives this stream.
            if self.device.bind_to_thread().is_err()
                || unsafe { result::stream::synchronize(stream.stream) }.is_err()
            {
                std::process::abort();
            }
            self.pending_work.set(false);
        }
        let _held = self.event.lock().unwrap_or_else(PoisonError::into_inner);
        // All work on this stream is confirmed complete above (or in `read_back`), so a failure in
        // cudarc's `CudaStream::drop` (`wait_for` or `destroy` unwrapping) leaves no unconfirmed
        // transfer and surfaces as an ordinary error on `finish()` instead of aborting every
        // painter.
        catch_unwind(AssertUnwindSafe(move || drop(stream)))
            .map_err(|payload| format!("cuda: stream teardown failed ({})", panic_message(payload)))
    }
}

impl Drop for ForkedStream<'_, '_> {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panic_message_handles_known_and_unknown_payloads() {
        assert_eq!(panic_message(Box::new(String::from("owned"))), "owned");
        assert_eq!(panic_message(Box::new("static")), "static");
        assert_eq!(panic_message(Box::new(42u32)), "driver unwrap panicked");
    }

    #[test]
    fn pinned_free_retains_pointer_on_error_and_clears_on_success() {
        let mut dummy = 0u32;
        let mut ptr: *mut u32 = &mut dummy;
        let mut capacity = 16usize;
        assert!(clear_after_free(&mut ptr, &mut capacity, |_| Err("bind failed".into())).is_err());
        assert_eq!(ptr, &mut dummy as *mut u32);
        assert_eq!(capacity, 16);
        assert!(clear_after_free(&mut ptr, &mut capacity, |_| Ok(())).is_ok());
        assert!(ptr.is_null());
        assert_eq!(capacity, 0);
    }
}

//! Default-stream host transfers and forked side streams on the card.
//!
//! cudarc allocates and frees device buffers through stream-ordered pools on the default stream
//! where the card supports them, and with classic `cuMemAlloc`/`cuMemFree` on cards without pools.
//! A side stream may use a buffer only once it follows that allocation, and a launch's buffers are
//! declared before its stream so that the stream synchronizes or joins before they are freed.
//!
//! Host buffers passed to [`upload_default`], [`download_default`], [`ForkedStream::copy`] and
//! [`ForkedStream::read_back`] stay alive and unchanged until a host-side `cuStreamSynchronize`
//! confirms completion (on normal return, early return or unwind). Whenever a host-side
//! synchronize fails to confirm completion of a transfer or of pending side-stream work, the
//! process aborts without unwinding so no host or device buffer can be freed while CUDA's use of
//! it is unknown. An abort preserves every bundle already installed (the worker finds an installed
//! bundle whose completion line was lost); an independent writer thread can still install an
//! earlier finished square between error detection and process termination, while the failing
//! square itself is never handed to a writer. Errors before anything is queued, copy errors whose
//! follow-up synchronize succeeds, a fork that fails its initial wait before carrying work, and
//! stream teardown failures after completion is already confirmed all return ordinary `Err`
//! without aborting.
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
use std::fmt::Debug;
use std::io::Write;
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, PoisonError};

/// Aborts the process if dropped (for example if a diagnostic write ever panicked), so nothing on
/// the fatal path can unwind into a caller's drops or `catch_unwind`.
struct AbortOnDrop;

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        std::process::abort();
    }
}

fn write_abort_diagnostic(out: &mut impl Write, step: &str, code: impl Debug) {
    let _ = writeln!(
        out,
        "qm-paint: CUDA {step} failed ({code:?}); aborting to avoid unconfirmed transfers"
    );
}

/// Terminates the process without unwinding when CUDA completion cannot be confirmed.
/// Formats only the symbolic `CUresult` variant (no `cuGetErrorString` callback into a failed
/// driver, no heap allocation) and ignores stderr write errors.
fn abort_unconfirmed(step: &str, error: DriverError) -> ! {
    let _abort = AbortOnDrop;
    write_abort_diagnostic(&mut std::io::stderr(), step, error.0);
    std::process::abort();
}

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else {
        "driver unwrap panicked".to_string()
    }
}

/// Unconditionally synchronizes `stream` after a queued host<->device copy; aborts without
/// unwinding if synchronize fails, otherwise surfaces `copy`'s own result.
///
/// # Safety
/// The device context must be current on this thread and `stream` must belong to it.
unsafe fn finish_host_transfer(
    stream: sys::CUstream,
    step: &'static str,
    copy: Result<(), DriverError>,
) -> Result<(), String> {
    if let Err(error) = unsafe { result::stream::synchronize(stream) } {
        abort_unconfirmed(step, error);
    }
    driver(copy)
}

/// Copies `source` to the host on `stream`, keeping the destination `Vec` alive until a host-side
/// synchronize confirms completion (aborting without unwinding if synchronize fails).
///
/// # Safety
/// The device context must be current on this thread and `stream` must belong to it.
unsafe fn dtoh_on_stream<T: DeviceRepr + Default + Clone>(
    source: &CudaSlice<T>,
    stream: sys::CUstream,
    step: &'static str,
) -> Result<Vec<T>, String> {
    if source.is_empty() {
        return Ok(Vec::new());
    }
    let mut values = vec![T::default(); source.len()];
    // SAFETY: `values` holds `source.len()` initialized elements and stays alive across both the
    // copy and the unconditional synchronize in `finish_host_transfer` (which aborts on failure
    // before `values` can drop).
    let copy = unsafe { result::memcpy_dtoh_async(&mut values, *source.device_ptr(), stream) };
    unsafe { finish_host_transfer(stream, step, copy) }?;
    Ok(values)
}

/// Allocates on the card and copies `values` on the default stream, keeping `values` alive until
/// a host-side synchronize confirms completion (aborting without unwinding if synchronize fails).
/// Errors before anything is queued (allocation or context bind) and copy errors whose follow-up
/// synchronize succeeds return normally.
pub(crate) fn upload_default<T: DeviceRepr>(
    device: &Arc<CudaDevice>,
    values: &[T],
) -> Result<CudaSlice<T>, String> {
    let mut dst = driver(unsafe { device.alloc(values.len()) })?;
    if values.is_empty() {
        return Ok(dst);
    }
    driver(device.bind_to_thread())?;
    let stream = *device.cu_stream();
    // SAFETY: `dst` holds `values.len()` elements; `values` remains borrowed across both the copy
    // and the unconditional synchronize below, which aborts on failure before `values` can drop.
    let copy = unsafe { result::memcpy_htod_async(*dst.device_ptr_mut(), values, stream) };
    unsafe { finish_host_transfer(stream, "default-stream HtoD synchronize", copy) }?;
    Ok(dst)
}

/// Copies `source` to the host on the default stream, keeping the destination `Vec` alive until a
/// host-side synchronize confirms completion (aborting without unwinding if synchronize fails).
/// Errors before anything is queued (context bind) and copy errors whose follow-up synchronize
/// succeeds return normally.
pub(crate) fn download_default<T: DeviceRepr + Default + Clone>(
    device: &Arc<CudaDevice>,
    source: &CudaSlice<T>,
) -> Result<Vec<T>, String> {
    if source.is_empty() {
        return Ok(Vec::new());
    }
    driver(device.bind_to_thread())?;
    // SAFETY: context bound above; `device.cu_stream()` is its default stream.
    unsafe {
        dtoh_on_stream(
            source,
            *device.cu_stream(),
            "default-stream DtoH synchronize",
        )
    }
}

pub(crate) struct ForkedStream<'a, 'host> {
    stream: ManuallyDrop<CudaStream>,
    device: &'a Arc<CudaDevice>,
    event: &'a Mutex<()>,
    /// True from any side-stream work (`copy`, `zero`, `launch`) until a host-side synchronize
    /// confirms it finished.
    pending_work: Cell<bool>,
    /// Set once [`Self::close`] has already drained, joined and dropped `stream`.
    closed: bool,
    /// Ties every borrowed host source passed to [`Self::copy`] (invariant in `'host`) so Rust
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
            stream: ManuallyDrop::new(stream),
            device: &self.device,
            event: &self.event,
            pending_work: Cell::new(false),
            closed: false,
            _host: PhantomData,
        })
    }
}

impl<'host> ForkedStream<'_, 'host> {
    /// The stream's next work follows the default stream's so far as well (buffers allocated
    /// since).
    pub(crate) fn wait_for_default(&self) -> Result<(), String> {
        let _held = self.event.lock().unwrap_or_else(PoisonError::into_inner);
        driver(self.stream.wait_for_default())
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

    /// `values` copied into `buffer` on this stream, in its order: a kernel launched on it after
    /// reads them. Copied on the default stream, each copy waited for there, a launch's six
    /// inputs waited six times for whatever the passes of the squares had on that one stream.
    ///
    /// # Safety
    /// 1. The caller must not leak this stream (for example with `std::mem::forget`): `values` is
    ///    tied to `'host` so Rust keeps it alive and unchanged until a host-side synchronize
    ///    confirms completion (in [`Self::read_back`] or [`Drop`], aborting if synchronize fails).
    /// 2. `buffer` is only borrowed for this call, not tied to `'host`: the caller must declare
    ///    `buffer` before this stream so this stream synchronizes or joins before `buffer` drops.
    pub(crate) unsafe fn copy<T: DeviceRepr>(
        &self,
        values: &'host [T],
        buffer: &mut CudaSlice<T>,
    ) -> Result<(), String> {
        if values.len() != buffer.len() {
            return Err("a buffer of another length than the values copied into it".into());
        }
        if values.is_empty() {
            return Ok(());
        }
        self.begin_work()?;
        // SAFETY: the buffer holds as many values as are copied; `values` outlives this stream and
        // `buffer` is kept alive past this stream by the caller's declaration order.
        driver(unsafe {
            result::memcpy_htod_async(*buffer.device_ptr_mut(), values, self.stream.stream)
        })
    }

    /// `buffer` zeroed on this stream, in its order: a kernel launched on it after reads zeros.
    ///
    /// `buffer` is only borrowed for this call while the memset remains queued on this stream: the
    /// caller must declare `buffer` before this stream so this stream synchronizes or joins before
    /// `buffer` drops.
    pub(crate) fn zero<T: DeviceRepr + ValidAsZeroBits>(
        &self,
        buffer: &mut CudaSlice<T>,
    ) -> Result<(), String> {
        let bytes = buffer.num_bytes();
        if bytes == 0 {
            return Ok(());
        }
        self.begin_work()?;
        // SAFETY: the buffer's own bytes, which are valid as zeros.
        driver(unsafe {
            result::memset_d8_async(*buffer.device_ptr_mut(), 0, bytes, self.stream.stream)
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
        driver(unsafe { function.launch_on_stream(&self.stream, config, params) })
    }

    /// `source` read back once the stream's work so far is done. When work is pending on this
    /// stream, waiting on it with a synchronize before the copy has been observed to use more CPU
    /// time and finish squares faster than letting the copy wait on its own, though the driver
    /// does not guarantee spin-waiting (2026-11-19). Once the pre-wait succeeds, immediate
    /// follow-up read-backs with no intervening work skip the redundant pre-wait. If a synchronize
    /// fails to confirm completion, the process aborts without unwinding before `values` can drop.
    pub(crate) fn read_back<T: DeviceRepr + Default + Clone>(
        &self,
        source: &CudaSlice<T>,
    ) -> Result<Vec<T>, String> {
        if self.pending_work.get() {
            // SAFETY: a stream this card created and still holds on this thread's bound context.
            if let Err(error) = unsafe { result::stream::synchronize(self.stream.stream) } {
                abort_unconfirmed("synchronize before read-back", error);
            }
            self.pending_work.set(false);
        }
        // SAFETY: this thread bound the context in `Gpu::fork`; `self.stream.stream` belongs to it.
        unsafe { dtoh_on_stream(source, self.stream.stream, "synchronize after read-back") }
    }

    /// Joins and destroys the stream, returning any teardown failure as an ordinary error once all
    /// work on the stream has been confirmed finished.
    pub(crate) fn finish(mut self) -> Result<(), String> {
        self.close()
    }

    fn close(&mut self) -> Result<(), String> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        if self.pending_work.get() {
            if let Err(error) = self.device.bind_to_thread() {
                abort_unconfirmed("bind context on stream close", error);
            }
            // SAFETY: a stream this card created and still holds; every host slice passed to
            // `copy` and every device buffer used on this stream outlives this stream.
            if let Err(error) = unsafe { result::stream::synchronize(self.stream.stream) } {
                abort_unconfirmed("synchronize on stream close", error);
            }
            self.pending_work.set(false);
        }
        let _held = self.event.lock().unwrap_or_else(PoisonError::into_inner);
        // All work on this stream is confirmed complete above (or in `read_back`), so a failure in
        // cudarc's `CudaStream::drop` (`wait_for` or `destroy` unwrapping) leaves no unconfirmed
        // transfer and surfaces as an ordinary error on `finish()` instead of aborting every
        // painter.
        catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: dropped once here (`self.closed` guards `Drop`) under the event lock.
            unsafe { ManuallyDrop::drop(&mut self.stream) };
        }))
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
    use std::io::{self, ErrorKind};

    struct FailingWriter(ErrorKind);

    impl Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(self.0, "simulated stderr failure"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::new(self.0, "simulated stderr failure"))
        }
    }

    /// A failing stderr sink (full disk or broken pipe) never panics on the fatal diagnostic path.
    #[test]
    fn abort_diagnostic_ignores_failing_sink() {
        write_abort_diagnostic(&mut FailingWriter(ErrorKind::StorageFull), "read-back", 700);
        write_abort_diagnostic(&mut FailingWriter(ErrorKind::BrokenPipe), "read-back", 700);

        let mut buffer = Vec::new();
        write_abort_diagnostic(&mut buffer, "read-back", "CUDA_ERROR_ILLEGAL_ADDRESS");
        let text = String::from_utf8(buffer).unwrap();
        assert!(text.contains("CUDA read-back failed"));
        assert!(text.contains("CUDA_ERROR_ILLEGAL_ADDRESS"));
    }

    #[test]
    fn panic_message_handles_known_and_unknown_payloads() {
        assert_eq!(panic_message(Box::new(String::from("owned"))), "owned");
        assert_eq!(panic_message(Box::new("static")), "static");
        assert_eq!(panic_message(Box::new(42u32)), "driver unwrap panicked");
    }
}

//! The card's side streams: forked from its default stream once that stream's work so far is done,
//! joined back when dropped.
//!
//! Every buffer is allocated and freed on the default stream, where a later allocation may reuse
//! the memory of a freed buffer: a side stream may use a buffer only once it follows that
//! allocation, and a launch's buffers are declared before its stream, so that the stream has
//! joined before they are freed. Host buffers passed to [`ForkedStream::copy`] are tied to the
//! stream's `'host` lifetime and stay alive and unchanged until a host-side synchronize confirms
//! completion (on normal return, early return or unwind). Whenever a synchronize or join fails to
//! confirm completion, the process aborts so no transfer can outlive its buffer and no square of
//! that process can be published after the error.
//!
//! cudarc orders fork and join through the device's one event, recorded on one stream and waited on
//! by the other in two calls. Another thread recording it between them orders the stream after that
//! thread's work instead: kernels could start before their buffers are theirs, or buffers be freed
//! under them (a review of r066, 2026-10-15; the painters and the preparer share the card). The
//! card's lock makes each record and wait one step.

use crate::device::{Gpu, driver};
use cudarc::driver::{
    CudaDevice, CudaSlice, CudaStream, DevicePtr, DevicePtrMut, DeviceRepr, DeviceSlice,
    DriverError, ValidAsZeroBits, result,
};
use std::cell::Cell;
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::ops::Deref;
use std::sync::{Arc, Mutex, PoisonError};

pub(crate) struct ForkedStream<'a, 'host> {
    stream: ManuallyDrop<CudaStream>,
    device: &'a Arc<CudaDevice>,
    event: &'a Mutex<()>,
    /// True from an asynchronous HtoD copy until a host-side synchronize confirms it finished.
    pending_host_copy: Cell<bool>,
    /// Ties every borrowed host source passed to [`Self::copy`] (invariant in `'host`) so Rust
    /// requires each one to outlive this stream and remain unchanged until [`Drop`] has
    /// synchronized it.
    _host: PhantomData<Cell<&'host ()>>,
}

/// Aborts the process if dropped during an unwind (for example, `CudaStream::drop` panicking on a
/// failed driver join or destroy) instead of unwinding into `catch_unwind`.
struct AbortOnUnwind(&'static str);

impl Drop for AbortOnUnwind {
    fn drop(&mut self) {
        eprintln!(
            "qm-paint: CUDA {} panicked; aborting to avoid unconfirmed stream state",
            self.0
        );
        std::process::abort();
    }
}

fn abort_stream(step: &str, error: DriverError) -> ! {
    eprintln!("qm-paint: CUDA {step} failed ({error}); aborting to avoid unconfirmed transfers");
    std::process::abort();
}

impl Gpu {
    /// A stream whose work follows the default stream's so far (the buffers just allocated there,
    /// and what a launch still copies or zeroes there).
    pub(crate) fn fork<'a, 'host>(&'a self) -> Result<ForkedStream<'a, 'host>, String> {
        let _held = self.event.lock().unwrap_or_else(PoisonError::into_inner);
        let abort = AbortOnUnwind("stream fork");
        let stream = driver(self.device.fork_default_stream());
        std::mem::forget(abort);
        Ok(ForkedStream {
            stream: ManuallyDrop::new(stream?),
            device: &self.device,
            event: &self.event,
            pending_host_copy: Cell::new(false),
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

    /// `values` copied into `buffer` on this stream, in its order: a kernel launched on it after
    /// reads them. Copied on the default stream, each copy waited for there, a launch's six
    /// inputs waited six times for whatever the passes of the squares had on that one stream.
    ///
    /// # Safety
    /// The caller must not leak this stream (for example with `std::mem::forget`): `values` is
    /// tied to `'host` so Rust keeps it alive and unchanged until this stream drops, and [`Drop`]
    /// synchronizes the stream on the host (or aborts) before `values` can be dropped or modified.
    pub(crate) unsafe fn copy<T: DeviceRepr>(
        &self,
        values: &'host [T],
        buffer: &mut CudaSlice<T>,
    ) -> Result<(), String> {
        if values.len() != buffer.len() {
            return Err("a buffer of another length than the values copied into it".into());
        }
        self.pending_host_copy.set(true);
        // SAFETY: the buffer holds as many values as are copied; `values` outlives this stream,
        // whose `Drop` synchronizes the stream on the host (or aborts) before `values` can drop.
        driver(unsafe {
            result::memcpy_htod_async(*buffer.device_ptr_mut(), values, self.stream.stream)
        })
    }

    /// `source` read back once the stream's work so far is done. Waiting on the stream with a
    /// synchronize before the copy has been observed to use more CPU time and finish squares
    /// faster than letting the copy wait for the stream on its own, though the driver does not
    /// guarantee spin-waiting (2026-11-19). If either synchronize fails to confirm completion, the
    /// process aborts before `values` can be dropped or another square published.
    pub(crate) fn read_back<T: DeviceRepr + Default + Clone>(
        &self,
        source: &CudaSlice<T>,
    ) -> Result<Vec<T>, String> {
        let mut values = vec![T::default(); source.len()];
        // SAFETY: a stream this card created; `values` holds `source.len()` values and stays
        // alive until the trailing synchronize returns success (a failure aborts before `values`
        // can drop).
        unsafe {
            if let Err(error) = result::stream::synchronize(self.stream.stream) {
                abort_stream("synchronize before read-back", error);
            }
            self.pending_host_copy.set(false);
            if let Err(error) =
                result::memcpy_dtoh_async(&mut values, *source.device_ptr(), self.stream.stream)
            {
                if let Err(sync_error) = result::stream::synchronize(self.stream.stream) {
                    abort_stream("synchronize after failed read-back", sync_error);
                }
                return driver(Err(error));
            }
            if let Err(error) = result::stream::synchronize(self.stream.stream) {
                abort_stream("synchronize after read-back", error);
            }
        }
        Ok(values)
    }

    /// `buffer` zeroed on this stream, in its order: a kernel launched on it after reads zeros.
    pub(crate) fn zero<T: DeviceRepr + ValidAsZeroBits>(
        &self,
        buffer: &mut CudaSlice<T>,
    ) -> Result<(), String> {
        let bytes = buffer.num_bytes();
        // SAFETY: the buffer's own bytes, which are valid as zeros.
        driver(unsafe {
            result::memset_d8_async(*buffer.device_ptr_mut(), 0, bytes, self.stream.stream)
        })
    }
}

impl Deref for ForkedStream<'_, '_> {
    type Target = CudaStream;

    fn deref(&self) -> &CudaStream {
        &self.stream
    }
}

impl Drop for ForkedStream<'_, '_> {
    fn drop(&mut self) {
        if self.pending_host_copy.get() {
            if let Err(error) = self.device.bind_to_thread() {
                abort_stream("bind context on drop", error);
            }
            // SAFETY: a stream this card created and still holds; every host slice passed to
            // `copy` outlives this stream and stays alive until synchronize returns.
            if let Err(error) = unsafe { result::stream::synchronize(self.stream.stream) } {
                abort_stream("synchronize on drop", error);
            }
        }
        let _held = self.event.lock().unwrap_or_else(PoisonError::into_inner);
        // `CudaStream::drop` joins into the default stream and destroys the stream, unwrapping
        // any driver error. Abort on unwind so an unconfirmed join never returns through
        // `catch_unwind` while device buffers free or other painters publish.
        let abort = AbortOnUnwind("stream join or destroy");
        // SAFETY: the stream is dropped once, here, joined into the default stream under the lock.
        unsafe { ManuallyDrop::drop(&mut self.stream) };
        std::mem::forget(abort);
    }
}

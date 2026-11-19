//! The card's side streams: forked from its default stream once that stream's work so far is done,
//! joined back when dropped.
//!
//! Every buffer is allocated and freed on the default stream, whose pool hands a freed buffer's
//! memory to the next allocation there: a side stream may use a buffer only once it follows that
//! allocation, and a launch's buffers are declared before its stream, so that the stream has
//! joined before they are freed. cudarc orders both through the device's one event, recorded on
//! one stream and waited on by the other in two calls. Another thread recording it between them
//! orders the stream after that thread's work instead: kernels could start before their buffers
//! are theirs, or buffers be freed under them (a review of r066, 2026-10-15; the painters and
//! the preparer share the card). The card's lock makes each record and wait one step.

use crate::device::{Gpu, driver};
use cudarc::driver::{
    CudaSlice, CudaStream, DevicePtr, DevicePtrMut, DeviceRepr, DeviceSlice, ValidAsZeroBits,
    result,
};
use std::mem::ManuallyDrop;
use std::ops::Deref;
use std::sync::{Mutex, PoisonError};

pub(crate) struct ForkedStream<'a> {
    stream: ManuallyDrop<CudaStream>,
    event: &'a Mutex<()>,
}

impl Gpu {
    /// A stream whose work follows the default stream's so far (the buffers just allocated there,
    /// and what a launch still copies or zeroes there).
    pub(crate) fn fork(&self) -> Result<ForkedStream<'_>, String> {
        let _held = self.event.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(ForkedStream {
            stream: ManuallyDrop::new(driver(self.device.fork_default_stream())?),
            event: &self.event,
        })
    }
}

impl ForkedStream<'_> {
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
    /// `values` stay alive and unchanged until the stream is next synchronized: until then the
    /// driver may still read them. A launch that fails before that returns without it, relying
    /// on the driver having staged pageable memory during the call, which it does not promise; a
    /// copy read too late would reach that launch's own buffer alone.
    pub(crate) unsafe fn copy<T: DeviceRepr>(
        &self,
        values: &[T],
        buffer: &mut CudaSlice<T>,
    ) -> Result<(), String> {
        if values.len() != buffer.len() {
            return Err("a buffer of another length than the values copied into it".into());
        }
        // SAFETY: the buffer holds as many values as are copied; the caller keeps `values`.
        driver(unsafe {
            result::memcpy_htod_async(*buffer.device_ptr_mut(), values, self.stream.stream)
        })
    }

    /// `source` read back once the stream's work so far is done. That work is waited for as a
    /// synchronize, which holds a core and answers at once; the copy's own wait for its stream
    /// answers later: with every read-back waited for so, where only a launch's results had
    /// been, the painter took more of the cores and less time for its squares on two kinds of
    /// card (2026-11-19).
    pub(crate) fn read_back<T: DeviceRepr + Default + Clone>(
        &self,
        source: &CudaSlice<T>,
    ) -> Result<Vec<T>, String> {
        let mut values = vec![T::default(); source.len()];
        // SAFETY: a stream this card created; `values` holds `source.len()` values, read once
        // the stream is synchronized.
        unsafe {
            driver(result::stream::synchronize(self.stream.stream))?;
            driver(result::memcpy_dtoh_async(
                &mut values,
                *source.device_ptr(),
                self.stream.stream,
            ))?;
            driver(result::stream::synchronize(self.stream.stream))?;
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

impl Deref for ForkedStream<'_> {
    type Target = CudaStream;

    fn deref(&self) -> &CudaStream {
        &self.stream
    }
}

impl Drop for ForkedStream<'_> {
    fn drop(&mut self) {
        let _held = self.event.lock().unwrap_or_else(PoisonError::into_inner);
        // SAFETY: the stream is dropped once, here, joined into the default stream under the lock.
        unsafe { ManuallyDrop::drop(&mut self.stream) };
    }
}

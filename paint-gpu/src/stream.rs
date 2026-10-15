//! The card's side streams: forked from its default stream once that stream's work so far is done,
//! joined back when dropped.
//!
//! cudarc orders both through the device's one event, recorded on one stream and waited on by the
//! other in two calls. Another thread recording it between them orders the stream after that
//! thread's work instead: kernels could start before their buffers are zeroed, or buffers be freed
//! under them (a review of r066, 2026-10-15; two painters and the preparer share the card). The
//! card's lock makes each record and wait one step.

use crate::device::{Gpu, driver};
use cudarc::driver::CudaStream;
use std::mem::ManuallyDrop;
use std::ops::Deref;
use std::sync::{Mutex, PoisonError};

pub(crate) struct ForkedStream<'a> {
    stream: ManuallyDrop<CudaStream>,
    event: &'a Mutex<()>,
}

impl Gpu {
    /// A stream whose work follows the default stream's so far (the buffers just zeroed there).
    pub(crate) fn fork(&self) -> Result<ForkedStream<'_>, String> {
        let _held = self.event.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(ForkedStream {
            stream: ManuallyDrop::new(driver(self.device.fork_default_stream())?),
            event: &self.event,
        })
    }
}

impl ForkedStream<'_> {
    /// The stream's next work follows the default stream's so far as well (buffers zeroed since).
    pub(crate) fn wait_for_default(&self) -> Result<(), String> {
        let _held = self.event.lock().unwrap_or_else(PoisonError::into_inner);
        driver(self.stream.wait_for_default())
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

//! The heatmap painter on CUDA GPUs: the popup's physics as device code (`kernels/`, in single
//! precision, each function citing its Rust), compiled at run time for the card, fed a square's
//! tiles as stored. `qm-paint` paints with the card when there is one; `qm-paint-gpu check` holds
//! the card equal to the CPU on random pairs.
//!
//! Map: [`device`] (the card, the kernels, a square's buffers and the pair evaluation), [`batch`]
//! (the card as the painter's batch).

pub mod batch;
pub mod device;

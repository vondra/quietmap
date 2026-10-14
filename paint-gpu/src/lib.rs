//! The heatmap painter on CUDA GPUs: the popup's physics as device code (`kernels/`, in single
//! precision, each function citing its Rust), compiled at run time for the card, fed a square's
//! tiles as stored. `qm-paint` paints with the card when there is one; `qm-paint-gpu check` holds
//! the card equal to the CPU on random pairs.
//!
//! Map: [`device`] (the card, the kernels, a square's buffers and the pair evaluation), [`loudness`]
//! (the pixels' loudness), [`aircraft`] (the aircraft boxes at the corners' lattices), [`batch`]
//! (the card as the painter's batch).
//!
//! For visitors, down to this code: the About page The noise map
//! (`frontend/src/about/the-noise-map.md`).

pub mod aircraft;
pub mod batch;
pub mod device;
pub mod loudness;
pub mod square_writer;

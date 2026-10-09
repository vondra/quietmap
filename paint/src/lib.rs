//! The heatmap: every pixel's Lden per layer on the popup's physics, a z12 square at a time.
//!
//! Map: [`square`] (a square's neighbourhood read once, its sources indexed by place), [`levels`]
//! (each source's energy split by distance into the near level evaluated at every pixel, the mid
//! level at the corners of the pixel blocks and the far level at the corners of the coarse cells;
//! each point's evaluation from the largest bound, sampled past a number), [`paint`] (the square's
//! pixels) and [`hm3`] (the map's tiles).

pub mod hm3;
pub mod levels;
pub mod pack;
pub mod paint;
pub mod square;

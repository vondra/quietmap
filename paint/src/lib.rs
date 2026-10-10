//! The heatmap: every pixel's Lden per layer on the popup's physics, a z12 square at a time.
//!
//! Map: [`square`] (a square's neighbourhood read once, its sources indexed by place and reach),
//! [`exact`] (the points and the exact evaluation of (point, source) pairs, here or on a card),
//! [`lattice`] (the coarse lattice: the loud sources exact, the quiet hum as the popup estimates
//! it), [`paint`] (the square's pixels: the loud sources near a block exact, the others in groups
//! each set right by one probe, `groups`; the flights, `flights`), [`etalon`] (every pixel as the
//! popup answers it, the painter's reference),
//! [`hm3`] and [`pack`] (the map's tiles).

pub mod etalon;
pub mod exact;
mod flights;
mod groups;
pub mod hm3;
pub mod lattice;
pub mod pack;
pub mod paint;
pub mod square;

//! The heatmap: every pixel's Lden per layer on the popup's physics, a z12 square at a time.
//!
//! Map: [`square`] (a square's neighbourhood read once, its sources indexed by place and reach),
//! [`exact`] (the points and the exact evaluation of (point, source) pairs, here or on a card),
//! [`hum`] (the quiet hum as the popup estimates it, at points 128 pixels apart), [`paint`] (the
//! square's pixels in blocks, as dev1 painted them; a pass's points, `points`; the buildings,
//! `buildings`; the flights, `flights`), [`etalon`] (every outdoor pixel as the popup answers it,
//! the painter's reference),
//! [`hm3`] and [`pack`] (the map's tiles).

mod blocks;
mod buildings;
pub mod etalon;
pub mod exact;
mod flights;
pub mod hm3;
pub mod hum;
mod loudest;
pub mod pack;
pub mod paint;
mod points;
pub mod sone;
pub mod square;

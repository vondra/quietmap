//! Acoustic kernels: CNOSSOS-EU propagation and emission, ISO 9613-2's foliage, ECAC Doc 29, with
//! the standards' test cases. Pure computation, no I/O.
//!
//! For visitors, down to this code: the About pages Data preparation (emission) and Noise at a
//! point (propagation, loudness), `frontend/src/about/`.
//!
//! Map: [`bands`] (bands, periods, Lden), [`atmosphere`] (ISO 9613-1), [`cnossos`] (the vertical
//! path per meteorological state), [`foliage`] (tree crowns), [`line`] (the point-sum quadrature
//! of a line piece), [`bound`] (the upper bound behind the popup's stop rule), [`profile`] (the
//! ground under a ray), [`weather`] (favourable probability), [`ray`] (one ray's transfer),
//! [`doc29`] (aircraft).

pub mod atmosphere;
pub mod bands;
pub mod bound;
pub mod cnossos;
pub mod doc29;
pub mod emission;
pub mod foliage;
pub mod line;
pub mod loudness;
pub mod percentile;
pub mod profile;
pub mod ray;
pub mod weather;

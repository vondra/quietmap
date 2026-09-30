//! Acoustic kernels: CNOSSOS-EU propagation and emission, ECAC Doc 29, with the standards' test
//! cases. Pure computation, no I/O.
//!
//! Map: [`bands`] (bands, periods, Lden), [`atmosphere`] (ISO 9613-1), [`cnossos`] (the vertical
//! path per meteorological state), [`line`] (the point-sum quadrature of a line piece), [`bound`]
//! (the upper bound behind the popup's stop rule), [`profile`] (the ray cadence), [`foliage`]
//! (ISO 9613-2 Table A.1), [`weather`] (favourable probability), [`ray`] (one ray's transfer),
//! [`doc29`] (aircraft).

pub mod atmosphere;
pub mod bands;
pub mod bound;
pub mod cnossos;
pub mod doc29;
pub mod emission;
pub mod foliage;
pub mod line;
pub mod percentile;
pub mod profile;
pub mod ray;
pub mod weather;

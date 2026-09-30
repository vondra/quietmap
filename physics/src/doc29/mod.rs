//! ECAC Doc 29 (4th ed., Vol 2) aircraft noise: the NPD curves of 15 noise classes interpolated by
//! per-segment thrust, EASA-certified helicopter levels, the corrections of Eq. 4-8b, dev4's
//! terrain and building screening, and one segment's exact SEL at a receiver, which is both the
//! reference of the aircraft boxes and the source of their levels at the ten NPD distances.
//!
//! Map: [`npd`] (curves, interpolation, tail, scaled distance, power rows per class), [`thrust`]
//! (a segment's power bracket), [`helicopters`] (certified levels per designator), [`corrections`]
//! (Delta_V, Delta_F, Lambda, Delta_I), [`screening`] (edge loss and its composition with Lambda),
//! [`segment`] (a segment at a receiver), [`box_sums`] and [`boxes`] (an aircraft box: the sums a
//! builder keeps, and the click-time equation), [`bound`] (upper bound for the stop rule); generated
//! tables [`profiles_generated`] (profiles, classes, designator mapping) and [`thrust_generated`]
//! (thrust rows per class).

pub mod bound;
pub mod box_sums;
pub mod boxes;
pub mod corrections;
pub mod helicopters;
pub mod npd;
pub mod profiles_generated;
pub mod screening;
pub mod segment;
pub mod thrust;
pub mod thrust_generated;

#[cfg(test)]
mod designator_tests;

//! Emission models, evaluated when tiles are built: [`road`] (CNOSSOS-EU road traffic), [`rail`]
//! (railways, trams and level-crossing horns), [`airport`] (aircraft and vehicles on runways and
//! taxiways), and the point and area sources stated as a
//! [`spectrum::SoundPower`]: [`industrial`] (sites, solar farms, substations), [`wind`]
//! (turbines), [`settlement`] (buildings), [`leisure`] (open-air leisure, shooting ranges) and
//! [`ships`] (AIS vessel-density cells).

pub mod airport;
pub mod industrial;
pub mod leisure;
pub mod people;
pub mod rail;
pub mod road;
pub mod settlement;
pub mod ships;
pub mod spectrum;
pub mod wind;

/// Evening and night of a day-only source (solar inverters, shooting): dev4's -50 dB, silent in
/// effect.
pub const DAY_ONLY_OFFSET_DB: f64 = -50.0;

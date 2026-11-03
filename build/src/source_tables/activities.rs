//! Observed activity and local practice; missing rows leave only directly mapped activity.

use crate::sources::prayer_times::Method;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VenueSeating {
    pub beer_garden_m2: f64,
    pub seats: [[f64; 2]; 6],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorshipCalls {
    pub method: Method,
    pub sounding: [(f64, f64); 5],
    pub sound_power_dba: f64,
    pub iqama_seconds: f64,
    pub skip_dawn: bool,
    pub friday_only: bool,
    pub silent: bool,
    pub sala_seconds: f64,
    pub horn_height_m: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChurchBells {
    pub orthodox: bool,
    pub strikes_per_hour: f64,
    pub night_strikes: f64,
}

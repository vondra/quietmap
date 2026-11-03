//! Fitted traffic laws' coefficients, with the training inputs and vintage recorded in the manifest.

use super::table::country_code;
use serde::{Deserialize, Deserializer};

fn country<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[u8; 2], D::Error> {
    country_code(&String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
}

fn countries<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<[u8; 2]>, D::Error> {
    Vec::<String>::deserialize(deserializer)?
        .iter()
        .map(|code| country_code(code).map_err(serde::de::Error::custom))
        .collect()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MainModel {
    pub class: u8,
    pub radius_m: f64,
    pub most_around: f64,
    pub intercept: [f64; 2],
    pub slope: f64,
    pub oneway: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MajorModel {
    pub intercept: [f64; 2],
    pub slopes: [f64; 3],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CountryShift {
    #[serde(deserialize_with = "country")]
    pub country: [u8; 2],
    pub value: [[f64; 2]; 2],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionalMainModels {
    #[serde(deserialize_with = "country")]
    pub country: [u8; 2],
    pub models: Vec<MainModel>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoadModels {
    pub main: Vec<MainModel>,
    pub regional_main: Vec<RegionalMainModels>,
    pub major: [MajorModel; 4],
    pub major_radii_m: [f64; 3],
    pub major_oneway: f64,
    pub major_ln_lanes: f64,
    pub major_lanes_unknown: f64,
    #[serde(deserialize_with = "countries")]
    pub major_countries: Vec<[u8; 2]>,
    pub country_shifts_db: Vec<CountryShift>,
    #[serde(deserialize_with = "countries")]
    pub selective_tertiary_countries: Vec<[u8; 2]>,
    pub unselected_tertiary_shift_db: Vec<(f64, f64)>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BusModel {
    pub one_line_direction: f64,
    pub line_directions_exponent: f64,
    pub trolleybus_line_weight: f64,
    pub coach_departures: f64,
    pub period_shares: [f64; 3],
    pub city_trip_ends: f64,
    pub service_exponent: f64,
    pub most_trip_ends: f64,
    pub least_service: f64,
    pub service_by_built_up: [f64; 3],
}

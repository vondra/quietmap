//! Country polygons in longitude/latitude, with explicit disputed-land attribution.

use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RegionFile {
    pub regions: Vec<RegionInput>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RegionInput {
    pub country: Option<String>,
    pub continent: u8,
    pub disputed: bool,
    pub polygons: Vec<Vec<Vec<[f64; 2]>>>,
}

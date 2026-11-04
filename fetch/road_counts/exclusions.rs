//! Retain nearby competing network geometry so a provincial count cannot be assigned to a national road.

use super::{
    geometry::{GeometryFormat, geometry},
    records,
};
use crate::{roads::observation::Observation, year::YearInputs};
use anyhow::{Result, ensure};
use rstar::{AABB, RTree, RTreeObject};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    pub input_id: String,
    pub files: Vec<PathBuf>,
    pub radius_m: f64,
}

#[derive(Clone)]
struct Segment([[f64; 2]; 2]);
impl RTreeObject for Segment {
    type Envelope = AABB<[f64; 2]>;
    fn envelope(&self) -> Self::Envelope {
        AABB::from_points(&self.0)
    }
}

pub(super) fn attach(recipe: &Recipe, inputs: &YearInputs, rows: &mut [Observation]) -> Result<()> {
    ensure!(
        recipe.radius_m.is_finite() && recipe.radius_m > 0.0,
        "invalid network exclusion radius"
    );
    let path = inputs.by_id(&recipe.input_id).map_err(anyhow::Error::msg)?;
    let mut segments = Vec::new();
    for feature in records::pages(&path, &recipe.files)? {
        for part in geometry(&feature, &serde_json::Value::Null, &GeometryFormat::Geojson)? {
            segments.extend(part.windows(2).map(|p| Segment([p[0], p[1]])));
        }
    }
    ensure!(!segments.is_empty(), "excluded network contains no lines");
    let tree = RTree::bulk_load(segments);
    for row in rows {
        let points = row.geometry.iter().flatten().copied().collect::<Vec<_>>();
        ensure!(
            !points.is_empty(),
            "network exclusion needs a located count"
        );
        let bounds = AABB::from_points(&points);
        let [west, south] = bounds.lower();
        let [east, north] = bounds.upper();
        let latitude = south.abs().max(north.abs());
        let reach = (recipe.radius_m + row.search_radius_m) / 110_540.0;
        let longitude = (reach / latitude.to_radians().cos()).min(360.0);
        let query = AABB::from_corners(
            [west - longitude, south - reach],
            [east + longitude, north + reach],
        );
        row.target.excluded_segments = tree
            .locate_in_envelope_intersecting(&query)
            .map(|s| s.0)
            .collect();
        row.target.exclusion_radius_m = recipe.radius_m;
    }
    Ok(())
}

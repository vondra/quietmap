//! Public raster semantics and source precedence, selected by the year manifest.
use crate::year::Manifest;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum Band {
    Coverage,
    GroundHeight,
    Hardness,
    Sealing,
    TreeCover,
    TreeLoss,
    TreeHeight,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RasterSource {
    pub input: String,
    pub band: Band,
    pub horizontal_crs: String,
    pub vertical_crs: Option<u32>,
    #[serde(deserialize_with = "nodata")]
    pub nodata: Option<f64>,
    pub priority: u32,
    #[serde(default)]
    pub classes: BTreeMap<u16, u8>,
    #[serde(default)]
    pub feather_nodes: u32,
    pub datum_horizontal_crs: Option<String>,
    pub datum_area: Option<[f64; 4]>,
    pub loss_base_year: Option<i32>,
    pub loss_input: Option<String>,
    #[serde(default)]
    pub global_nearest: bool,
    pub antimeridian_input: Option<String>,
}

fn nodata<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<f64>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Number(number) => number
            .as_f64()
            .map(Some)
            .ok_or_else(|| serde::de::Error::custom("invalid nodata number")),
        serde_json::Value::String(name) if name == "nan" => Ok(Some(f64::NAN)),
        _ => Err(serde::de::Error::custom(
            "nodata must be a number, null, or nan",
        )),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RasterPlan {
    pub sources: Vec<RasterSource>,
    pub runtime: super::runtime::RasterRuntime,
}

impl RasterPlan {
    pub fn validate(&self, manifest: &Manifest) -> Result<(), String> {
        let mut priorities = BTreeSet::new();
        for source in &self.sources {
            if !manifest.inputs.iter().any(|i| i.id == source.input) {
                return Err(format!("raster source {} is not an input", source.input));
            }
            if !priorities.insert((source.band, source.priority)) {
                return Err(format!("duplicate raster priority for {:?}", source.band));
            }
            if source.horizontal_crs.is_empty()
                || source.nodata.is_some_and(|n| n.is_infinite())
                || source.classes.values().any(|v| *v > 100)
                || source.feather_nodes == 1
            {
                return Err(format!("invalid raster semantics for {}", source.input));
            }
            if (source.band != Band::GroundHeight
                && (source.feather_nodes != 0
                    || source.datum_horizontal_crs.is_some()
                    || source.datum_area.is_some()))
                || (source.band != Band::Hardness
                    && (!source.classes.is_empty() || source.global_nearest))
                || source.classes.keys().any(|code| *code > 255)
                || (source.antimeridian_input.is_some()
                    && !matches!(
                        source.band,
                        Band::Hardness | Band::TreeCover | Band::TreeLoss
                    ))
                || (source.global_nearest && source.antimeridian_input.is_some())
            {
                return Err(format!("inapplicable raster fields: {}", source.input));
            }
            if (source.band == Band::GroundHeight) != source.vertical_crs.is_some()
                || (source.band == Band::TreeLoss) != source.loss_base_year.is_some()
            {
                return Err(format!(
                    "missing or inappropriate datum/loss epoch: {}",
                    source.input
                ));
            }
            if let Some(loss) = &source.loss_input
                && (source.band != Band::TreeCover
                    || !self
                        .sources
                        .iter()
                        .any(|s| s.input == *loss && s.band == Band::TreeLoss))
            {
                return Err(format!("invalid forest loss input {loss}"));
            }
            if let Some(wrapped) = &source.antimeridian_input
                && !manifest
                    .inputs
                    .iter()
                    .any(|i| i.id == *wrapped && i.depends_on.contains(&source.input))
            {
                return Err(format!(
                    "antimeridian input {wrapped} must derive from {}",
                    source.input
                ));
            }
        }
        if self
            .sources
            .iter()
            .filter(|source| source.band == Band::Coverage)
            .count()
            != 1
        {
            return Err("raster plan needs exactly one land coverage".into());
        }
        let required: BTreeSet<_> = self
            .sources
            .iter()
            .map(|s| s.input.clone())
            .chain(
                self.sources
                    .iter()
                    .filter_map(|s| s.antimeridian_input.clone()),
            )
            .chain(std::iter::once(self.runtime.datum_input.clone()))
            .collect();
        let output = manifest
            .inputs
            .iter()
            .find(|i| i.kind == crate::year::Kind::Rasters)
            .ok_or("raster plan needs its output input")?;
        if output.depends_on.iter().cloned().collect::<BTreeSet<_>>() != required {
            return Err("raster dependencies differ from its plan".into());
        }
        self.runtime.validate()?;
        if !manifest
            .inputs
            .iter()
            .any(|i| i.id == self.runtime.datum_input)
        {
            return Err("unknown raster datum input".into());
        }
        for band in [
            Band::Coverage,
            Band::GroundHeight,
            Band::Hardness,
            Band::TreeCover,
            Band::TreeHeight,
        ] {
            if !self.sources.iter().any(|s| s.band == band) {
                return Err(format!("raster plan lacks {band:?}"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn manifest() -> Value {
        let bands = [
            "coverage",
            "ground-height",
            "hardness",
            "tree-cover",
            "tree-height",
        ];
        let sources: Vec<_> = bands
            .iter()
            .map(|band| {
                json!({
                    "input":band,"band":band,"horizontal_crs":"EPSG:4326", "nodata":null,
                    "vertical_crs":if *band == "ground-height" {Some(3855)} else {None},
                    "priority":0
                })
            })
            .collect();
        let dependencies: Vec<_> = bands.into_iter().chain(["datum"]).collect();
        let inputs: Vec<_> = dependencies
            .iter()
            .copied()
            .chain(["rasters"])
            .map(|id| {
                json!({
                    "id":id,"kind":if id == "rasters" {"rasters"} else {"raster-mosaic"},
                    "region":"world","publisher":"Survey","url":"https://example.org/data",
                    "observed":{"start":"2000","end":"2000"},"published":null,
                    "licence":"CC0","public_ok":true,"description":"Synthetic observation.",
                    "depends_on":if id == "rasters" {dependencies.clone()} else {vec![]}
                })
            })
            .collect();
        json!({"year":"2000","built":false,"as_of":"2001-01-01",
            "flows":{"start":"2000-01-01","end":"2001-01-01"},
            "inputs":inputs,"unavailable":[],"rasters":{"sources":sources,
                "runtime":{"gdal_version":"1","proj_version":"1","datum_input":"datum",
                    "files":{"proj.db":"0".repeat(64)}}}})
    }

    #[test]
    fn plan_refuses_ignored_fields_duplicate_coverage_and_incomplete_dependencies() {
        let validate = |value| {
            serde_json::from_value::<Manifest>(value)
                .unwrap()
                .validate()
        };
        validate(manifest()).unwrap();
        for (index, field, value) in [
            (0, "feather_nodes", json!(2)),
            (0, "datum_area", json!([0, 0, 1, 1])),
            (0, "datum_horizontal_crs", json!("EPSG:4326")),
            (0, "classes", json!({"1":0})),
            (0, "global_nearest", json!(true)),
            (1, "antimeridian_input", json!("coverage")),
            (2, "classes", json!({"256":0})),
        ] {
            let mut candidate = manifest();
            candidate["rasters"]["sources"][index][field] = value;
            assert!(validate(candidate).unwrap_err().contains("inapplicable"));
        }
        let mut candidate = manifest();
        let mut extra = candidate["rasters"]["sources"][0].clone();
        extra["priority"] = json!(1);
        candidate["rasters"]["sources"]
            .as_array_mut()
            .unwrap()
            .push(extra);
        assert!(validate(candidate).unwrap_err().contains("exactly one"));
        let mut candidate = manifest();
        candidate["inputs"]
            .as_array_mut()
            .unwrap()
            .last_mut()
            .unwrap()["depends_on"] = json!([]);
        assert!(
            validate(candidate)
                .unwrap_err()
                .contains("dependencies differ")
        );
    }
}

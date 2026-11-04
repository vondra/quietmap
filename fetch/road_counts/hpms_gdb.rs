//! Read FHWA full-extent geodatabases with their annual observation and independently reported vehicle classes.

use super::{
    Source,
    fields::{GeometryFormat, geometry},
    hpms_sections::{Section, class_mask, relabel_twins},
    records,
};
use crate::{
    roads::{
        observation::{CountBasis, Observation},
        observation_target::DistanceRule,
    },
    year::YearInputs,
};
use anyhow::{Result, anyhow, ensure};
use gdal::{
    Dataset,
    spatial_ref::{AxisMappingStrategy, CoordTransform, SpatialRef},
    vector::LayerAccess,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    pub source: Source,
    pub archive_directory: Option<String>,
    pub layers: Vec<String>,
    pub search_radius_m: f64,
    pub minimum_alignment_cosine: f64,
    twin_radius_m: f64,
    twin_total_ratio_tolerance: f64,
}

fn actual_count(kind: Option<&str>) -> bool {
    // FHWA TMG section 5.3: A/C originate in counts; B/D are models, E is an unspecified method.
    matches!(kind, Some("A" | "C"))
}

pub(super) fn convert(
    recipe: &Recipe,
    inputs: &YearInputs,
    path: &Path,
    scratch: &Path,
) -> Result<Vec<Observation>> {
    ensure!(!recipe.layers.is_empty(), "no declared HPMS layers");
    let expanded = recipe
        .archive_directory
        .as_ref()
        .map(|_| tempfile::tempdir_in(scratch))
        .transpose()?;
    let path = if let Some(directory) = &recipe.archive_directory {
        records::object_path(Path::new(""), Path::new(directory))?;
        // FileGDB seeks through large tables; repeatedly inflating a ZIP member makes a world census quadratic.
        let root = expanded.as_ref().unwrap().path();
        zip::ZipArchive::new(std::fs::File::open(path)?)?.extract(root)?;
        root.join(directory)
    } else {
        path.to_path_buf()
    };
    let dataset = Dataset::open(path)?;
    let mut wgs84 = SpatialRef::from_epsg(4326)?;
    wgs84.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
    let mut rows = Vec::new();
    for name in &recipe.layers {
        let mut layer = dataset.layer_by_name(name)?;
        let mut source_crs = layer
            .spatial_ref()
            .ok_or_else(|| anyhow!("HPMS layer lacks its CRS"))?;
        source_crs.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
        let projection = CoordTransform::new(&source_crs, &wgs84)?;
        for feature in layer.features() {
            let number = |name: &str| -> Result<Option<f64>> {
                Ok(feature.field_as_double(feature.field_index(name)?)?)
            };
            let text = |name: &str| -> Result<Option<String>> {
                Ok(feature.field_as_string(feature.field_index(name)?)?)
            };
            let Some(total) = number("aadt")?.filter(|v| *v > 0.0 && v.is_finite()) else {
                continue;
            };
            let Some(function) = number("f_system")?.filter(|v| (1.0..=7.0).contains(v)) else {
                continue;
            };
            let Some(facility) =
                number("facility_type")?.filter(|v| [1.0, 2.0, 4.0, 6.0].contains(v))
            else {
                continue;
            };
            if number("is_restricted")? == Some(1.0) {
                continue;
            }
            let Some(year) = number("data_year")? else {
                continue;
            };
            let identity = format!(
                "{name}/{}",
                feature
                    .fid()
                    .ok_or_else(|| anyhow!("HPMS feature has no identity"))?
            );
            let mut row = recipe.source.observation(
                inputs,
                identity,
                if facility == 1.0 || facility == 4.0 {
                    CountBasis::Directional
                } else {
                    CountBasis::BothDirections
                },
            )?;
            if !records::survey_year(&mut row, &recipe.source, inputs, year as i32)? {
                continue;
            }
            let Some(shape) = feature.geometry() else {
                continue;
            };
            let transformed = shape.transform(&projection)?;
            let raw: Value = serde_json::from_str(&transformed.json()?)?;
            row.geometry = geometry(
                &json!({"geometry":raw}),
                &Value::Null,
                &GeometryFormat::Geojson,
            )?;
            if row.geometry.is_empty() {
                continue;
            }
            let rank = function as u8 - 1;
            let class = [0, 1, 2, 3, 4, 4, 5][usize::from(rank)];
            row.road_class_mask = class_mask(rank, Some(facility as u8));
            if row.road_class_mask == 0 {
                continue;
            }
            row.target.distance_rule = DistanceRule::NearestPiece;
            row.target.minimum_alignment_cosine = recipe.minimum_alignment_cosine;
            row.search_radius_m = recipe.search_radius_m;
            row.total = Some(total);
            row.measured = actual_count(text("aadt_vt")?.as_deref());
            row.estimated_classes = 15;
            for (class, field) in [(1, "aadt_single_unit"), (2, "aadt_combination")] {
                if let Some(value) =
                    number(field)?.filter(|v| v.is_finite() && *v >= 0.0 && *v <= total)
                {
                    row.vehicle_classes[class] = Some(value);
                    if actual_count(text(&format!("{field}_vt"))?.as_deref()) {
                        row.estimated_classes &= !(1 << class);
                    }
                }
            }
            if row.vehicle_classes.iter().flatten().sum::<f64>() > total {
                continue;
            }
            row.sample.road_class = Some(class);
            row.sample.environment = match number("urban_id")? {
                Some(99999.0) => 1,
                Some(n) if n > 0.0 => 2,
                _ => 0,
            };
            row.sample.length_m = number("sectionlength")?
                .filter(|m| *m > 0.0)
                .map(|m| m * 1609.344);
            let points = row.geometry.iter().flatten().collect::<Vec<_>>();
            let centroid = std::array::from_fn(|axis| {
                points.iter().map(|p| p[axis]).sum::<f64>() / points.len() as f64
            });
            rows.push(Section {
                row,
                rank,
                facility: Some(facility as u8),
                centroid,
            });
        }
        eprintln!(
            "road-counts: HPMS layer {name}: {} admitted sections",
            rows.len()
        );
    }
    relabel_twins(
        &mut rows,
        recipe.twin_radius_m,
        recipe.twin_total_ratio_tolerance,
    );
    Ok(rows.into_iter().map(|section| section.row).collect())
}

#[cfg(test)]
mod tests {
    #[test]
    fn modeled_or_unspecified_aadt_does_not_train_the_count_model() {
        for code in [Some("A"), Some("C")] {
            assert!(super::actual_count(code));
        }
        for code in [Some("B"), Some("D"), Some("E"), None] {
            assert!(!super::actual_count(code));
        }
    }
}

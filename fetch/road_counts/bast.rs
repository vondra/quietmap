//! BASt Zeilenformat workbooks preserve independent vehicle-class missingness and survey-point support.

use super::Source;
use crate::{
    roads::{
        observation::{CountBasis, Observation},
        observation_target::{DistanceRule, ReferenceFallback, ReferenceRule},
    },
    year::YearInputs,
};
use anyhow::{Result, anyhow, ensure};
use gdal::{
    Dataset, DatasetOptions, GdalOpenFlags,
    spatial_ref::{AxisMappingStrategy, CoordTransform, SpatialRef},
    vector::LayerAccess,
};
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    pub source: Source,
    sheet: String,
    reference_prefix: String,
    search_radius_m: f64,
    fallback: ReferenceFallback,
}

pub(super) fn convert(
    recipe: &Recipe,
    inputs: &YearInputs,
    path: &Path,
) -> Result<Vec<Observation>> {
    let dataset = Dataset::open_ex(
        path,
        DatasetOptions {
            open_flags: GdalOpenFlags::GDAL_OF_VECTOR,
            open_options: Some(&["FIELD_TYPES=STRING", "HEADERS=FORCE"]),
            ..Default::default()
        },
    )?;
    let mut sheet = dataset.layer_by_name(&recipe.sheet)?;
    let required = [
        "Str",
        "TKZST",
        "DTV",
        "DTVLVm",
        "DTVBus",
        "DTVLoA",
        "DTVLZ",
        "DTVKrad",
        "X_Koordinate",
        "Y_Koordinate",
    ];
    let columns = required
        .iter()
        .map(|s| sheet.defn().field_index(s))
        .collect::<Result<Vec<_>, _>>()?;
    let mut source = SpatialRef::from_epsg(25832)?;
    let mut target = SpatialRef::from_epsg(4326)?;
    source.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
    target.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
    let transform = CoordTransform::new(&source, &target)?;
    let mut rows = Vec::new();
    for feature in sheet.features() {
        let text = |i: usize| -> Result<String> {
            Ok(feature
                .field_as_string(columns[i])?
                .unwrap_or_default()
                .trim()
                .to_string())
        };
        let number = |i: usize| -> Result<Option<f64>> {
            let value = text(i)?;
            if value.is_empty() || value == "-" {
                return Ok(None);
            }
            let number: f64 = value
                .parse()
                .map_err(|_| anyhow!("invalid BASt numeric cell"))?;
            ensure!(
                number.is_finite() && number >= 0.0,
                "invalid BASt numeric cell"
            );
            Ok(Some(if i < 8 { number.round() } else { number }))
        };
        let reference = text(0)?;
        let identity = text(1)?;
        let (Some(total), Some(x), Some(y)) = (number(2)?, number(8)?, number(9)?) else {
            continue;
        };
        if identity.is_empty()
            || !reference.starts_with(&recipe.reference_prefix)
            || total <= 0.0
            || x < 100_000.0
            || y < 5_000_000.0
        {
            continue;
        }
        let mut x = [x];
        let mut y = [y];
        transform.transform_coords(&mut x, &mut y, &mut [])?;
        let light = number(3)?;
        let medium = number(4)?.zip(number(5)?).map(|(a, b)| a + b);
        let heavy = number(6)?;
        let motorcycle = number(7)?;
        let classes = [light, medium, heavy, motorcycle];
        let observed = classes.iter().flatten().sum::<f64>();
        // Published independent class rounding can differ from DTV by two vehicles.
        if observed == 0.0
            || (classes.iter().all(Option::is_some) && (observed - total).abs() > 2.0)
        {
            continue;
        }
        let mut row = recipe
            .source
            .observation(inputs, identity, CountBasis::BothDirections)?;
        row.vehicle_classes = classes;
        row.total = Some(total);
        row.geometry = vec![vec![[x[0], y[0]]]];
        row.reference = reference;
        row.reference_required = true;
        row.road_class_mask = 31;
        row.search_radius_m = recipe.search_radius_m;
        row.target.reference_rule = ReferenceRule::Whole;
        row.target.distance_rule = DistanceRule::GreatCircle;
        row.target.reference_fallback = Some(recipe.fallback.clone());
        rows.push(row);
    }
    ensure!(!rows.is_empty(), "BASt workbook has no usable observations");
    Ok(rows)
}

//! Declare publisher field semantics separately from record conversion.

use super::geometry::GeometryFormat;
use crate::roads::observation::{CountBasis, TimeBasis};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Publisher {
    pub input_id: String,
    pub(super) records: String,
    pub(super) properties: String,
    pub(super) identity: String,
    pub(super) reference: Option<String>,
    pub(super) geometry: GeometryFormat,
    pub(super) source_id: u16,
    pub(super) source_rank: u32,
    pub(super) country: String,
    pub(super) measured: bool,
    pub(super) count_basis: CountBasis,
    pub(super) time_basis: TimeBasis,
    pub(super) daily_factor: f64,
    pub(super) direction: u8,
    pub(super) road_class_mask: u16,
    pub(super) search_radius_m: f64,
    pub(super) reference_required: bool,
    pub(super) classes: [Option<Vec<String>>; 4],
    #[serde(default)]
    pub(super) class_groups: Vec<(u8, Vec<String>)>,
    pub(super) total: Option<String>,
    pub(super) year_field: Option<String>,
    pub(super) estimated_classes: u8,
    #[serde(default)]
    pub(super) reference_prefix_by_field: Option<(String, BTreeMap<String, String>)>,
    #[serde(default)]
    pub(super) class_masks_by_field: Option<(String, BTreeMap<String, u16>)>,
    #[serde(default)]
    pub(super) reference_alias_prefixes: Vec<String>,
    #[serde(default)]
    pub(super) reference_alias_suffixes: Vec<String>,
    #[serde(default)]
    pub(super) skip_zero_total: bool,
    #[serde(default)]
    pub(super) target: crate::roads::observation_target::Target,
    #[serde(default)]
    pub(super) files: Vec<std::path::PathBuf>,
    #[serde(default)]
    pub(super) minimum_total: f64,
    #[serde(default)]
    pub(super) total_fallback_fields: Vec<String>,
    #[serde(default)]
    pub(super) percentages: bool,
    #[serde(default)]
    pub(super) percentage_total: Option<f64>,
    #[serde(default)]
    pub(super) fixed_fractions: [Option<f64>; 4],
    #[serde(default)]
    pub(super) field_filter: Option<(String, String)>,
    #[serde(default = "unit_scale")]
    pub(super) total_scale: f64,
    #[serde(default)]
    pub coverage: Option<super::records::Coverage>,
    #[serde(default)]
    pub(super) residual_class: Option<usize>,
    #[serde(default)]
    pub(super) exclusion: Option<super::exclusions::Recipe>,
}

fn unit_scale() -> f64 {
    1.0
}

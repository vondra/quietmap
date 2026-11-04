//! Publisher coverage describes which road identity, area and geometric support an observation represents.

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReferenceRule {
    #[default]
    AnyToken,
    Whole,
    FirstValidToken,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DistanceRule {
    #[default]
    Midpoint,
    WholePiece,
    NearestPiece,
    PointToPiece,
    GreatCircle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum IdentityField {
    Name,
    Reference,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub field: IdentityField,
    pub pattern: String,
    pub road_class_mask: u16,
    pub priority: u16,
    #[serde(default)]
    pub prefer_first_match: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Target {
    #[serde(skip_serializing_if = "is_default")]
    pub required_oneway: Option<bool>,
    #[serde(skip_serializing_if = "is_default")]
    pub source_way: Option<i64>,
    #[serde(skip_serializing_if = "is_default")]
    pub way_assignment: Option<WayAssignment>,
    #[serde(skip_serializing_if = "is_default")]
    pub proximity: Option<Proximity>,
    #[serde(skip_serializing_if = "is_default")]
    pub street_assignment: Option<StreetAssignment>,
    #[serde(skip_serializing_if = "is_default")]
    pub reference_rule: ReferenceRule,
    #[serde(skip_serializing_if = "is_default")]
    pub reference_rewrites: Vec<ReferenceRewrite>,
    #[serde(skip_serializing_if = "is_default")]
    pub distance_rule: DistanceRule,
    #[serde(skip_serializing_if = "is_default")]
    pub identities: Vec<Identity>,
    #[serde(skip_serializing_if = "is_default")]
    pub reference_fallback: Option<ReferenceFallback>,
    #[serde(skip_serializing_if = "is_default")]
    pub minimum_alignment_cosine: f64,
    #[serde(skip_serializing_if = "is_default")]
    pub excluded_segments: Vec<[[f64; 2]; 2]>,
    #[serde(skip_serializing_if = "is_default")]
    pub exclusion_radius_m: f64,
    /// Polygon, ring, coordinate; the first ring is the exterior and the others are holes.
    #[serde(skip_serializing_if = "is_default")]
    pub coverage: Vec<Vec<Vec<[f64; 2]>>>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WayAssignment {
    pub maximum_distance_m: f64,
    pub clearance_m: f64,
    #[serde(default = "enabled")]
    pub require_class_clearance: bool,
    #[serde(default = "enabled")]
    pub extend_along_way: bool,
}

fn enabled() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Proximity {
    pub allowed_classes: u16,
    pub compatible_classes: u16,
    pub street_names: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StreetAssignment {
    pub candidate_classes: u16,
    pub point_band_m: f64,
    pub line_near_m: f64,
    pub street_hint: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceRewrite {
    pub pattern: String,
    pub replacement: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceFallback {
    pub road_class_mask: u16,
    pub search_radius_m: f64,
    pub reference_sources: Vec<u16>,
}

pub fn coordinate(point: &[f64; 2]) -> bool {
    point[0].is_finite()
        && point[1].is_finite()
        && (-180.0..=180.0).contains(&point[0])
        && (-90.0..=90.0).contains(&point[1])
}

impl Target {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.exclusion_radius_m.is_finite()
                && self.exclusion_radius_m >= 0.0
                && (self.excluded_segments.is_empty() || self.exclusion_radius_m > 0.0)
                && self.excluded_segments.iter().flatten().all(coordinate),
            "invalid excluded road support"
        );
        if let Some(street) = &self.street_assignment {
            ensure!(
                street.candidate_classes > 0
                    && street.candidate_classes < 1 << 13
                    && [street.point_band_m, street.line_near_m]
                        .iter()
                        .all(|v| v.is_finite() && *v >= 0.0),
                "invalid street assignment support"
            );
        }
        if let Some(proximity) = &self.proximity {
            ensure!(
                proximity.allowed_classes > 0
                    && proximity.allowed_classes < 1 << 13
                    && proximity.compatible_classes < 1 << 13,
                "invalid proximity class support"
            );
        }
        ensure!(
            (self.reference_rule == ReferenceRule::FirstValidToken)
                != self.reference_rewrites.is_empty(),
            "first-valid reference support needs explicit syntax"
        );
        for rewrite in &self.reference_rewrites {
            regex::Regex::new(&rewrite.pattern)?;
        }
        ensure!(
            self.source_way.is_none_or(|id| id > 0),
            "invalid publisher way identity"
        );
        if let Some(assignment) = &self.way_assignment {
            ensure!(
                assignment.maximum_distance_m.is_finite()
                    && assignment.maximum_distance_m > 0.0
                    && assignment.clearance_m.is_finite()
                    && assignment.clearance_m >= assignment.maximum_distance_m,
                "invalid counter-to-way support"
            );
        }
        if let Some(fallback) = &self.reference_fallback {
            ensure!(
                fallback.road_class_mask > 0
                    && fallback.road_class_mask < 1 << 13
                    && fallback.search_radius_m.is_finite()
                    && fallback.search_radius_m > 0.0
                    && !fallback.reference_sources.is_empty()
                    && !fallback.reference_sources.contains(&0),
                "invalid reference fallback support"
            );
        }
        ensure!(
            self.minimum_alignment_cosine.is_finite()
                && (0.0..=1.0).contains(&self.minimum_alignment_cosine),
            "invalid directional support"
        );
        for identity in &self.identities {
            ensure!(
                !identity.pattern.is_empty()
                    && identity.road_class_mask > 0
                    && identity.road_class_mask < 1 << 13,
                "invalid observation road identity"
            );
            regex::Regex::new(&identity.pattern)?;
        }
        ensure!(
            self.coverage.iter().all(|polygon| !polygon.is_empty()
                && polygon.iter().all(|ring| ring.len() >= 4
                    && ring.first() == ring.last()
                    && ring.iter().all(coordinate))),
            "invalid observation coverage rings"
        );
        Ok(())
    }
}

pub(super) fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

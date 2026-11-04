//! Manifest-bound classification rules, publisher recipes and geographic coverage are data.

use super::records::{Facility, INDUSTRY};
use anyhow::{Result, ensure};
use serde::Deserialize;
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub countries: Vec<u16>,
    pub bbox: [f64; 4],
    #[serde(default)]
    pub exclude: Vec<[f64; 4]>,
    #[serde(default)]
    pub include: Vec<[f64; 4]>,
    #[serde(default)]
    pub source_land: bool,
    #[serde(default)]
    pub bbox_ownership: bool,
}

impl Scope {
    pub fn area_contains(&self, latitude: f64, longitude: f64, country: u16) -> bool {
        let inside = |b: &[f64; 4]| {
            latitude >= b[0] && longitude >= b[1] && latitude <= b[2] && longitude <= b[3]
        };
        if self.source_land {
            return self.owns(latitude, longitude, country);
        }
        inside(&self.bbox)
            && !self.exclude.iter().any(inside)
            && (self.include.is_empty() || self.include.iter().any(inside))
    }
    pub fn owns(&self, latitude: f64, longitude: f64, country: u16) -> bool {
        if self.bbox_ownership {
            latitude >= self.bbox[0]
                && longitude >= self.bbox[1]
                && latitude <= self.bbox[2]
                && longitude <= self.bbox[3]
        } else {
            self.countries.contains(&country)
        }
    }
    pub fn mapped(&self, row: &Facility) -> bool {
        self.owns(row.latitude, row.longitude, row.country)
            && self.area_contains(row.latitude, row.longitude, row.country)
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusRule {
    pub fields: Vec<String>,
    pub pattern: String,
    #[serde(default)]
    pub operating_pattern: String,
    #[serde(default)]
    pub inactive_pattern: String,
    #[serde(default)]
    pub blank: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputSpec {
    pub input: String,
    pub format: String,
    pub classifier: String,
    pub source_id: u16,
    pub authority_rank: u8,
    pub authority_edition: u16,
    #[serde(default)]
    pub activity: u16,
    pub scope: Option<Scope>,
    pub status: Option<StatusRule>,
    #[serde(default)]
    pub identities: Vec<String>,
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(default)]
    pub capacity_fields: Vec<String>,
    #[serde(default)]
    pub capacity_unit: String,
    #[serde(default = "one")]
    pub capacity_scale: f64,
    #[serde(default)]
    pub start_fields: Vec<String>,
    #[serde(default)]
    pub end_fields: Vec<String>,
    #[serde(default)]
    pub geometry_types: Vec<String>,
    pub deduplicate_digits: Option<usize>,
    #[serde(default)]
    pub deduplicate_group: String,
    #[serde(default)]
    pub containment: bool,
    #[serde(default)]
    pub point_land: bool,
    #[serde(default)]
    pub known_empty: bool,
    pub border_names: Option<String>,
    pub minimum_voltage: Option<f64>,
    #[serde(default)]
    pub voltage_positive_only: bool,
    #[serde(default)]
    pub voltage_fields: Vec<String>,
}
fn one() -> f64 {
    1.0
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NameRule {
    pub keywords: Vec<String>,
    pub activity: u16,
    pub source_id: u16,
    pub authority_rank: u8,
    pub authority_edition: u16,
    pub bbox: Option<[f64; 4]>,
    #[serde(default)]
    pub excluded_countries: Vec<u16>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub inputs: Vec<InputSpec>,
    pub names: Vec<NameRule>,
    pub activity_order: BTreeMap<u16, f64>,
    pub coal_sources: Vec<u16>,
    pub subtype_divisions: BTreeMap<u8, Vec<u16>>,
    pub source_divisions: BTreeMap<u8, Vec<u16>>,
    pub overlap_minimum_m2: f64,
    pub overlap_area_ratio: f64,
}

impl Model {
    pub fn read(path: &Path) -> Result<Self> {
        let model: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        ensure!(
            model.overlap_minimum_m2 > 0.0 && model.overlap_area_ratio >= 1.0,
            "invalid facility overlap model"
        );
        let mut inputs = std::collections::BTreeSet::new();
        for spec in &model.inputs {
            ensure!(
                inputs.insert(&spec.input)
                    && spec.capacity_scale.is_finite()
                    && spec.capacity_scale > 0.0,
                "invalid or duplicate facility input"
            );
        }
        Ok(model)
    }
    pub fn accepts(&self, site: &Facility, activity: u16) -> bool {
        site.source_type < 10
            && [
                self.subtype_divisions.get(&site.site_subtype),
                self.source_divisions.get(&site.source_type),
            ]
            .into_iter()
            .flatten()
            .all(|divisions| divisions.contains(&(activity / 100)))
    }
    pub fn beats(&self, a: &Facility, b: &Facility) -> bool {
        let coal = |f: &Facility| self.coal_sources.contains(&f.source_id) && f.activity / 100 == 5;
        let mining = |f: &Facility| {
            !self.coal_sources.contains(&f.source_id) && matches!(f.activity / 100, 7 | 8)
        };
        if coal(a) && mining(b) {
            return true;
        }
        if coal(b) && mining(a) {
            return false;
        }
        let priority = |f: &Facility| (f.authority_rank, f.authority_edition, f.source_id);
        if priority(a) != priority(b) {
            return priority(a) > priority(b);
        }
        let loudness = |code: u16| {
            self.activity_order
                .get(&code)
                .or_else(|| self.activity_order.get(&(code / 100 * 100)))
                .copied()
                .unwrap_or(-1.0)
        };
        loudness(a.activity) > loudness(b.activity)
    }
}

impl Model {
    pub fn infer_names(&self, sites: &mut [Facility]) {
        for site in sites {
            if site.role != INDUSTRY || site.source_type >= 10 || site.classification == 1 {
                continue;
            }
            let lower = site.name.as_deref().unwrap_or("").to_lowercase();
            for rule in &self.names {
                if rule.bbox.is_some_and(|b| {
                    site.latitude < b[0]
                        || site.longitude < b[1]
                        || site.latitude > b[2]
                        || site.longitude > b[3]
                }) || rule.excluded_countries.contains(&site.country)
                {
                    continue;
                }
                if !rule.keywords.iter().any(|word| lower.contains(word)) {
                    continue;
                }
                if rule.activity > 0 && self.accepts(site, rule.activity) {
                    site.activity = rule.activity;
                    site.source_id = rule.source_id;
                    site.authority_rank = rule.authority_rank;
                    site.authority_edition = rule.authority_edition;
                    site.classification = 2;
                    site.source = "facility-model".into();
                }
                break;
            }
        }
    }
}

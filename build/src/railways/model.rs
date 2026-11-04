//! Manifest-selected provenance, category priors and period distributions for every network.
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRule {
    pub id: u16,
    pub rank: u8,
    pub edition: u16,
    pub owner: String,
    pub countries: Vec<String>,
    pub residual: bool,
    pub whole_line: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RailModel {
    pub network_bounds: BTreeMap<String, [f64; 4]>,
    pub residuals: BTreeMap<String, Residual>,
    pub spatial_priors: BTreeMap<u16, Vec<super::prior_rules::Rule>>,
    pub sources: Vec<SourceRule>,
    pub type_priors: Vec<[f64; 2]>,
    pub usage_priors: Vec<[f64; 2]>,
    pub mainline_freight: BTreeMap<String, f64>,
    pub passenger_periods: [f64; 3],
    pub freight_periods: [f64; 3],
    pub urban_periods: [f64; 3],
    pub regional_freight_periods: Vec<RegionalPeriods>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Residual {
    pub source_id: u16,
    pub passenger: f64,
    pub freight: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionalPeriods {
    pub countries: Vec<String>,
    pub periods: [f64; 3],
}

pub struct PeriodShares {
    pub pax: [f64; 3],
    pub frt: [f64; 3],
}

impl RailModel {
    pub fn read(path: &Path) -> Result<Self, String> {
        let model: Self = serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if model.type_priors.len() != 7 || model.usage_priors.len() != 4 {
            return Err("rail model needs every mapped rail type and usage".into());
        }
        for values in model.type_priors.iter().chain(&model.usage_priors) {
            if values.iter().any(|v| !v.is_finite() || *v < 0.0) {
                return Err("rail prior must be nonnegative and finite".into());
            }
        }
        for periods in [
            &model.passenger_periods,
            &model.freight_periods,
            &model.urban_periods,
        ]
        .into_iter()
        .chain(model.regional_freight_periods.iter().map(|r| &r.periods))
        {
            if periods.iter().any(|v| !v.is_finite() || *v < 0.0)
                || (periods.iter().sum::<f64>() - 1.0).abs() > 1e-12
            {
                return Err("rail period fractions must sum to one".into());
            }
        }
        let mut ids = std::collections::HashSet::new();
        for source in &model.sources {
            if source.id == 0
                || !ids.insert(source.id)
                || (!source.owner.is_empty() && !valid_country(&source.owner))
                || source
                    .countries
                    .iter()
                    .any(|country| !valid_country(country))
            {
                return Err("rail model has invalid or repeated source identity".into());
            }
        }
        for (scope, bounds) in &model.network_bounds {
            let (source, country) = scope.split_once(':').ok_or("invalid rail network scope")?;
            if !source.parse::<u16>().is_ok_and(|id| ids.contains(&id))
                || !valid_country(country)
                || !super::prior_rules::valid_bounds(*bounds)
            {
                return Err(format!("invalid rail network scope {scope}"));
            }
        }
        for (scope, residual) in &model.residuals {
            if !model.network_bounds.contains_key(scope)
                || !model.is_residual(residual.source_id)
                || !valid_counts([residual.passenger, residual.freight])
            {
                return Err(format!("invalid residual rail source {scope}"));
            }
        }
        for (source, rules) in &model.spatial_priors {
            if !ids.contains(source)
                || rules
                    .iter()
                    .any(|rule| !valid_counts([rule.passenger, rule.freight]) || !rule.when.valid())
            {
                return Err(format!("invalid corridor prior source {source}"));
            }
        }
        let mut regional_countries = std::collections::HashSet::new();
        for region in &model.regional_freight_periods {
            if region
                .countries
                .iter()
                .any(|country| !valid_country(country) || !regional_countries.insert(country))
            {
                return Err("invalid or repeated regional rail period country".into());
            }
        }
        if model
            .mainline_freight
            .iter()
            .any(|(country, count)| !valid_country(country) || !valid_counts([*count, 0.0]))
        {
            return Err("invalid national mainline freight prior".into());
        }
        Ok(model)
    }

    pub(super) fn source(&self, id: u16) -> Option<&SourceRule> {
        self.sources.iter().find(|row| row.id == id)
    }

    pub fn should_overwrite(&self, existing: u16, candidate: u16) -> bool {
        if existing == 0 || existing == candidate {
            return true;
        }
        match (self.source(existing), self.source(candidate)) {
            (Some(a), Some(b)) => (b.rank, b.edition, b.id) > (a.rank, a.edition, a.id),
            _ => false,
        }
    }

    pub fn source_applies_to_row(&self, source: u16, country: [u8; 2]) -> bool {
        self.source(source).is_some_and(|row| {
            row.countries.is_empty() || row.countries.iter().any(|iso| iso.as_bytes() == country)
        })
    }

    pub fn local_source_applies(&self, source: u16, declared: &str, country: [u8; 2]) -> bool {
        declared.as_bytes() == country
            || self.source(source).is_some_and(|row| {
                row.countries.len() > 1 && row.countries.iter().any(|iso| iso.as_bytes() == country)
            })
    }

    pub fn blocks_foreign_national(&self, existing: u16, candidate: u16) -> bool {
        match (self.source(existing), self.source(candidate)) {
            (Some(a), Some(b)) => !a.owner.is_empty() && !b.owner.is_empty() && a.owner != b.owner,
            _ => false,
        }
    }

    pub fn is_residual(&self, id: u16) -> bool {
        self.source(id).is_some_and(|row| row.residual)
    }

    pub fn stamps_whole_line(&self, id: u16) -> bool {
        self.source(id).is_some_and(|row| row.whole_line)
    }

    pub fn period_shares(&self, country: [u8; 2], rail_type: u8) -> PeriodShares {
        if rail_type != 0 {
            return PeriodShares {
                pax: self.urban_periods,
                frt: self.urban_periods,
            };
        }
        let frt = self
            .regional_freight_periods
            .iter()
            .find(|r| r.countries.iter().any(|iso| iso.as_bytes() == country))
            .map_or(self.freight_periods, |r| r.periods);
        PeriodShares {
            pax: self.passenger_periods,
            frt,
        }
    }

    pub fn default_traffic(
        &self,
        rail_type: u8,
        usage: u8,
        country: [u8; 2],
        mode: u8,
    ) -> (f64, f64) {
        let [mut passenger, mut freight] = if rail_type == 0 {
            let mut prior = self.usage_priors[usize::from(usage.min(3))];
            if usage == 0 {
                let iso = std::str::from_utf8(&country).unwrap_or("");
                if let Some(value) = self.mainline_freight.get(iso) {
                    prior[1] = *value;
                }
            }
            prior
        } else {
            self.type_priors[usize::from(rail_type.min(6))]
        };
        if mode == 1 {
            freight = 0.0;
        }
        if mode == 2 {
            passenger = 0.0;
        }
        (passenger, freight)
    }
}

fn valid_country(country: &str) -> bool {
    country.len() == 2 && country.bytes().all(|byte| byte.is_ascii_uppercase())
}

fn valid_counts(counts: [f64; 2]) -> bool {
    counts
        .iter()
        .all(|count| count.is_finite() && *count >= 0.0)
}

//! A year's calendar and public provenance; one admission rule for observations, derivations and practices.
#[path = "year_dates.rs"]
mod dates;
#[path = "year_inputs.rs"]
mod inputs;
#[path = "year_kind.rs"]
mod kind;
use chrono::{Datelike, Days};
use dates::{Interval, date};
pub use inputs::YearInputs;
pub use kind::Kind;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub year: String,
    pub as_of: String,
    pub built: bool,
    pub flows: FlowWindow,
    pub inputs: Vec<Input>,
    pub unavailable: Vec<Unavailable>,
    #[serde(default)]
    pub validity: BTreeMap<String, Interval>,
    #[serde(default)]
    pub shared: BTreeSet<String>,
    #[serde(default)]
    pub rasters: Option<crate::rasters::config::RasterPlan>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowWindow {
    pub start: String,
    pub end: String,
    #[serde(default)]
    pub excluded_days: BTreeSet<String>,
    #[serde(default)]
    pub increment_days: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub id: String,
    pub kind: Kind,
    pub region: String,
    pub publisher: String,
    pub url: String,
    pub observed: Interval,
    pub published: Option<String>,
    pub licence: String,
    #[serde(rename = "public_ok")]
    pub _public_ok: bool,
    pub depends_on: Vec<String>,
    pub description: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unavailable {
    pub id: String,
    pub kind: Kind,
    pub region: String,
    pub description: String,
    pub depends_on: Vec<String>,
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
}

impl Manifest {
    pub fn read(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let manifest: Self =
            serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        manifest
            .validate()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), String> {
        if let Some(rasters) = &self.rasters {
            rasters.validate(self)?;
        }
        if self.year.len() != 4 || !self.year.bytes().all(|b| b.is_ascii_digit()) {
            return Err("year must have four digits".into());
        }
        if self.built && self.inputs.is_empty() {
            return Err("a built year needs its input inventory".into());
        }
        let as_of = date(&self.as_of)?;
        let start = date(&self.flows.start)?;
        let end = date(&self.flows.end)?;
        if start >= end || end > as_of {
            return Err("flows must have start < end <= as_of (end exclusive)".into());
        }
        let regions: BTreeMap<String, String> =
            serde_json::from_str(include_str!("../../years/regions.json"))
                .map_err(|e| e.to_string())?;
        let mut ids = BTreeSet::new();
        for (id, region, description) in self
            .inputs
            .iter()
            .map(|i| (&i.id, &i.region, &i.description))
            .chain(
                self.unavailable
                    .iter()
                    .map(|i| (&i.id, &i.region, &i.description)),
            )
        {
            if !identifier(id) || !ids.insert(id) {
                return Err(format!("invalid or repeated input id {id}"));
            }
            if region.is_empty() || description.trim().is_empty() {
                return Err(format!("{id}: region and description are required"));
            }
            let base = region.split(':').next().unwrap();
            if !regions.contains_key(base) {
                return Err(format!("{id}: unknown About region {region}"));
            }
        }
        for input in &self.inputs {
            input
                .observed
                .bounds()
                .map_err(|e| format!("{}: {e}", input.id))?;
            if let Some(published) = &input.published {
                dates::Interval {
                    start: published.clone(),
                    end: published.clone(),
                }
                .bounds()?;
            }
            if [&input.publisher, &input.url, &input.licence]
                .iter()
                .any(|v| v.trim().is_empty())
            {
                return Err(format!("{}: missing public source metadata", input.id));
            }
        }
        for (id, interval) in &self.validity {
            if !self.inputs.iter().any(|i| &i.id == id) {
                return Err(format!("validity: unknown input {id}"));
            }
            interval.bounds()?;
        }
        if self.shared.iter().any(|id| !ids.contains(id)) {
            return Err("shared: unknown input id".into());
        }
        for (id, dependencies) in self
            .inputs
            .iter()
            .map(|i| (&i.id, &i.depends_on))
            .chain(self.unavailable.iter().map(|i| (&i.id, &i.depends_on)))
        {
            for dependency in dependencies {
                if !ids.contains(dependency) {
                    return Err(format!("{id}: unknown dependency {dependency}"));
                }
            }
            self.admitted(id)?;
        }
        let mut seen = BTreeSet::new();
        for day in self
            .flows
            .excluded_days
            .iter()
            .chain(self.flows.increment_days.iter())
        {
            if !(start..end).contains(&date(day)?) || !seen.insert(day) {
                return Err(format!(
                    "flow sample {day}: repeated, excluded or outside window"
                ));
            }
        }
        if self.window().baseline_days.is_empty() {
            return Err("flow window has no admitted day".into());
        }
        Ok(())
    }

    /// Observed endpoints are inclusive; the map's state is the instant before as_of.
    /// A derivation is admitted only through every dependency, and validity covers that instant.
    pub fn admitted(&self, id: &str) -> Result<bool, String> {
        fn visit(
            manifest: &Manifest,
            id: &str,
            visiting: &mut BTreeSet<String>,
        ) -> Result<bool, String> {
            if !visiting.insert(id.into()) {
                return Err(format!("dependency cycle at {id}"));
            }
            let (dependencies, own) =
                if let Some(input) = manifest.inputs.iter().find(|i| i.id == id) {
                    let cutoff = date(&manifest.as_of)?;
                    let own = if let Some(validity) = manifest.validity.get(id) {
                        let (start, end) = validity.bounds()?;
                        start < cutoff
                            && end >= cutoff.pred_opt().ok_or("as-of has no preceding day")?
                    } else if input.depends_on.is_empty() {
                        input.observed.bounds()?.1 < cutoff
                    } else {
                        true
                    };
                    (&input.depends_on, own)
                } else if let Some(input) = manifest.unavailable.iter().find(|i| i.id == id) {
                    (&input.depends_on, false)
                } else {
                    return Err(format!("unknown dependency {id}"));
                };
            let mut admitted = own;
            for dependency in dependencies {
                admitted &= visit(manifest, dependency, visiting)?;
            }
            visiting.remove(id);
            Ok(admitted)
        }
        visit(self, id, &mut BTreeSet::new())
    }

    /// Solar geometry is an analytic annual cycle, not an observation of future traffic.
    pub fn solar_days(&self) -> Result<Vec<i64>, String> {
        let last = date(&self.as_of)?
            .pred_opt()
            .ok_or("as-of has no preceding day")?;
        let mut day =
            chrono::NaiveDate::from_ymd_opt(last.year(), 1, 1).ok_or("invalid calendar year")?;
        let mut days = Vec::new();
        while day.year() == last.year() {
            days.push(day.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp());
            day = day
                .checked_add_days(Days::new(5))
                .ok_or("calendar overflow")?;
        }
        Ok(days)
    }

    pub fn window(&self) -> crate::boxes::Window {
        let mut day = date(&self.flows.start).expect("validated flow start");
        let end = date(&self.flows.end).expect("validated flow end");
        let mut baseline_days = Vec::new();
        while day < end {
            if !self.flows.excluded_days.contains(&day.to_string()) {
                baseline_days.push(day.to_string());
            }
            day = day.succ_opt().expect("validated flow end");
        }
        crate::boxes::Window {
            baseline_days,
            increment_days: self.flows.increment_days.clone(),
        }
    }
}

#[cfg(test)]
#[path = "year_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "year_storage_tests.rs"]
mod storage_tests;

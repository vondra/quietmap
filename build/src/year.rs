//! A year's admitted inputs and flow window, resolved below one caller-supplied sources root.

use chrono::{Datelike, Days, NaiveDate};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub as_of: String,
    #[serde(default)]
    pub note: String,
    pub flows: FlowWindow,
    pub inputs: Vec<Input>,
    pub unavailable: Vec<Unavailable>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FlowWindow {
    pub start: String,
    pub end: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_days: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub increment_days: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub id: String,
    pub kind: String,
    pub region: String,
    pub publisher: String,
    pub url: String,
    pub vintage: String,
    pub licence: String,
    pub public_ok: bool,
    pub path: PathBuf,
    pub description: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Unavailable {
    pub id: String,
    pub kind: String,
    pub region: String,
    pub reason: String,
}

pub struct YearInputs {
    pub manifest: Manifest,
    paths: BTreeMap<String, Vec<PathBuf>>,
}

fn date(value: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .filter(|date| date.format("%Y-%m-%d").to_string() == value)
        .ok_or_else(|| format!("invalid date {value:?}; expected YYYY-MM-DD"))
}

/// A vintage given as a year describes the whole year, so admission uses its last day.
fn vintage_end(value: &str) -> Result<NaiveDate, String> {
    if value.len() == 4 && value.bytes().all(|byte| byte.is_ascii_digit()) {
        date(&format!("{value}-12-31"))
    } else {
        date(value)
    }
}

impl Manifest {
    pub fn read(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let manifest: Self = serde_json::from_slice(&bytes)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        manifest
            .validate()
            .map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), String> {
        let as_of = date(&self.as_of)?;
        let start = date(&self.flows.start)?;
        let end = date(&self.flows.end)?;
        if start >= end || end > as_of {
            return Err("flows must have start < end <= as_of (end is exclusive)".into());
        }
        let mut ids = HashSet::new();
        for input in &self.inputs {
            if !ids.insert(&input.id) {
                return Err(format!("duplicate input id {:?}", input.id));
            }
            for (field, value) in [
                ("id", &input.id),
                ("kind", &input.kind),
                ("region", &input.region),
                ("publisher", &input.publisher),
                ("url", &input.url),
                ("licence", &input.licence),
                ("description", &input.description),
            ] {
                if value.trim().is_empty() {
                    return Err(format!("input {}: empty {field}", input.id));
                }
            }
            if vintage_end(&input.vintage)? > as_of {
                return Err(format!(
                    "input {} ({}, {}): vintage {} is after as-of {}; use an earlier input or declare it unavailable",
                    input.id, input.kind, input.region, input.vintage, self.as_of
                ));
            }
            if input.path.as_os_str().is_empty()
                || input
                    .path
                    .components()
                    .any(|part| !matches!(part, Component::Normal(_)))
                || input.path.to_string_lossy().contains(['\\', ':'])
            {
                return Err(format!(
                    "input {}: path must be relative to --sources without parent components",
                    input.id
                ));
            }
        }
        for missing in &self.unavailable {
            if missing.id.trim().is_empty()
                || missing.kind.trim().is_empty()
                || missing.region.trim().is_empty()
                || missing.reason.trim().is_empty()
            {
                return Err("an unavailable input needs its id, kind, region and reason".into());
            }
            if !ids.insert(&missing.id) {
                return Err(format!(
                    "unavailable {} in {} repeats an input id",
                    missing.kind, missing.region
                ));
            }
        }
        for days in self
            .flows
            .baseline_days
            .iter()
            .chain(std::iter::once(&self.flows.increment_days))
        {
            let mut seen = HashSet::new();
            for day in days {
                let day_date = date(day)?;
                if !(start..end).contains(&day_date) || !seen.insert(day) {
                    return Err(format!(
                        "flow sample {day}: outside the flow window or repeated"
                    ));
                }
            }
        }
        if self.flows.baseline_days.as_ref().is_some_and(Vec::is_empty) {
            return Err("baseline_days must contain at least one day when specified".into());
        }
        if let Some(baseline) = &self.flows.baseline_days
            && self
                .flows
                .increment_days
                .iter()
                .any(|day| !baseline.contains(day))
        {
            return Err("every increment day must also be a baseline day".into());
        }
        Ok(())
    }

    /// The state's calendar year, sampled every fifth day for the astronomical daily cycle.
    /// At midnight on New Year's Day the state belongs to the year just ended.
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
        let baseline_days = self.flows.baseline_days.clone().unwrap_or_else(|| {
            let mut day = date(&self.flows.start).expect("validated flow start");
            let end = date(&self.flows.end).expect("validated flow end");
            let mut days = Vec::new();
            while day < end {
                days.push(day.to_string());
                day = day
                    .checked_add_days(Days::new(1))
                    .expect("validated flow end");
            }
            days
        });
        crate::boxes::Window {
            baseline_days,
            increment_days: self.flows.increment_days.clone(),
        }
    }
}

impl YearInputs {
    pub fn read(manifest: &Path, sources: &Path) -> Result<Self, String> {
        let manifest = Manifest::read(manifest)?;
        let mut paths: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
        for input in &manifest.inputs {
            paths
                .entry(input.kind.clone())
                .or_default()
                .push(sources.join(&input.path));
        }
        Ok(Self { manifest, paths })
    }

    pub fn paths(&self, kind: &str) -> Result<&[PathBuf], String> {
        match self.paths.get(kind) {
            Some(paths) => Ok(paths),
            None if self
                .manifest
                .unavailable
                .iter()
                .any(|input| input.kind == kind) =>
            {
                Ok(&[])
            }
            None => Err(format!(
                "input kind {kind}: declare its inputs or explain its absence in unavailable"
            )),
        }
    }

    pub fn optional(&self, kind: &str) -> Result<Option<&Path>, String> {
        match self.paths(kind)? {
            [] => Ok(None),
            [path] => Ok(Some(path)),
            _ => Err(format!(
                "input kind {kind}: this builder needs one file, but the manifest names several"
            )),
        }
    }

    pub fn required(&self, kind: &str) -> Result<&Path, String> {
        self.optional(kind)?.ok_or_else(|| {
            format!("no {kind} input in the year manifest; this step cannot run without it")
        })
    }
}

#[cfg(test)]
#[path = "year_tests.rs"]
mod tests;

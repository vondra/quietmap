//! Published monthly operation counts: one class-partitioned input with source scope and rights.

use crate::boxes::Window;
use chrono::{Datelike, NaiveDate};
use physics::doc29::npd::Family;
use physics::doc29::profiles_generated::CLASS_FAMILY;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub urls: Vec<String>,
    pub licence: String,
    pub public_ok: bool,
    pub scope: String,
}

/// Published classes must be disjoint within an airport-month. A broader class uses the observed mixture;
/// it does not assert that the publisher measured a finer split than it actually published.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Class {
    All,
    FixedWing,
    Jet,
    Turboprop,
    Piston,
    Helicopter,
}

impl Class {
    pub fn contains(self, class: u8) -> bool {
        let Some(family) = CLASS_FAMILY.get(usize::from(class)) else {
            return false;
        };
        match self {
            Self::All => true,
            Self::FixedWing => *family != Family::Helicopter,
            Self::Jet => matches!(
                family,
                Family::Narrowbody | Family::Widebody | Family::Regional | Family::Business
            ),
            Self::Turboprop => *family == Family::Turboprop,
            Self::Piston => *family == Family::Piston,
            Self::Helicopter => *family == Family::Helicopter,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Count {
    pub airport: String,
    pub month: String,
    pub class: Class,
    pub movements: u64,
    /// A published arrival count, when the source separates the two operations.
    /// Otherwise only the combined total is known; missing operations restore flow balance.
    pub arrivals: Option<u64>,
    pub source: String,
}

/// The period is explicit: sampled days are not assumed to be the whole calendar window.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operations {
    pub first_month: String,
    pub last_month: String,
    pub sources: BTreeMap<String, Source>,
    pub counts: Vec<Count>,
}

fn month_start(month: &str) -> Result<NaiveDate, String> {
    if month.len() != 7 {
        return Err(format!("invalid operations month {month}"));
    }
    NaiveDate::parse_from_str(&format!("{month}-01"), "%Y-%m-%d")
        .map_err(|error| format!("invalid operations month {month}: {error}"))
}

pub fn month_days(month: &str) -> Result<f64, String> {
    let start = month_start(month)?;
    let end = start
        .checked_add_months(chrono::Months::new(1))
        .ok_or("month out of range")?;
    Ok((end - start).num_days() as f64)
}

impl Operations {
    pub fn read(path: &Path, window: &Window) -> Result<Self, String> {
        let input = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let operations: Self = serde_json::from_slice(&input)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        operations.validate(window)?;
        Ok(operations)
    }

    pub fn calendar_days(&self) -> Result<f64, String> {
        let start = month_start(&self.first_month)?;
        let last = month_start(&self.last_month)?;
        let end = if last.month() == 12 {
            NaiveDate::from_ymd_opt(last.year() + 1, 1, 1)
        } else {
            NaiveDate::from_ymd_opt(last.year(), last.month() + 1, 1)
        }
        .ok_or("operations period out of range")?;
        let days = (end - start).num_days();
        if days <= 0 {
            return Err("operations period is reversed".into());
        }
        Ok(days as f64)
    }

    pub fn validate(&self, window: &Window) -> Result<(), String> {
        self.calendar_days()?;
        let days: Vec<_> = window
            .baseline_days
            .iter()
            .chain(&window.increment_days)
            .collect();
        let months: Vec<_> = days.iter().filter_map(|day| day.get(..7)).collect();
        if months.len() != days.len()
            || months.iter().min().copied() != Some(self.first_month.as_str())
            || months.iter().max().copied() != Some(self.last_month.as_str())
        {
            return Err(
                "operations period must match the sampled window's first and last months".into(),
            );
        }
        for (id, source) in &self.sources {
            if id.is_empty()
                || source.urls.is_empty()
                || source.urls.iter().any(|url| !url.starts_with("https://"))
                || source.licence.trim().is_empty()
                || source.scope.trim().is_empty()
            {
                return Err(format!(
                    "operations source {id} lacks scope, licence or HTTPS URL"
                ));
            }
        }
        let mut occupied = HashSet::new();
        for count in &self.counts {
            month_start(&count.month)?;
            if count.airport.trim().is_empty()
                || count.airport != count.airport.trim()
                || !self.sources.contains_key(&count.source)
                || count
                    .arrivals
                    .is_some_and(|arrivals| arrivals > count.movements)
                || count.month < self.first_month
                || count.month > self.last_month
            {
                return Err(format!(
                    "invalid operations count for {} {}",
                    count.airport, count.month
                ));
            }
            for class in 0..CLASS_FAMILY.len() as u8 {
                if count.class.contains(class)
                    && !occupied.insert((&count.airport, &count.month, class))
                {
                    return Err(format!(
                        "overlapping operations classes for {} {}",
                        count.airport, count.month
                    ));
                }
            }
        }
        Ok(())
    }
}

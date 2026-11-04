//! Integrate service calendars and additions/removals over every day of the manifest flow window.
use crate::railways::coverage::Coverage;
use chrono::{Datelike, NaiveDate};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

#[derive(Debug)]
pub struct ServiceCalendar {
    pub days: Vec<NaiveDate>,
    pub active: HashMap<String, Vec<bool>>,
    service_bounds: HashMap<String, (usize, usize)>,
    allowed: Vec<bool>,
    complete_window: bool,
}

pub fn compact_date(value: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(value, "%Y%m%d")
        .map_err(|e| format!("invalid GTFS date {value}: {e}"))
}

pub fn read_rows(
    path: &Path,
    mut visit: impl FnMut(&BTreeMap<String, String>) -> Result<(), String>,
) -> Result<(), String> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_path(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let headers = reader
        .headers()
        .map_err(|e| e.to_string())?
        .iter()
        .map(|s| s.trim_start_matches('\u{feff}').trim().to_string())
        .collect::<Vec<_>>();
    for row in reader.records() {
        let row = row.map_err(|e| format!("{}: {e}", path.display()))?;
        let fields = headers
            .iter()
            .cloned()
            .zip(row.iter().map(str::to_string))
            .collect();
        visit(&fields).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

pub fn field<'a>(row: &'a BTreeMap<String, String>, name: &str) -> &'a str {
    row.get(name).map(String::as_str).unwrap_or("").trim()
}

impl ServiceCalendar {
    #[cfg(test)]
    pub fn read(directory: &Path, start: NaiveDate, end: NaiveDate) -> Result<Self, String> {
        Self::read_selected(directory, start, end, None, false)
    }

    pub fn read_selected(
        directory: &Path,
        start: NaiveDate,
        end: NaiveDate,
        eligible: Option<&HashSet<String>>,
        ignore_feed_window: bool,
    ) -> Result<Self, String> {
        if start >= end {
            return Err("rail calendar needs a nonempty half-open flow window".into());
        }
        let days: Vec<_> = start.iter_days().take_while(|d| *d < end).collect();
        let mut calendar = Self {
            allowed: vec![true; days.len()],
            service_bounds: HashMap::new(),
            complete_window: false,
            days,
            active: HashMap::new(),
        };
        let path = directory.join("calendar.txt");
        let exceptions = directory.join("calendar_dates.txt");
        if !path.exists() && !exceptions.exists() {
            return Err(
                "timetable has neither calendar.txt nor calendar_dates.txt; exposure is unknown"
                    .into(),
            );
        }
        let weekdays = [
            "monday",
            "tuesday",
            "wednesday",
            "thursday",
            "friday",
            "saturday",
            "sunday",
        ];
        if path.exists() {
            read_rows(&path, |row| {
                let id = field(row, "service_id").to_string();
                if eligible.is_some_and(|ids| !ids.contains(&id)) {
                    return Ok(());
                }
                let first = compact_date(field(row, "start_date"))?;
                let last = compact_date(field(row, "end_date"))?;
                if id.is_empty() || first > last || calendar.active.contains_key(&id) {
                    return Err("empty/repeated service or reversed calendar interval".into());
                }
                for weekday in weekdays {
                    if !["0", "1"].contains(&field(row, weekday)) {
                        return Err(format!("invalid {weekday} flag"));
                    }
                }
                if first < end && last >= start {
                    calendar.service_bounds.insert(
                        id.clone(),
                        (
                            (first - start).num_days().max(0) as usize,
                            (last - start)
                                .num_days()
                                .min(calendar.days.len() as i64 - 1)
                                as usize,
                        ),
                    );
                }
                let mut active = vec![false; calendar.days.len()];
                for (index, day) in calendar.days.iter().enumerate() {
                    if (first..=last).contains(day) {
                        active[index] =
                            field(row, weekdays[day.weekday().num_days_from_monday() as usize])
                                == "1";
                    }
                }
                calendar.active.insert(id, active);
                Ok(())
            })?;
        }
        let mut seen = HashMap::new();
        if exceptions.exists() {
            read_rows(&exceptions, |row| {
                let id = field(row, "service_id").to_string();
                if eligible.is_some_and(|ids| !ids.contains(&id)) {
                    return Ok(());
                }
                let date = compact_date(field(row, "date"))?;
                let added = match field(row, "exception_type") {
                    "1" => true,
                    "2" => false,
                    _ => return Err("invalid calendar exception_type".into()),
                };
                if id.is_empty() {
                    return Err("empty calendar service".into());
                }
                if let Some(previous) = seen.insert((id.clone(), date), added) {
                    if previous != added {
                        return Err("contradictory calendar exceptions".into());
                    }
                    return Ok(());
                }
                let active = calendar
                    .active
                    .entry(id.clone())
                    .or_insert_with(|| vec![false; calendar.days.len()]);
                if let Ok(index) = calendar.days.binary_search(&date) {
                    active[index] = added;
                    if added {
                        let bounds = calendar.service_bounds.entry(id).or_insert((index, index));
                        bounds.0 = bounds.0.min(index);
                        bounds.1 = bounds.1.max(index);
                    }
                }
                Ok(())
            })?;
        }
        let declared = directory.join("feed_info.txt");
        if declared.exists() && !ignore_feed_window {
            let mut records = 0;
            read_rows(&declared, |row| {
                records += 1;
                if records > 1 {
                    return Err("GTFS feed_info must have at most one row".into());
                }
                let parse = |key| {
                    if field(row, key).is_empty() {
                        Ok(None)
                    } else {
                        compact_date(field(row, key)).map(Some)
                    }
                };
                let first = parse("feed_start_date")?;
                let last = parse("feed_end_date")?;
                if first.zip(last).is_some_and(|(a, b)| a > b) {
                    return Err("reversed feed window".into());
                }
                for (index, day) in calendar.days.iter().enumerate() {
                    let within = first.is_none_or(|date| *day >= date)
                        && last.is_none_or(|date| *day <= date);
                    calendar.complete_window |= within && first.is_some() && last.is_some();
                    // GTFS feed_info explicitly asserts no service for omitted dates inside its complete window.
                    calendar.allowed[index] = within;
                    if !within {
                        for active in calendar.active.values_mut() {
                            active[index] = false;
                        }
                    }
                }
                Ok(())
            })?;
        }
        // A future-only path cannot assert silence on a line absent from this window.
        // A complete feed_info window is the publisher's explicit exception to that rule.
        calendar
            .active
            .retain(|id, _| calendar.complete_window || calendar.service_bounds.contains_key(id));
        Ok(calendar)
    }

    pub fn coverage_for<'a>(
        &self,
        services: impl Iterator<Item = &'a str>,
    ) -> Result<Coverage, String> {
        let bounds = services
            .filter_map(|service| self.service_bounds.get(service))
            .fold(None, |range: Option<(usize, usize)>, &(first, last)| {
                Some(range.map_or((first, last), |(a, b)| (a.min(first), b.max(last))))
            });
        let covered = self
            .allowed
            .iter()
            .enumerate()
            .map(|(index, allowed)| {
                *allowed
                    && (self.complete_window
                        || bounds.is_some_and(|(first, last)| (first..=last).contains(&index)))
            })
            .collect::<Vec<_>>();
        Coverage::from_days(&covered)
    }

    pub fn active_days(&self, service: &str) -> Result<usize, String> {
        self.active
            .get(service)
            .map(|days| days.iter().filter(|v| **v).count())
            .ok_or_else(|| format!("trip references undefined calendar service {service}"))
    }

    pub fn annual_departures(
        &self,
        service: &str,
        departures_per_service_day: f64,
    ) -> Result<f64, String> {
        if !departures_per_service_day.is_finite() || departures_per_service_day < 0.0 {
            return Err("departures must be finite and nonnegative".into());
        }
        Ok(self.active_days(service)? as f64 * departures_per_service_day / self.days.len() as f64)
    }
}

#[cfg(test)]
#[path = "calendar_tests.rs"]
mod tests;

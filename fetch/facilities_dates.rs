//! Normalize explicit commissioning and retirement dates without dating unknown operation by a filename.

use chrono::{Datelike, NaiveDate};

pub fn published_date(value: &str) -> Option<String> {
    let value = value.trim();
    if value.len() == 4 && value.bytes().all(|b| b.is_ascii_digit()) {
        let year = value.parse().ok()?;
        return NaiveDate::from_ymd_opt(year, 1, 1).map(|_| value.to_owned());
    }
    if let Ok(date) = NaiveDate::parse_from_str(value.get(..10).unwrap_or(value), "%Y-%m-%d") {
        return Some(date.to_string());
    }
    if let Ok(milliseconds) = value.parse::<i64>()
        && milliseconds.unsigned_abs() > 10_000_000_000
    {
        return chrono::DateTime::from_timestamp_millis(milliseconds)
            .map(|d| d.date_naive().to_string());
    }
    None
}

pub fn dated_history(value: &str, observed: &str) -> Option<String> {
    published_date(value).filter(|date| date.as_str() <= observed)
}

pub fn observation_date(input: &crate::year::Input) -> anyhow::Result<String> {
    let (_, end) = input.observed.bounds().map_err(anyhow::Error::msg)?;
    Ok(end.to_string())
}

pub fn report_date(properties: &serde_json::Value, fallback: &str) -> String {
    properties["reportingYear"]
        .as_i64()
        .and_then(|year| NaiveDate::from_ymd_opt(year as i32, 12, 31))
        .filter(|date| date.year() > 0)
        .map_or_else(|| fallback.to_string(), |date| date.to_string())
}

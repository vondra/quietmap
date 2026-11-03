//! Explicit observation intervals; publication dates never decide admission.
use chrono::{Datelike, NaiveDate};
use serde::Deserialize;

pub fn date(value: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .filter(|day| day.format("%Y-%m-%d").to_string() == value)
        .ok_or_else(|| format!("invalid date {value:?}; expected YYYY-MM-DD"))
}

/// Year and month precision conservatively includes their entire period.
fn boundary(value: &str, end: bool) -> Result<NaiveDate, String> {
    match value.len() {
        4 => date(&format!("{value}-{}", if end { "12-31" } else { "01-01" })),
        7 => {
            let first = date(&format!("{value}-01"))?;
            if !end {
                return Ok(first);
            }
            let (year, month) = if first.month() == 12 {
                (first.year() + 1, 1)
            } else {
                (first.year(), first.month() + 1)
            };
            NaiveDate::from_ymd_opt(year, month, 1)
                .and_then(|day| day.pred_opt())
                .ok_or("date overflow".into())
        }
        _ => date(value),
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Interval {
    pub start: String,
    pub end: String,
}

impl Interval {
    pub fn bounds(&self) -> Result<(NaiveDate, NaiveDate), String> {
        let start = boundary(&self.start, false)?;
        let end = boundary(&self.end, true)?;
        if start > end {
            return Err("observation start is after end".into());
        }
        Ok((start, end))
    }
}

/// Dates embedded in a citation are an audit signal, never an admission rule. A later
/// publication or archive capture must be documented separately from the observation.
#[cfg(test)]
pub fn citation_dates(value: &str) -> Vec<NaiveDate> {
    let bytes = value.as_bytes();
    let mut dates = Vec::new();
    for i in 0..bytes.len() {
        if i > 0 && bytes[i - 1].is_ascii_digit() {
            continue;
        }
        for (length, separated) in [(10, true), (8, false)] {
            let Some(part) = bytes.get(i..i + length) else {
                continue;
            };
            let text = String::from_utf8_lossy(part);
            let text = if separated {
                text.replace(['.', '_'], "-")
            } else {
                if !part.iter().all(u8::is_ascii_digit) {
                    continue;
                }
                format!("{}-{}-{}", &text[..4], &text[4..6], &text[6..8])
            };
            if let Ok(day) = date(&text)
                && (1900..2100).contains(&day.year())
            {
                dates.push(day);
                break;
            }
        }
    }
    dates
}

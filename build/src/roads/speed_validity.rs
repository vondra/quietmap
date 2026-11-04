//! Select documented legal/default speed records valid at the manifest's map-state cutoff.

use crate::year::{Kind, YearInputs};
use anyhow::{Result, anyhow, ensure};
use chrono::NaiveDate;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct History {
    rows: Vec<Record>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    country: String,
    value: Value,
    valid_from: String,
    valid_until: Option<String>,
    source_url: String,
    scope: String,
}

fn selected(records: Vec<Record>, date: NaiveDate, kind: Kind) -> Result<Value> {
    let mut rows = BTreeMap::new();
    for row in records {
        let from = row.valid_from.parse::<NaiveDate>()?;
        let until = row
            .valid_until
            .as_deref()
            .map(str::parse::<NaiveDate>)
            .transpose()?;
        ensure!(
            until.is_none_or(|until| until > from),
            "empty speed validity interval"
        );
        if from > date || until.is_some_and(|until| date >= until) {
            continue;
        }
        ensure!(
            row.country.len() == 2
                && row.country.bytes().all(|c| c.is_ascii_uppercase())
                && row.source_url.starts_with("https://")
                && !row.scope.is_empty(),
            "speed history needs its country and documented scope"
        );
        let valid = match kind {
            Kind::LegalSpeeds => row.value.as_array().is_some_and(|v| {
                v.len() == 4 && v.iter().all(|n| n.as_u64().is_some_and(|n| n <= 255))
            }),
            Kind::HeavySpeeds => row
                .value
                .as_f64()
                .is_some_and(|v| v.is_finite() && v > 0.0 && v <= 200.0),
            _ => false,
        };
        ensure!(valid, "invalid documented speed value");
        ensure!(
            rows.insert(row.country, row.value).is_none(),
            "overlapping speed rules for one country"
        );
    }
    ensure!(
        !rows.is_empty(),
        "no documented speed rules valid at map-state cutoff"
    );
    Ok(
        json!({"rows":rows.into_iter().map(|(country,value)|json!({"country":country,"value":value})).collect::<Vec<_>>()}),
    )
}

pub fn build(inputs: &YearInputs, kind: &str, output: &Path) -> Result<()> {
    let kind = match kind {
        "legal-speeds" => Kind::LegalSpeeds,
        "heavy-speeds" => Kind::HeavySpeeds,
        _ => return Err(anyhow!("expected legal-speeds or heavy-speeds")),
    };
    let outputs = inputs
        .manifest
        .inputs
        .iter()
        .filter(|r| r.kind == kind)
        .collect::<Vec<_>>();
    ensure!(
        outputs.len() == 1,
        "speed table needs one declared output input"
    );
    let mut rows = Vec::new();
    for id in &outputs[0].depends_on {
        let input = inputs
            .manifest
            .inputs
            .iter()
            .find(|r| r.id == *id)
            .ok_or_else(|| anyhow!("missing speed history dependency"))?;
        ensure!(
            input.kind == Kind::ModelObservations,
            "speed history must be an observed policy input"
        );
        let history: History = serde_json::from_slice(&std::fs::read(
            inputs.by_id(id).map_err(anyhow::Error::msg)?,
        )?)?;
        rows.extend(history.rows);
    }
    let date = inputs
        .manifest
        .as_of
        .parse::<NaiveDate>()?
        .pred_opt()
        .ok_or_else(|| anyhow!("invalid map-state cutoff"))?;
    let result = selected(rows, date, kind)?;
    let mut bytes = serde_json::to_vec_pretty(&result)?;
    bytes.push(b'\n');
    std::fs::write(output, bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_future_law_never_changes_the_historical_cap() {
        let history = |from: &str, until: Option<&str>, speed: f64| Record {
            country: "JP".into(),
            value: json!(speed),
            valid_from: from.into(),
            valid_until: until.map(str::to_owned),
            source_url: "https://example.org/law".into(),
            scope: "fixture".into(),
        };
        let rows = vec![
            history("2000-01-01", Some("2001-04-01"), 80.0),
            history("2001-04-01", None, 90.0),
        ];
        assert_eq!(
            selected(rows, "2000-12-31".parse().unwrap(), Kind::HeavySpeeds).unwrap()["rows"][0]["value"],
            80.0
        );
        let rows = vec![history("2001-04-01", None, 90.0)];
        assert_eq!(
            selected(rows, "2001-04-01".parse().unwrap(), Kind::HeavySpeeds).unwrap()["rows"][0]["value"],
            90.0
        );
    }
}

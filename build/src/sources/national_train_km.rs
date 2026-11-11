//! A country's published train-km a day by train type, from Eurostat's train movements table.
use anyhow::{Context, Result, anyhow, ensure};
use chrono::{Datelike, NaiveDate};
use serde_json::Value;
use std::collections::BTreeMap;

/// A country's published train-km a day by train type: (year, passenger), (year, goods).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct NationalTrainKm {
    pub passenger: Option<(i32, f64)>,
    pub freight: Option<(i32, f64)>,
}

/// Eurostat's national train-km (JSON-stat 2.0, thousand train-km by train type, country and year)
/// a day: for each country and type, the latest year within `years`. Eurostat writes Greece EL and
/// the United Kingdom UK; its aggregates (EU27_2020 ...) are not countries.
pub(super) fn from_eurostat(
    dataset: &Value,
    years: std::ops::RangeInclusive<i32>,
) -> Result<BTreeMap<[u8; 2], NationalTrainKm>> {
    let ids: Vec<&str> = dataset["id"]
        .as_array()
        .context("JSON-stat id")?
        .iter()
        .map(|id| id.as_str().context("JSON-stat id"))
        .collect::<Result<_>>()?;
    let sizes: Vec<usize> = dataset["size"]
        .as_array()
        .context("JSON-stat size")?
        .iter()
        .map(|size| {
            size.as_u64()
                .map(|size| size as usize)
                .context("JSON-stat size")
        })
        .collect::<Result<_>>()?;
    ensure!(ids.len() == sizes.len(), "JSON-stat id and size differ");
    let categories: Vec<BTreeMap<&str, usize>> = ids
        .iter()
        .map(|id| {
            let index = dataset["dimension"][id]["category"]["index"]
                .as_object()
                .with_context(|| format!("JSON-stat dimension {id}"))?;
            index
                .iter()
                .map(|(code, position)| {
                    Ok((
                        code.as_str(),
                        position.as_u64().context("category index")? as usize,
                    ))
                })
                .collect()
        })
        .collect::<Result<_>>()?;
    // Every dimension but country, year and train type takes its total (one unit, one frequency).
    let mut fixed = vec![None; ids.len()];
    for (dimension, (id, codes)) in ids.iter().zip(&categories).enumerate() {
        fixed[dimension] = match *id {
            "geo" | "time" | "train" => continue,
            "unit" => codes.get("THS_TRKM"),
            _ if codes.len() == 1 => codes.values().next(),
            _ => codes.get("TOTAL"),
        }
        .copied()
        .map(Some)
        .with_context(|| format!("JSON-stat dimension {id}: no total"))?;
    }
    let position = |name: &str| {
        ids.iter()
            .position(|id| *id == name)
            .ok_or_else(|| anyhow!("JSON-stat has no {name} dimension"))
    };
    let (geo, time, train) = (position("geo")?, position("time")?, position("train")?);
    let values = dataset["value"].as_object().context("JSON-stat value")?;
    let mut countries = BTreeMap::<[u8; 2], NationalTrainKm>::new();
    for (code, &geo_index) in &categories[geo] {
        let country = match *code {
            "EL" => *b"GR",
            "UK" => *b"GB",
            code if code.len() == 2 && code.bytes().all(|b| b.is_ascii_uppercase()) => {
                [code.as_bytes()[0], code.as_bytes()[1]]
            }
            _ => continue,
        };
        for (category, slot) in ["TRN_PAS", "TRN_GD"].into_iter().enumerate() {
            let train_index = *categories[train]
                .get(slot)
                .with_context(|| format!("JSON-stat train has no {slot}"))?;
            for (year, &time_index) in &categories[time] {
                let year: i32 = year.parse().context("JSON-stat year")?;
                if !years.contains(&year) {
                    continue;
                }
                let mut linear = 0;
                for (dimension, &size) in sizes.iter().enumerate() {
                    let index = match dimension {
                        d if d == geo => geo_index,
                        d if d == time => time_index,
                        d if d == train => train_index,
                        d => fixed[d].expect("fixed dimension"),
                    };
                    linear = linear * size + index;
                }
                let Some(thousands) = values.get(&linear.to_string()).and_then(Value::as_f64)
                else {
                    continue;
                };
                let days = NaiveDate::from_ymd_opt(year, 12, 31)
                    .context("JSON-stat year")?
                    .ordinal();
                let entry = countries.entry(country).or_default();
                let slot = if category == 0 {
                    &mut entry.passenger
                } else {
                    &mut entry.freight
                };
                if slot.is_none_or(|(latest, _)| latest < year) {
                    *slot = Some((year, thousands * 1000.0 / f64::from(days)));
                }
            }
        }
    }
    Ok(countries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The latest year within the interval, a day of it; Eurostat's EL and UK, no aggregates.
    #[test]
    fn national_train_km_reads_eurostat_json_stat() {
        let index = |codes: &[&str]| {
            serde_json::Value::Object(
                codes
                    .iter()
                    .enumerate()
                    .map(|(i, code)| (code.to_string(), json!(i)))
                    .collect(),
            )
        };
        let (trains, geos, years) = (
            ["TOTAL", "TRN_GD", "TRN_PAS"],
            ["CZ", "EL", "EU27_2020", "UK"],
            ["2019", "2023", "2024", "2025"],
        );
        let mut values = serde_json::Map::new();
        let mut put = |train: usize, geo: usize, year: usize, value: f64| {
            values.insert(((train * 4 + geo) * 4 + year).to_string(), json!(value));
        };
        put(2, 0, 2, 141_622.0);
        put(2, 0, 3, 142_002.0);
        put(1, 0, 1, 29_898.0);
        put(2, 1, 0, 9_531.0);
        put(2, 1, 1, 6_090.0);
        put(1, 2, 2, 1.0);
        put(1, 3, 0, 33_941.0);
        put(0, 3, 0, 594_460.0);
        let dataset = json!({
            "id": ["freq", "unit", "train", "geo", "time"],
            "size": [1, 1, 3, 4, 4],
            "dimension": {
                "freq": {"category": {"index": index(&["A"])}},
                "unit": {"category": {"index": index(&["THS_TRKM"])}},
                "train": {"category": {"index": index(&trains)}},
                "geo": {"category": {"index": index(&geos)}},
                "time": {"category": {"index": index(&years)}},
            },
            "value": values,
        });
        let national = from_eurostat(&dataset, 2019..=2024).unwrap();
        assert_eq!(national.len(), 3);
        assert_eq!(
            national[b"CZ"].passenger,
            Some((2024, 141_622_000.0 / 366.0))
        );
        assert_eq!(national[b"CZ"].freight, Some((2023, 29_898_000.0 / 365.0)));
        assert_eq!(national[b"GR"].passenger, Some((2023, 6_090_000.0 / 365.0)));
        assert_eq!(national[b"GR"].freight, None);
        assert_eq!(national[b"GB"].freight, Some((2019, 33_941_000.0 / 365.0)));
        assert_eq!(national[b"GB"].passenger, None);
    }
}

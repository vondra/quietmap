//! Normalize line geometry and declared publisher fields without embedding national traffic models.
use crate::railways::{
    config::RailObject,
    movements::{Location, Movement},
};
use chrono::{DateTime, Datelike, NaiveDate};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

fn text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.trim().to_string(),
        Some(Value::Number(value)) => value.to_string(),
        _ => String::new(),
    }
}

fn parse_date_bounds(value: Option<&Value>) -> Option<(NaiveDate, NaiveDate)> {
    match value? {
        Value::Null => None,
        Value::Number(number) => {
            if let Some(year) = number.as_i64().filter(|y| (1900..=2100).contains(y)) {
                let year = year as i32;
                return Some((
                    NaiveDate::from_ymd_opt(year, 1, 1)?,
                    NaiveDate::from_ymd_opt(year, 12, 31)?,
                ));
            }
            let ms = number
                .as_i64()
                .or_else(|| number.as_f64().map(|v| v as i64))?;
            let day = DateTime::from_timestamp_millis(ms)?.date_naive();
            (1800..=2100).contains(&day.year()).then_some((day, day))
        }
        Value::String(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return None;
            }
            let head = trimmed.split(['T', ' ']).next().unwrap_or(trimmed);
            let normalized = head.replace('/', "-");
            if let Ok(day) = NaiveDate::parse_from_str(&normalized, "%Y-%m-%d") {
                return (1800..=2100).contains(&day.year()).then_some((day, day));
            }
            if normalized.len() == 6 {
                let bytes = normalized.as_bytes();
                if matches!(bytes[4], b'Q' | b'q')
                    && bytes[..4].iter().all(u8::is_ascii_digit)
                    && matches!(bytes[5], b'1'..=b'4')
                {
                    let year: i32 = normalized[..4].parse().ok()?;
                    let (start_m, end_m, end_d) = match bytes[5] {
                        b'1' => (1, 3, 31),
                        b'2' => (4, 6, 30),
                        b'3' => (7, 9, 30),
                        b'4' => (10, 12, 31),
                        _ => unreachable!(),
                    };
                    return Some((
                        NaiveDate::from_ymd_opt(year, start_m, 1)?,
                        NaiveDate::from_ymd_opt(year, end_m, end_d)?,
                    ));
                }
            }
            if normalized.len() == 7 && normalized.as_bytes()[4] == b'-' {
                let year: i32 = normalized[..4].parse().ok()?;
                let month: u32 = normalized[5..7].parse().ok()?;
                let first = NaiveDate::from_ymd_opt(year, month, 1)?;
                let next = if month == 12 {
                    NaiveDate::from_ymd_opt(year + 1, 1, 1)?
                } else {
                    NaiveDate::from_ymd_opt(year, month + 1, 1)?
                };
                return Some((first, next.pred_opt()?));
            }
            if normalized.len() == 4 && normalized.bytes().all(|b| b.is_ascii_digit()) {
                let year: i32 = normalized.parse().ok()?;
                if (1800..=2100).contains(&year) {
                    return Some((
                        NaiveDate::from_ymd_opt(year, 1, 1)?,
                        NaiveDate::from_ymd_opt(year, 12, 31)?,
                    ));
                }
            }
            None
        }
        _ => None,
    }
}

fn first_date(properties: &Value, fields: &[String]) -> Option<(NaiveDate, NaiveDate)> {
    fields
        .iter()
        .find_map(|field| parse_date_bounds(properties.get(field)))
}

pub fn convert(
    path: &Path,
    config: &RailObject,
    cutoff: NaiveDate,
) -> Result<Vec<Movement>, String> {
    let RailObject::Geojson {
        input,
        source_id,
        country,
        category,
        rail_types,
        radius_m,
        fields,
        accepted,
        excluded,
        start_fields,
        end_fields,
    } = config
    else {
        unreachable!()
    };
    let collection: Value =
        serde_json::from_slice(&std::fs::read(path).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    let features = collection["features"]
        .as_array()
        .ok_or("rail line input is not a GeoJSON FeatureCollection")?;
    let mut output = Vec::new();
    let mut usable_lines_before_dates = 0usize;
    for (index, feature) in features.iter().enumerate() {
        let properties = &feature["properties"];
        if accepted
            .iter()
            .any(|(field, values)| !values.contains(&text(properties.get(field))))
            || excluded
                .iter()
                .any(|(field, values)| values.contains(&text(properties.get(field))))
        {
            continue;
        }
        let geometry = &feature["geometry"];
        let lines = match geometry["type"].as_str() {
            Some("LineString") => vec![&geometry["coordinates"]],
            Some("MultiLineString") => geometry["coordinates"]
                .as_array()
                .ok_or("invalid MultiLineString")?
                .iter()
                .collect(),
            _ => continue,
        };
        let mut parsed_lines = Vec::new();
        for (part, line) in lines.into_iter().enumerate() {
            let points = line.as_array().ok_or("invalid source railway line")?;
            if points.len() < 2 {
                continue;
            }
            let coordinates = points
                .iter()
                .map(|point| {
                    Ok([
                        point[1].as_f64().ok_or("invalid railway latitude")?,
                        point[0].as_f64().ok_or("invalid railway longitude")?,
                    ])
                })
                .collect::<Result<Vec<_>, String>>()?;
            parsed_lines.push((part, coordinates));
        }
        if parsed_lines.is_empty() {
            continue;
        }
        usable_lines_before_dates += parsed_lines.len();
        if first_date(properties, start_fields).is_some_and(|(first, _)| first >= cutoff)
            || first_date(properties, end_fields).is_some_and(|(_, last)| last < cutoff)
        {
            continue;
        }
        let mut attributes: BTreeMap<_, _> = fields
            .iter()
            .map(|(key, field)| (key.clone(), text(properties.get(field))))
            .collect();
        attributes.insert("kind".into(), category.clone());
        for (part, coordinates) in parsed_lines {
            let movement = Movement {
                id: format!("{input}/{index}/{part}"),
                source_id: *source_id,
                country: country.clone(),
                passenger: None,
                freight: None,
                passenger_status: 0,
                freight_status: 0,
                matching: 0,
                rail_types: rail_types.clone(),
                coverage: Default::default(),
                missing_calendar_estimate: [0.0; 2],
                attributes: attributes.clone(),
                location: Location::Line {
                    coordinates,
                    radius_m: *radius_m,
                },
            };
            movement.validate()?;
            output.push(movement);
        }
    }
    if usable_lines_before_dates == 0 {
        return Err(format!("{input}: no accepted railway lines"));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn spatial_rail_dates_respect_field_precedence_quarters_closures_and_empty_cutoffs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lines.geojson");
        let geom = json!({"type": "LineString", "coordinates": [[116.3, 39.9], [116.4, 39.9]]});
        let feature =
            |props: Value| json!({"type": "Feature", "properties": props, "geometry": geom});
        // SegmentOpenDate 2021-12-26 beats later OpenDate 2025-12-26; 2024Q4 kept; 2025-02-12 dropped;
        // CloseDate 2024-10-28 dropped; CloseDate 2025-12-20 kept; corrupt OpenSeason ignored when earlier field parses.
        let epoch_2021_12_26 = NaiveDate::from_ymd_opt(2021, 12, 26)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        let collection = json!({"type": "FeatureCollection", "features": [
            feature(json!({"Status": "ok", "SegmentOpenDate": epoch_2021_12_26, "OpenDate": "2025-12-26", "OpenSeason": "d/9BmH"})),
            feature(json!({"Status": "ok", "OpenSeason": "2024Q4"})),
            feature(json!({"Status": "ok", "OpenDate": "2025-02-12", "OpenSeason": "2025Q1"})),
            feature(json!({"Status": "ok", "OpenDate": "2010-01-01", "CloseDate": "2024-10-28"})),
            feature(json!({"Status": "ok", "OpenDate": "2010-01-01", "CloseDate": "2025-12-20"})),
        ]});
        std::fs::write(&path, serde_json::to_vec(&collection).unwrap()).unwrap();
        let object = RailObject::Geojson {
            input: "cn-metro".into(),
            source_id: 2079,
            country: "CN".into(),
            category: "metro".into(),
            rail_types: vec![2],
            radius_m: 120.0,
            fields: BTreeMap::new(),
            accepted: BTreeMap::from([("Status".into(), vec!["ok".into()])]),
            excluded: BTreeMap::new(),
            start_fields: vec![
                "SegmentOpenDate".into(),
                "OpenDate".into(),
                "OpenSeason".into(),
            ],
            end_fields: vec!["CloseDate".into()],
        };
        let rows = convert(&path, &object, NaiveDate::from_ymd_opt(2025, 1, 1).unwrap()).unwrap();
        let ids: Vec<_> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["cn-metro/0/0", "cn-metro/1/0", "cn-metro/4/0"]);

        // Date filtering that removes every accepted line succeeds with an empty result.
        let empty = convert(&path, &object, NaiveDate::from_ymd_opt(1990, 1, 1).unwrap()).unwrap();
        assert!(empty.is_empty());

        // A feed containing only points or degenerate one-point lines still fails as unusable.
        let broken_path = dir.path().join("broken.geojson");
        let broken = json!({"type": "FeatureCollection", "features": [
            {"type": "Feature", "properties": {"Status": "ok", "OpenDate": "2020-01-01"},
             "geometry": {"type": "Point", "coordinates": [116.3, 39.9]}},
            {"type": "Feature", "properties": {"Status": "ok", "OpenDate": "2020-01-01"},
             "geometry": {"type": "LineString", "coordinates": [[116.3, 39.9]]}},
        ]});
        std::fs::write(&broken_path, serde_json::to_vec(&broken).unwrap()).unwrap();
        assert!(
            convert(
                &broken_path,
                &object,
                NaiveDate::from_ymd_opt(2025, 1, 1).unwrap()
            )
            .is_err()
        );
    }
}

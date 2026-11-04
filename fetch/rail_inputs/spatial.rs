//! Normalize line geometry and declared publisher fields without embedding national traffic models.
use crate::railways::{
    config::RailObject,
    movements::{Location, Movement},
};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

fn text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.trim().to_string(),
        Some(Value::Number(value)) => value.to_string(),
        _ => String::new(),
    }
}

pub fn convert(path: &Path, config: &RailObject) -> Result<Vec<Movement>, String> {
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
        let mut attributes: BTreeMap<_, _> = fields
            .iter()
            .map(|(key, field)| (key.clone(), text(properties.get(field))))
            .collect();
        attributes.insert("kind".into(), category.clone());
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
    if output.is_empty() {
        return Err(format!("{input}: no accepted railway lines"));
    }
    Ok(output)
}

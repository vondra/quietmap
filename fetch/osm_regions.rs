//! Convert manifest-selected CGAZ boundaries and ISO identities to the common country-polygon input.

use crate::osm::region_input::{RegionFile, RegionInput};
use anyhow::{Result, anyhow, ensure};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::Path,
};

#[derive(Deserialize)]
struct CountryIdentities {
    rows: Vec<CountryIdentity>,
}

#[derive(Deserialize)]
struct CountryIdentity {
    group: String,
    country: String,
    continent: u8,
}

pub fn convert(inputs: &crate::year::YearInputs, output: &Path) -> Result<()> {
    let boundaries = inputs
        .required("prepare-boundaries")
        .map_err(|error| anyhow!(error))?;
    let identities = inputs
        .required("prepare-geography")
        .map_err(|error| anyhow!(error))?;
    let source: serde_json::Value = serde_json::from_slice(&std::fs::read(boundaries)?)?;
    let identities: CountryIdentities = serde_json::from_slice(&std::fs::read(identities)?)?;
    let converted = convert_features(&source, &identities)?;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    let mut writer = std::io::BufWriter::new(file);
    serde_json::to_writer(&mut writer, &converted)?;
    writer.flush()?;
    Ok(())
}

fn convert_features(
    source: &serde_json::Value,
    identities: &CountryIdentities,
) -> Result<RegionFile> {
    let countries: BTreeMap<_, _> = identities
        .rows
        .iter()
        .map(|row| (row.group.as_str(), row))
        .collect();
    ensure!(
        countries.len() == identities.rows.len(),
        "duplicate CGAZ identity"
    );
    let mut regions = Vec::new();
    let mut groups = BTreeSet::new();
    for feature in source["features"]
        .as_array()
        .ok_or_else(|| anyhow!("missing boundary features"))?
    {
        let group = feature["properties"]["shapeGroup"]
            .as_str()
            .ok_or_else(|| anyhow!("missing CGAZ shapeGroup"))?;
        let disputed = group.bytes().all(|byte| byte.is_ascii_digit());
        ensure!(
            groups.insert(group),
            "duplicate CGAZ boundary group {group}"
        );
        let identity = countries.get(group);
        ensure!(
            identity.is_some() || disputed,
            "unmapped CGAZ country {group}"
        );
        let geometry = &feature["geometry"];
        let polygons = match geometry["type"].as_str() {
            Some("Polygon") => vec![serde_json::from_value(geometry["coordinates"].clone())?],
            Some("MultiPolygon") => serde_json::from_value(geometry["coordinates"].clone())?,
            other => return Err(anyhow!("unsupported CGAZ geometry {other:?}")),
        };
        regions.push(RegionInput {
            country: identity.map(|row| row.country.clone()),
            continent: identity.map_or(0, |row| row.continent),
            disputed,
            polygons,
        });
    }
    ensure!(!regions.is_empty(), "empty country input");
    Ok(RegionFile { regions })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cgaz_identity_and_disputed_land_survive_conversion() {
        let identities = CountryIdentities {
            rows: vec![CountryIdentity {
                group: "ABC".into(),
                country: "XY".into(),
                continent: 1,
            }],
        };
        let mut source = serde_json::json!({"features":[{"properties":{"shapeGroup":"ABC"},"geometry":{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,0]]]}}]});
        let converted = convert_features(&source, &identities).unwrap();
        assert_eq!(converted.regions[0].country.as_deref(), Some("XY"));
        let mut repeated = source.clone();
        repeated["features"]
            .as_array_mut()
            .unwrap()
            .push(source["features"][0].clone());
        assert!(convert_features(&repeated, &identities).is_err());
        source["features"][0]["properties"]["shapeGroup"] = "999".into();
        let disputed = convert_features(&source, &identities).unwrap();
        assert!(disputed.regions[0].disputed);
        assert!(disputed.regions[0].country.is_none());
        source["features"][0]["properties"]["shapeGroup"] = "MISSING".into();
        assert!(convert_features(&source, &identities).is_err());
    }
}

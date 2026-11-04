//! Fit country/class posted-speed medians from the admitted OSM state and explicitly mapped road environments.

use crate::{
    osm::{classify, geography::Regions, junctions::NodeIdSet, node_cache::NodeCache},
    year::{Kind, YearInputs},
};
use anyhow::{Result, anyhow, ensure};
use osmpbf::{Element, ElementReader};
use rayon::prelude::*;
use serde_json::json;
use std::{collections::BTreeMap, path::Path};

struct Sample {
    node: i64,
    class: u8,
    speed: u8,
    environment: usize,
}

fn signed_environment<'a>(tag: impl Fn(&str) -> Option<&'a str>) -> Option<usize> {
    for key in ["maxspeed:type", "source:maxspeed", "zone:traffic"] {
        if let Some(value) = tag(key) {
            let value = value.rsplit(':').next().unwrap_or(value);
            if value == "rural" {
                return Some(0);
            }
            if value == "urban" {
                return Some(1);
            }
        }
    }
    match tag("urban") {
        Some("yes") => Some(1),
        Some("no") => Some(0),
        _ => None,
    }
}

pub fn fit(inputs: &YearInputs, output: &Path, scratch: &Path) -> Result<()> {
    ensure!(
        !output.exists() && !scratch.exists(),
        "speed calibration needs new output and scratch paths"
    );
    let pbf = inputs
        .required(Kind::PrepareOsm)
        .map_err(anyhow::Error::msg)?;
    let regions = Regions::read(
        inputs
            .required(Kind::PrepareRegions)
            .map_err(anyhow::Error::msg)?,
    )?;
    let needed = NodeIdSet::new()?;
    eprintln!("tagged-speed-fit: reading explicitly signed road environments");
    let samples = ElementReader::from_path(pbf)?.par_map_reduce(
        |element| {
            let Element::Way(way) = element else {
                return Vec::new();
            };
            let tags = way.tags().collect::<Vec<_>>();
            let tag = |key: &str| tags.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
            let Some(highway) = tag("highway") else {
                return Vec::new();
            };
            let class = classify::road_class(highway);
            if !(2..=9).contains(&class)
                || !matches!(
                    classify::classify_primary_way(&way),
                    Some(classify::FeatureType::Road)
                )
            {
                return Vec::new();
            }
            let Some(environment) = signed_environment(tag) else {
                return Vec::new();
            };
            let speed = classify::parse_maxspeed_kmh(tag("maxspeed").unwrap_or(""));
            if speed == 0 || speed >= 255 {
                return Vec::new();
            }
            let refs = way.refs().collect::<Vec<_>>();
            let Some(&node) = refs.get(refs.len() / 2) else {
                return Vec::new();
            };
            needed.insert(node);
            vec![Sample {
                node,
                class,
                speed: speed as u8,
                environment,
            }]
        },
        Vec::new,
        |mut left, mut right| {
            left.append(&mut right);
            left
        },
    )?;
    ensure!(!samples.is_empty(), "no located signed speed observations");
    eprintln!(
        "tagged-speed-fit: {} mapped ways, {} representative nodes",
        samples.len(),
        needed.count()
    );
    std::fs::create_dir(scratch)?;
    let cache_path = scratch.join("nodes");
    let cache = NodeCache::build(pbf, &cache_path, &needed.finish())?;
    type Histogram = BTreeMap<(u16, u8, usize, u8), u64>;
    let histograms = samples
        .par_iter()
        .try_fold(Histogram::new, |mut counts, sample| -> Result<_> {
            let [latitude, longitude] = cache
                .get(sample.node)
                .ok_or_else(|| anyhow!("missing node of signed road"))?;
            let country = regions.resolve(latitude, longitude, true).country;
            if country != 0 {
                *counts
                    .entry((country, sample.class, sample.environment, sample.speed))
                    .or_default() += 1;
            }
            Ok(counts)
        })
        .try_reduce(Histogram::new, |mut left, right| {
            for (key, count) in right {
                *left.entry(key).or_default() += count;
            }
            Ok(left)
        })?;
    let mut groups = BTreeMap::<(u16, u8, usize), Vec<(u8, u64)>>::new();
    for (&(country, class, area, speed), &count) in &histograms {
        groups
            .entry((country, class, area))
            .or_default()
            .push((speed, count));
    }
    let mut countries = BTreeMap::<u16, [[u8; 2]; 8]>::new();
    for ((country, class, area), values) in groups {
        let position = (values.iter().map(|(_, n)| n).sum::<u64>() - 1) / 2;
        let mut seen = 0;
        for (speed, count) in values {
            seen += count;
            if seen > position {
                countries.entry(country).or_insert([[0; 2]; 8])[usize::from(class - 2)][area] =
                    speed;
                break;
            }
        }
    }
    let rows = countries.into_iter().map(|(country,value)| json!({"country": String::from_utf8_lossy(&country.to_le_bytes()), "value":value})).collect::<Vec<_>>();
    let counts = histograms
        .into_iter()
        .map(|((country, class, area, speed), count)| {
            json!([
                String::from_utf8_lossy(&country.to_le_bytes()),
                class,
                area,
                speed,
                count
            ])
        })
        .collect::<Vec<_>>();
    std::fs::write(output, serde_json::to_vec_pretty(&json!({"rows":rows}))?)?;
    std::fs::write(
        output.with_extension("samples.json"),
        serde_json::to_vec(&counts)?,
    )?;
    drop(cache);
    std::fs::remove_file(cache_path)?;
    std::fs::remove_dir(scratch)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_numeric_speed_does_not_invent_an_urban_or_rural_environment() {
        assert_eq!(
            signed_environment(|key| (key == "maxspeed").then_some("50")),
            None
        );
        assert_eq!(
            signed_environment(|key| (key == "maxspeed:type").then_some("XX:rural")),
            Some(0)
        );
        assert_eq!(
            signed_environment(|key| (key == "zone:traffic").then_some("XX:urban")),
            Some(1)
        );
    }
}

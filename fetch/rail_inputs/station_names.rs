//! Preserve the publisher station-name join, including original input order for equal normalized names.
use crate::railways::movements::Stop;
use serde::{
    Deserialize,
    de::{MapAccess, Visitor},
};
use std::{collections::HashMap, path::Path};

#[derive(Deserialize)]
struct Coordinate {
    lat: f64,
    lon: f64,
}
#[derive(Default)]
pub struct Stations {
    names: HashMap<String, [f64; 2]>,
    normalized: HashMap<String, [f64; 2]>,
}

fn normalized(name: &str) -> String {
    static ABBREVIATIONS: std::sync::LazyLock<[regex::Regex; 2]> = std::sync::LazyLock::new(|| {
        [
            regex::Regex::new(r"pha\s*hl\.?n\.?").unwrap(),
            regex::Regex::new(r"hl\.?n\.?").unwrap(),
        ]
    });
    let name = name.to_lowercase().replace(['-', ' '], "");
    let name = ABBREVIATIONS[0].replace(&name, "prahahlavn");
    let name = ABBREVIATIONS[1].replace(&name, "hlavní nádraží");
    name.chars()
        .map(|value| match value {
            'á' | 'à' => 'a',
            'é' | 'è' | 'ě' => 'e',
            'í' | 'ì' => 'i',
            'ó' | 'ò' => 'o',
            'ú' | 'ù' | 'ů' => 'u',
            'ý' => 'y',
            'č' => 'c',
            'ď' => 'd',
            'ň' => 'n',
            'ř' => 'r',
            'š' => 's',
            'ť' => 't',
            'ž' => 'z',
            other => other,
        })
        .collect()
}

impl<'de> Deserialize<'de> for Stations {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Names;
        impl<'de> Visitor<'de> for Names {
            type Value = Stations;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("an ordered station-name coordinate object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Stations, A::Error> {
                let mut stations = Stations::default();
                while let Some((name, point)) = map.next_entry::<String, Coordinate>()? {
                    if name.is_empty()
                        || !(-90.0..=90.0).contains(&point.lat)
                        || !(-180.0..=180.0).contains(&point.lon)
                    {
                        return Err(serde::de::Error::custom(
                            "invalid station name or coordinates",
                        ));
                    }
                    stations
                        .normalized
                        .insert(normalized(&name), [point.lat, point.lon]);
                    stations.names.insert(name, [point.lat, point.lon]);
                }
                Ok(stations)
            }
        }
        deserializer.deserialize_map(Names)
    }
}

impl Stations {
    pub fn read(path: &Path) -> Result<Self, String> {
        let stations: Self =
            serde_json::from_slice(&std::fs::read(path).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?;
        if stations.names.is_empty() {
            return Err("empty station coordinate input".into());
        }
        Ok(stations)
    }
    pub fn resolve(&self, code: &str, name: &str) -> Option<Stop> {
        let &[latitude, longitude] = self
            .names
            .get(name)
            .or_else(|| self.normalized.get(&normalized(name)))?;
        Some(Stop {
            source_id: code.to_string(),
            id: code.to_string(),
            latitude,
            longitude,
        })
    }
}

//! Manifest-selected legal OSM tokens; explicit speed syntax never selects a country's law.

use super::classify::{Tags, oneway_direction, parse_maxspeed_kmh};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SpeedRules {
    #[serde(rename = "rows")]
    rules: Vec<Rule>,
    aliases: BTreeMap<String, String>,
    zone_units: BTreeMap<String, String>,
    #[serde(default)]
    default_zone_unit: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Rule {
    token: String,
    speed: u16,
    #[serde(default)]
    road_only: bool,
    #[serde(default)]
    highway: Option<String>,
    #[serde(default)]
    minimum_lanes_per_direction: Option<u8>,
}

impl SpeedRules {
    pub fn read(path: Option<&Path>) -> Result<Self> {
        let Some(path) = path else {
            return Ok(Self::default());
        };
        let rules: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        for rule in &rules.rules {
            ensure!(
                rule.token.contains(':') && rule.token == rule.token.to_ascii_lowercase(),
                "invalid speed token {}",
                rule.token
            );
            ensure!(
                (1..=400).contains(&rule.speed) || rule.speed == u16::MAX,
                "invalid speed for {}",
                rule.token
            );
        }
        ensure!(
            rules
                .zone_units
                .values()
                .chain(rules.default_zone_unit.iter())
                .all(|unit| unit == "mph" || unit == "km/h"),
            "invalid numeric zone unit"
        );
        Ok(rules)
    }

    pub fn explicit_speed(&self, raw: &str) -> u16 {
        self.token_speed(raw, &Tags::new(), false)
    }

    fn token_speed(&self, raw: &str, tags: &Tags, road: bool) -> u16 {
        let token = raw
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let Some((jurisdiction, name)) = token.split_once(':') else {
            return parse_maxspeed_kmh(raw);
        };
        let jurisdiction = self
            .aliases
            .get(jurisdiction)
            .map(String::as_str)
            .unwrap_or(jurisdiction);
        let token = format!("{jurisdiction}:{name}");
        let highway = tags.get("highway").map(String::as_str).unwrap_or("");
        let lanes = tags
            .get("lanes")
            .and_then(|value| value.parse::<u8>().ok())
            .unwrap_or(0);
        let oneway = oneway_direction(
            highway,
            tags.get("oneway").map(String::as_str),
            tags.get("junction").map(String::as_str),
        ) != 0;
        let lanes_per_direction = if oneway { lanes } else { lanes / 2 };
        for rule in &self.rules {
            if rule.token == token
                && (!rule.road_only || road)
                && rule
                    .highway
                    .as_ref()
                    .is_none_or(|required| required == highway)
                && rule
                    .minimum_lanes_per_direction
                    .is_none_or(|minimum| lanes_per_direction >= minimum)
            {
                return rule.speed;
            }
        }
        if !(jurisdiction.len() == 2
            || (jurisdiction.len() > 3 && jurisdiction.as_bytes()[2] == b'-'))
        {
            return 0;
        }
        let number = name
            .strip_prefix("zone")
            .unwrap_or(name)
            .trim_start_matches(':');
        if !number.bytes().all(|byte| byte.is_ascii_digit()) {
            return 0;
        }
        let Some(speed) = number
            .parse::<u16>()
            .ok()
            .filter(|speed| (1..=400).contains(speed))
        else {
            return 0;
        };
        let country = jurisdiction.split('-').next().unwrap_or(jurisdiction);
        let country = self
            .aliases
            .get(country)
            .map(String::as_str)
            .unwrap_or(country);
        let Some(unit) = self
            .zone_units
            .get(country)
            .or(self.default_zone_unit.as_ref())
        else {
            return 0;
        };
        parse_maxspeed_kmh(&format!("{speed} {unit}"))
    }

    pub fn road_speed(&self, tags: &Tags) -> u16 {
        if let Some(explicit) = tags.get("maxspeed") {
            return self.token_speed(explicit, tags, true);
        }
        ["maxspeed:type", "zone:maxspeed", "source:maxspeed"]
            .iter()
            .filter_map(|key| tags.get(*key))
            .map(|value| self.token_speed(value, tags, true))
            .find(|speed| *speed > 0)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn laws_are_instance_inputs_and_explicit_unknown_keeps_precedence() {
        let rules: SpeedRules = serde_json::from_value(serde_json::json!({
            "rows": [{"token":"xy:urban","speed":50,"minimum_lanes_per_direction":2}, {"token":"xy:urban","speed":30}],
            "aliases": {"xz":"xy"}, "zone_units": {"xy":"mph"}
        })).unwrap();
        let mut tags = Tags::from([
            ("highway".into(), "primary".into()),
            ("maxspeed:type".into(), "XZ:urban".into()),
            ("lanes".into(), "4".into()),
        ]);
        assert_eq!(rules.road_speed(&tags), 50);
        assert_eq!(SpeedRules::default().road_speed(&tags), 0);
        assert_eq!(SpeedRules::default().explicit_speed("XY:zone25"), 0);
        tags.insert("lanes".into(), "2".into());
        assert_eq!(rules.road_speed(&tags), 30);
        assert_eq!(rules.explicit_speed("XY:zone25"), 40);
        assert_eq!(rules.explicit_speed("XZ-AB:zone25"), 40);
        tags.insert("maxspeed".into(), "signals".into());
        assert_eq!(rules.road_speed(&tags), 0);
        tags.insert("maxspeed".into(), "60 mph".into());
        assert_eq!(rules.road_speed(&tags), 97);
    }

    #[test]
    fn road_only_laws_never_become_posted_railway_speeds() {
        let rules: SpeedRules = serde_json::from_value(serde_json::json!({
            "rows": [{"token":"xy:urban","speed":30,"road_only":true},
                     {"token":"xy:rural","speed":80}],
            "aliases": {}, "zone_units": {}
        }))
        .unwrap();
        let tags = Tags::from([("maxspeed".into(), "XY:urban".into())]);
        assert_eq!(rules.road_speed(&tags), 30);
        assert_eq!(rules.explicit_speed("XY:urban"), 0);
        assert_eq!(rules.explicit_speed("XY:rural"), 80);
    }
}

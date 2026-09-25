//! Resolve determinate OSM implicit passenger-car limits; keep ambiguous rules as unknown.

use crate::classify::{parse_maxspeed_kmh, Tags, MAXSPEED_NONE};

// OSM tagging definitions, checked 2026-09-24 (CC BY-SA 2.0):
// https://wiki.openstreetmap.org/wiki/Key:maxspeed#Implicit_maxspeed_values
// Values describe encoded rules, not a country-wide speed prior. Context-dependent
// rules (ES:urban, CA-AB:rural, TR:motorway, etc.) deliberately remain unresolved.
// This is the only implicit-rule table. Explicit maxspeed always takes precedence.
const RULES: &[(&str, &[(&str, u16)])] = &[
    (
        "ar",
        &[
            ("urban", 40),
            ("urban:primary", 60),
            ("urban:secondary", 60),
            ("rural", 110),
            ("motorway", 130),
        ],
    ),
    (
        "at",
        &[
            ("urban", 50),
            ("rural", 100),
            ("trunk", 100),
            ("motorway", 130),
            ("bicycle_road", 30),
        ],
    ),
    (
        "by",
        &[
            ("urban", 60),
            ("rural", 90),
            ("living_street", 20),
            ("motorway", 110),
        ],
    ),
    ("be-vlg", &[("urban", 50), ("rural", 70)]),
    ("be-wal", &[("urban", 50), ("rural", 90)]),
    ("be-bru", &[("urban", 30), ("rural", 70)]),
    (
        "be",
        &[
            ("living_street", 20),
            ("cyclestreet", 30),
            ("trunk", 120),
            ("motorway", 120),
        ],
    ),
    (
        "bg",
        &[
            ("urban", 50),
            ("rural", 90),
            ("living_street", 20),
            ("trunk", 120),
            ("motorway", 140),
        ],
    ),
    ("ca-bc", &[("urban", 50), ("rural", 80)]),
    ("ca-mb", &[("urban", 50), ("rural", 90)]),
    ("ca-on", &[("urban", 50), ("rural", 80)]),
    ("ca-qc", &[("urban", 50), ("motorway", 100)]),
    ("ca-sk", &[("nsl", 80)]),
    (
        "cz",
        &[
            ("urban", 50),
            ("rural", 90),
            ("pedestrian_zone", 20),
            ("living_street", 20),
            ("urban_motorway", 80),
            ("urban_trunk", 80),
            ("trunk", 110),
            ("motorway", 130),
        ],
    ),
    ("dk", &[("urban", 50), ("rural", 80), ("motorway", 130)]),
    ("ee", &[("urban", 50), ("rural", 90)]),
    ("fi", &[("urban", 50), ("rural", 80)]),
    ("fr", &[("urban", 50), ("rural", 80), ("motorway", 130)]),
    (
        "de",
        &[
            ("urban", 50),
            ("rural", 100),
            ("bicycle_road", 30),
            ("motorway", MAXSPEED_NONE),
        ],
    ),
    (
        "gb",
        &[
            ("nsl_restricted", 48),
            ("nsl_single", 97),
            ("nsl_dual", 113),
            ("motorway", 113),
        ],
    ),
    (
        "hu",
        &[
            ("urban", 50),
            ("rural", 90),
            ("living_street", 20),
            ("trunk", 110),
            ("motorway", 130),
        ],
    ),
    (
        "id",
        &[
            ("urban", 50),
            ("rural", 80),
            ("residential", 30),
            ("motorway", 100),
        ],
    ),
    (
        "it",
        &[
            ("urban", 50),
            ("rural", 90),
            ("trunk", 110),
            ("motorway", 130),
        ],
    ),
    ("no", &[("urban", 50), ("rural", 80)]),
    (
        "pt",
        &[
            ("urban", 50),
            ("rural", 90),
            ("trunk", 100),
            ("motorway", 120),
        ],
    ),
    (
        "ro",
        &[
            ("urban", 50),
            ("rural", 90),
            ("trunk", 100),
            ("motorway", 130),
        ],
    ),
    (
        "ru",
        &[
            ("urban", 60),
            ("rural", 90),
            ("living_street", 20),
            ("motorway", 110),
        ],
    ),
    (
        "rs",
        &[
            ("urban", 50),
            ("rural", 80),
            ("living_street", 10),
            ("trunk", 100),
            ("motorway", 130),
        ],
    ),
    (
        "sk",
        &[
            ("urban", 50),
            ("rural", 90),
            ("living_street", 20),
            ("trunk", 90),
            ("motorway", 130),
            ("motorway_urban", 90),
        ],
    ),
    (
        "si",
        &[
            ("urban", 50),
            ("rural", 90),
            ("trunk", 110),
            ("motorway", 130),
        ],
    ),
    ("za", &[("urban", 60), ("rural", 100), ("motorway", 120)]),
    ("es", &[("motorway", 120), ("living_street", 20)]),
    ("se", &[("urban", 50), ("rural", 70)]),
    (
        "ch",
        &[
            ("urban", 50),
            ("rural", 80),
            ("trunk", 100),
            ("motorway", 120),
        ],
    ),
    (
        "ua",
        &[
            ("urban", 50),
            ("rural", 90),
            ("living_street", 20),
            ("trunk", 110),
            ("motorway", 130),
        ],
    ),
];

pub fn resolve(token: &str) -> Option<u16> {
    let (country, rule) = token.split_once(':')?;
    let country = if country == "uk" { "gb" } else { country };
    if let Some((_, rules)) = RULES.iter().find(|(key, _)| *key == country) {
        if let Some((_, speed)) = rules.iter().find(|(key, _)| *key == rule) {
            return Some(*speed);
        }
    }
    // Numeric zone limits encode the number, not a legislative assumption.
    if !(country.len() == 2 || (country.len() > 3 && country.as_bytes()[2] == b'-')) {
        return None;
    }
    let number = rule
        .strip_prefix("zone")
        .unwrap_or(rule)
        .trim_start_matches(':');
    if number.chars().all(|c| c.is_ascii_digit()) {
        let speed = number.parse::<u16>().ok().filter(|v| *v > 0 && *v <= 400)?;
        let jurisdiction = country.split('-').next()?;
        return Some(
            if matches!(jurisdiction, "gb" | "uk" | "us" | "gg" | "im" | "je") {
                parse_maxspeed_kmh(&format!("{speed} mph"))
            } else {
                speed
            },
        );
    }
    None
}

pub fn road_speed(tags: &Tags) -> u16 {
    if let Some(explicit) = tags.get("maxspeed") {
        return parse_maxspeed_kmh(explicit);
    }
    ["maxspeed:type", "zone:maxspeed", "source:maxspeed"]
        .iter()
        .filter_map(|key| tags.get(*key))
        .map(|raw| parse_maxspeed_kmh(raw))
        .find(|speed| *speed > 0)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn implicit_mph_zones_and_restricted_roads_convert_to_kmh() {
        for (raw, speed) in [
            ("GB:nsl_restricted", 48),
            ("UK:nsl_restricted", 48),
            ("GB:zone20", 32),
            ("UK:zone:20", 32),
            ("US:zone:25", 40),
            ("US-CA:zone25", 40),
            ("GB-ENG:zone20", 32),
            ("GG:zone25", 40),
            ("JE:zone20", 32),
            ("IM:zone30", 48),
            ("DE:zone20", 20),
            ("CZ:zone:30", 30),
            ("CA-ON:zone40", 40),
        ] {
            assert_eq!(parse_maxspeed_kmh(raw), speed, "{raw}");
        }
        for raw in ["GB:zone0", "US:zone999", "US:zonefast"] {
            assert_eq!(parse_maxspeed_kmh(raw), 0, "{raw}");
        }
    }

    #[test]
    fn implicit_rules_explicit_precedence_and_ambiguity() {
        for (raw, speed) in [
            ("DE:urban", 50),
            ("CZ:rural", 90),
            ("RO:urban", 50),
            ("DE:motorway", MAXSPEED_NONE),
            ("DE:zone:30", 30),
            ("GB:nsl_single", 97),
        ] {
            assert_eq!(parse_maxspeed_kmh(raw), speed);
        }
        for raw in ["XX:urban", "ES:urban", "CA-AB:rural", "DE:living_street"] {
            assert_eq!(parse_maxspeed_kmh(raw), 0);
        }
        let mut tags = Tags::from([("source:maxspeed".into(), "DE:urban".into())]);
        assert_eq!(road_speed(&tags), 50);
        tags.insert("maxspeed".into(), "30".into());
        assert_eq!(road_speed(&tags), 30);
        tags.insert("maxspeed".into(), "signals".into());
        assert_eq!(road_speed(&tags), 0);
    }
}

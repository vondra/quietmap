//! Resolve determinate OSM implicit passenger-car limits; keep ambiguous rules as unknown.

use crate::classify::{oneway_direction, parse_maxspeed_kmh, Tags, MAXSPEED_NONE};

// OSM tagging definitions, checked 2026-09-24 (CC BY-SA 2.0):
// https://wiki.openstreetmap.org/wiki/Key:maxspeed#Implicit_maxspeed_values
// Values describe encoded rules, not a country-wide speed prior. Context-dependent
// rules (CA-AB:rural, TR:motorway, etc.) deliberately remain unresolved, except
// ES:urban, which the Spanish lane rule below makes determinate from OSM tags.
// This is the only implicit-rule table. Explicit maxspeed always takes precedence.
//
// Urban audit 2026-09-26: each row's law and the date its current value took effect.
// Full citations (URLs) are in the audit report; the article cites below locate each law.
// ES: Art 50 RGC as amended by RD 970/2020 (BOE-A-2020-13969,
//     https://www.boe.es/diario_boe/txt.php?id=BOE-A-2020-13969, in force 2021-05-11):
//     20 single-platform, 30 one lane per direction, 50 two or more; rural Art 48
//     as amended by RD 1514/2018 (https://www.boe.es/eli/es/rd/2018/12/28/1514,
//     in force 2019-01-29): 90. Reserved bus/taxi lanes do not count (Art 50).
// FR: Code de la route R413-3/R413-2 (urban 50 since 1990-12-01; rural 80 since
//     2018-07-01; departments may sign 90, which is then explicit, not implicit).
// BE: regional law — Flanders rural 70 (since 2017-01-01), Wallonia rural 90,
//     Brussels urban 30 (since 2021-01-01); plain BE:urban/BE:rural stay unknown.
// NL: RVV 1990 art 20 (urban 50, rural 80, autoweg 100; motorway 100 by day
//     since 2020-03-16, 130 at night — no single implicit value, omitted).
// DE: StVO §3 (urban 50, rural 100, no general motorway limit).
// AT: StVO §20 (urban 50, rural 100, motorway 130).
// CH: VRV Art 4a (urban 50, rural 80, Autostrasse 100, motorway 120; 50/80 since 1984).
// IT: Codice della Strada Art 142, D.Lgs. 285/1992 (50/90/110/130, in force 1993-01-01).
// PT: Codigo da Estrada Art 27 (localidades 50, other roads 90, vias reservadas 100,
//     motorways 120).
// PL: Prawo o ruchu drogowym Art 20 (urban 50 by day, 60 at night 23-05 — the table
//     holds the day value, the model's one speed covers all periods; rural 90,
//     motorway 140; expressway 100/120 needs lane data, omitted).
// CZ: 361/2000 Sb. §18 (urban 50, rural 90, motor-vehicle road 110, motorway 130;
//     80 on motorways inside built-up areas).
// GB: RTRA 1984 s81-86 (30/60/70 mph on restricted/single/dual roads; Wales has a
//     20 mph restricted-road default since 2023-09-17 — needs a subnational boundary).
// IE: Road Traffic Act 2004 as amended by RTA 2024 (urban 50; rural 60 on local
//     roads since 2025-02-07, 80 on regional roads — IE:rural stays unknown).
// SE: Trafikforordningen 1998:1276 3:17 (urban 50, rural 70; motorways always signed).
// DK: Faerdselsloven §42 (urban 50, rural 80, motorway 130).
// NO: general limits 50/80 (motorways always signed).
// FI: Tieliikennelaki 729/2018 (urban 50, rural 80, in force 2020-06-01; motorways signed).
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
        "ie",
        &[
            ("urban", 50),
            ("motorway", 120),
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
    (
        "nl",
        &[
            ("urban", 50),
            ("rural", 80),
            ("motorroad", 100),
            ("living_street", 15),
        ],
    ),
    ("no", &[("urban", 50), ("rural", 80)]),
    (
        "pl",
        &[
            ("urban", 50),
            ("rural", 90),
            ("living_street", 20),
            ("motorway", 140),
        ],
    ),
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
    (
        "es",
        &[
            ("rural", 90),
            ("motorway", 120),
            ("living_street", 20),
        ],
    ),
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

/// Spain's urban limit from Art 50 RGC (RD 970/2020): 20 on single-platform
/// streets, 30 on one lane per direction, 50 on two or more. OSM has no
/// single-platform tag, so `living_street` is the only 20 proxy; an unmarked
/// carriageway (`lanes` missing or unparsable) carries one effective lane per
/// direction and takes 30. Reserved bus/taxi lanes do not count under the law
/// but OSM never maps them alongside ES:urban (`lanes:bus/psv` absent on every
/// ES:urban way of planet-260921), so the plain `lanes` tag decides.
fn es_urban_speed(tags: &Tags) -> u16 {
    if tags.get("highway").is_some_and(|s| s == "living_street") {
        return 20;
    }
    let lanes = tags
        .get("lanes")
        .and_then(|s| s.parse::<u8>().ok())
        .unwrap_or(0);
    if lanes == 0 {
        return 30;
    }
    let oneway = oneway_direction(
        tags.get("highway").map(String::as_str).unwrap_or(""),
        tags.get("oneway").map(String::as_str),
        tags.get("junction").map(String::as_str),
    ) != 0;
    // Two-way `lanes` span both directions; three lanes hold one full lane per
    // direction plus a middle, so integer division floors to the ruled count.
    let per_direction = if oneway { lanes } else { lanes / 2 };
    if per_direction >= 2 { 50 } else { 30 }
}

/// Whether a raw tag value is the lane-counted Spanish urban rule (same
/// first-token lowercased normalization as [`parse_maxspeed_kmh`]).
fn is_es_urban_token(raw: &str) -> bool {
    raw.split(';')
        .next()
        .unwrap_or("")
        .trim()
        .eq_ignore_ascii_case("es:urban")
}

/// One raw tag value to km/h: the lane-counted ES:urban rule, else the shared parser.
fn token_speed(raw: &str, tags: &Tags) -> u16 {
    if is_es_urban_token(raw) {
        return es_urban_speed(tags);
    }
    parse_maxspeed_kmh(raw)
}

pub fn road_speed(tags: &Tags) -> u16 {
    if let Some(explicit) = tags.get("maxspeed") {
        return token_speed(explicit, tags);
    }
    ["maxspeed:type", "zone:maxspeed", "source:maxspeed"]
        .iter()
        .filter_map(|key| tags.get(*key))
        .map(|raw| token_speed(raw, tags))
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
            ("NL:urban", 50),
            ("NL:rural", 80),
            ("NL:motorroad", 100),
            ("NL:living_street", 15),
            ("PL:urban", 50),
            ("PL:rural", 90),
            ("PL:motorway", 140),
            ("PL:living_street", 20),
            ("IE:urban", 50),
            ("IE:motorway", 120),
            ("ES:rural", 90),
            ("ES:motorway", 120),
        ] {
            assert_eq!(parse_maxspeed_kmh(raw), speed);
        }
        // ES:urban needs lane counts, which a bare value cannot carry: the shared
        // parser leaves it unknown and only road_speed (with tags) resolves it.
        for raw in [
            "XX:urban",
            "ES:urban",
            "CA-AB:rural",
            "DE:living_street",
            "BE:urban",
            "BE:rural",
            "IE:rural",
            "NL:motorway",
        ] {
            assert_eq!(parse_maxspeed_kmh(raw), 0);
        }
        let mut tags = Tags::from([("source:maxspeed".into(), "DE:urban".into())]);
        assert_eq!(road_speed(&tags), 50);
        tags.insert("maxspeed".into(), "30".into());
        assert_eq!(road_speed(&tags), 30);
        tags.insert("maxspeed".into(), "signals".into());
        assert_eq!(road_speed(&tags), 0);
    }

    #[test]
    fn es_urban_follows_lane_counts_per_direction() {
        let speed = |pairs: &[(&str, &str)]| {
            road_speed(&Tags::from_iter(
                pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())),
            ))
        };
        // One lane per direction, however tagged, is 30; three two-way lanes
        // hold one full lane per direction plus a middle.
        for (lanes, oneway) in [("1", "yes"), ("2", "no"), ("3", "no"), ("2", "")] {
            let mut pairs = vec![
                ("highway", "residential"),
                ("maxspeed:type", "ES:urban"),
                ("lanes", lanes),
            ];
            if !oneway.is_empty() {
                pairs.push(("oneway", oneway));
            }
            assert_eq!(speed(&pairs), 30, "lanes={lanes} oneway={oneway}");
        }
        // Two or more lanes per direction is 50.
        assert_eq!(
            speed(&[
                ("highway", "primary"),
                ("source:maxspeed", "ES:urban"),
                ("lanes", "2"),
                ("oneway", "yes"),
            ]),
            50
        );
        assert_eq!(
            speed(&[
                ("highway", "primary"),
                ("source:maxspeed", "ES:urban"),
                ("lanes", "4"),
            ]),
            50
        );
        assert_eq!(
            speed(&[
                ("highway", "secondary"),
                ("zone:maxspeed", "es:urban"),
                ("lanes", "3"),
                ("oneway", "yes"),
            ]),
            50
        );
        // Unmarked and unparsable carriageways take 30.
        assert_eq!(
            speed(&[("highway", "residential"), ("source:maxspeed", "ES:urban")]),
            30
        );
        assert_eq!(
            speed(&[
                ("highway", "residential"),
                ("source:maxspeed", "ES:urban"),
                ("lanes", "2;3"),
            ]),
            30
        );
        // Living streets are single-platform 20.
        assert_eq!(
            speed(&[
                ("highway", "living_street"),
                ("source:maxspeed", "ES:urban"),
                ("lanes", "2"),
                ("oneway", "yes"),
            ]),
            20
        );
        // The rule also applies when the token sits in maxspeed itself.
        assert_eq!(
            speed(&[
                ("highway", "tertiary"),
                ("maxspeed", "ES:urban"),
                ("lanes", "2"),
                ("oneway", "yes"),
            ]),
            50
        );
        // Explicit numeric maxspeed keeps priority over the implicit rule.
        assert_eq!(
            speed(&[
                ("highway", "tertiary"),
                ("maxspeed", "50"),
                ("source:maxspeed", "ES:urban"),
                ("lanes", "1"),
                ("oneway", "yes"),
            ]),
            50
        );
        assert_eq!(
            speed(&[
                ("highway", "tertiary"),
                ("maxspeed", "30"),
                ("source:maxspeed", "ES:urban"),
                ("lanes", "4"),
            ]),
            30
        );
    }
}

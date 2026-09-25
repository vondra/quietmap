//! Shared tag admission and source classes for roads, rail, power and special leisure.

use super::{FeatureType, Tags};

pub fn keep_model_tag(family: &FeatureType, key: &str) -> bool {
    let geometry = matches!(
        key,
        "layer"
            | "bridge"
            | "tunnel"
            | "embankment"
            | "cutting"
            | "height"
            | "width"
            | "bridge:structure"
            | "bridge:movable"
            | "maxheight"
    );
    match family {
        FeatureType::Road => {
            geometry
                || key.starts_with("maxspeed:")
                || matches!(key, "source:maxspeed" | "zone:maxspeed")
        }
        FeatureType::Railway => {
            geometry
                || key.starts_with("railway:")
                || matches!(
                    key,
                    "railway"
                        | "usage"
                        | "service"
                        | "tracktype"
                        | "surface"
                        | "embedded_rails"
                        | "heritage"
                        | "historic"
                )
        }
        FeatureType::Industrial | FeatureType::WindTurbine => {
            key.starts_with("generator:")
                || key.starts_with("plant:")
                || lifecycle_key(key)
                || matches!(
                    key,
                    "power" | "substation" | "voltage" | "rating" | "transformer"
                )
        }
        FeatureType::Building => {
            matches!(key, "sport" | "indoor") || keep_model_tag(&FeatureType::Industrial, key)
        }
        FeatureType::Leisure => {
            key.starts_with("shooting:")
                || matches!(
                    key,
                    "highway" | "sport" | "shooting" | "building" | "indoor" | "area" | "surface"
                )
        }
        _ => false,
    }
}

fn lifecycle_key(key: &str) -> bool {
    [
        "disused",
        "abandoned",
        "historic",
        "razed",
        "demolished",
        "construction",
        "proposed",
    ]
    .iter()
    .any(|prefix| key == *prefix || key.strip_prefix(prefix).is_some_and(|s| s.starts_with(':')))
}

/// The building itself is power infrastructure, rather than hosting a generator.
pub fn is_power_building<'a>(tag: impl Fn(&str) -> Option<&'a str>) -> bool {
    matches!(tag("power"), Some("plant" | "substation"))
}

pub fn is_power_or_inactive_industry<'a>(tag: impl Fn(&str) -> Option<&'a str>) -> bool {
    matches!(
        tag("power"),
        Some("plant" | "substation" | "generator" | "transformer")
    ) || [
        "disused:power",
        "abandoned:power",
        "razed:power",
        "demolished:power",
        "construction:power",
        "proposed:power",
    ]
    .iter()
    .any(|key| {
        matches!(
            tag(key),
            Some("plant" | "substation" | "generator" | "transformer")
        )
    }) || [
        "disused:landuse",
        "abandoned:landuse",
        "historic:landuse",
        "razed:landuse",
        "demolished:landuse",
        "construction:landuse",
        "proposed:landuse",
    ]
    .iter()
    .any(|key| matches!(tag(key), Some("quarry" | "industrial")))
}

pub fn industrial_class(tags: &Tags) -> Option<u8> {
    let tag = |key: &str| tags.get(key).map(String::as_str);
    if tags.iter().any(|(key, value)| {
        (lifecycle_key(key)
            && key.split_once(':').is_none_or(|(_, feature)| {
                matches!(feature, "power" | "landuse" | "quarry" | "man_made")
            })
            && (!key.starts_with("historic")
                || tag("landuse") == Some("quarry")
                || key == "historic:landuse"))
            && !matches!(value.as_str(), "no" | "false" | "0" | "")
    }) {
        return Some(12); // inactive facility, retained for audit, no emission
    }
    match tag("power") {
        Some("substation") => return Some(14),
        Some("transformer") => return Some(15),
        _ => {}
    }
    if tag("power") == Some("plant")
        && tag("plant:source").is_some_and(|v| v.split(';').all(|s| s.trim() == "wind"))
    {
        return Some(11); // enclosing site, individual turbines own the emission
    }
    if tag("plant:source") == Some("solar")
        || (tag("power") != Some("plant") && tag("generator:source") == Some("solar"))
    {
        return Some(13);
    }
    None
}

pub fn special_leisure_class<'a>(tag: impl Fn(&str) -> Option<&'a str>) -> Option<u8> {
    let sports: Vec<_> = tag("sport")
        .unwrap_or("")
        .split(';')
        .map(str::trim)
        .collect();
    if tag("leisure") == Some("shooting_ground") || sports.contains(&"shooting") {
        Some(11)
    } else if tag("highway") == Some("raceway")
        || sports.iter().any(|sport| {
            matches!(
                *sport,
                "motor"
                    | "motorsport"
                    | "motor_sports"
                    | "karting"
                    | "motocross"
                    | "motorcycle"
                    | "auto_racing"
                    | "speedway"
                    | "stock_car_racing"
                    | "drag_racing"
            )
        })
    {
        Some(10)
    } else {
        None
    }
}

pub fn is_leisure_line<'a>(tag: impl Fn(&str) -> Option<&'a str>, closed: bool) -> bool {
    tag("area") != Some("yes")
        && (tag("highway") == Some("raceway")
            || (tag("leisure") == Some("track")
                && (!closed || special_leisure_class(&tag) == Some(10))))
}

pub fn is_special_leisure<'a>(tag: impl Fn(&str) -> Option<&'a str>) -> bool {
    special_leisure_class(tag).is_some()
}

/// Node features whose exact shared-node membership defines road/track attachment.
/// Preserve the complete crossing/sign family, including national whistle board codes.
pub fn transport_point_tags<'a>(tags: impl Iterator<Item = (&'a str, &'a str)>) -> Option<Tags> {
    let mut kept = Tags::new();
    let mut relevant = false;
    for (key, value) in tags {
        relevant |= (key == "highway" && value == "traffic_signals")
            || (key == "crossing" && matches!(value, "traffic_signals" | "signals"))
            || (key == "crossing:signals" && value == "yes")
            || (key == "railway" && value == "level_crossing")
            || key.starts_with("railway:signal:whistle");
        if matches!(
            key,
            "highway" | "railway" | "crossing" | "direction" | "name" | "ref"
        ) || key.starts_with("crossing:")
            || key.starts_with("traffic_signals")
            || key.starts_with("railway:signal:")
        {
            kept.insert(key.to_owned(), value.to_owned());
        }
    }
    relevant.then_some(kept)
}

/// A plant outline is never an individual turbine, even if generator tags are copied onto it.
pub fn is_turbine<'a>(tag: impl Fn(&str) -> Option<&'a str>) -> bool {
    tag("power") != Some("plant")
        && (tag("generator:source") == Some("wind") || tag("man_made") == Some("wind_turbine"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tags(pairs: &[(&str, &str)]) -> Tags {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }
    #[test]
    fn inactive_industry_requires_lifecycle_of_the_facility() {
        for unrelated in [
            "disused:railway",
            "abandoned:railway",
            "construction:railway",
            "proposed:building",
            "construction:voltage",
            "construction:power:voltage",
        ] {
            let site = tags(&[("landuse", "industrial"), (unrelated, "yes")]);
            assert_eq!(industrial_class(&site), None, "{unrelated}");
            assert!(keep_model_tag(&FeatureType::Industrial, unrelated));
        }
        assert_eq!(
            industrial_class(&tags(&[
                ("power", "transformer"),
                ("construction:voltage", "110000")
            ])),
            Some(15)
        );
        for (key, value) in [
            ("disused:power", "transformer"),
            ("abandoned:landuse", "industrial"),
            ("construction:man_made", "works"),
            ("disused:quarry", "yes"),
        ] {
            assert_eq!(industrial_class(&tags(&[(key, value)])), Some(12), "{key}");
        }
        assert_eq!(
            industrial_class(&tags(&[("landuse", "quarry"), ("historic", "yes")])),
            Some(12)
        );
        assert_eq!(
            industrial_class(&tags(&[("power", "plant"), ("historic", "yes")])),
            None
        );
        for value in ["no", "false", "0", ""] {
            assert_eq!(
                industrial_class(&tags(&[
                    ("landuse", "industrial"),
                    ("disused:landuse", value)
                ])),
                None
            );
        }
    }

    #[test]
    fn only_a_sole_wind_plant_source_makes_a_silent_outline() {
        for pairs in [
            vec![("power", "plant"), ("plant:source", "wind;gas")],
            vec![("power", "plant"), ("plant:source", "gas;wind")],
            vec![
                ("power", "plant"),
                ("plant:source", "gas"),
                ("generator:source", "wind"),
            ],
            vec![("power", "plant"), ("generator:source", "wind")],
            vec![
                ("power", "plant"),
                ("plant:source", "gas"),
                ("generator:source", "solar"),
            ],
        ] {
            let site = tags(&pairs);
            assert_eq!(industrial_class(&site), None, "{pairs:?}");
            assert!(!is_turbine(|key| site.get(key).map(String::as_str)));
        }
        for source in ["wind", " wind ", "wind;wind"] {
            assert_eq!(
                industrial_class(&tags(&[("power", "plant"), ("plant:source", source)])),
                Some(11)
            );
        }
        assert_eq!(
            industrial_class(&tags(&[
                ("power", "plant"),
                ("plant:source", "solar"),
                ("generator:source", "wind")
            ])),
            Some(13)
        );
        let turbine = tags(&[("power", "generator"), ("generator:source", "wind")]);
        assert!(is_turbine(|key| turbine.get(key).map(String::as_str)));
    }

    #[test]
    fn power_classes_and_lifecycle_are_distinct() {
        for (pairs, expected) in [
            (vec![("power", "plant"), ("plant:source", "wind")], 11),
            (vec![("landuse", "quarry"), ("disused", "yes")], 12),
            (vec![("abandoned:landuse", "quarry")], 12),
            (vec![("power", "plant"), ("plant:source", "solar")], 13),
            (vec![("power", "substation")], 14),
            (vec![("power", "transformer"), ("rating", "25 MVA")], 15),
        ] {
            assert_eq!(industrial_class(&tags(&pairs)), Some(expected));
        }
        assert_eq!(
            industrial_class(&tags(&[("landuse", "quarry"), ("disused", "no")])),
            None
        );
    }
    #[test]
    fn special_sports_keep_indoor_and_motor_variants() {
        for sport in ["motor", "karting", "motocross", "motor_sports", "speedway"] {
            assert_eq!(
                special_leisure_class(|key| (key == "sport").then_some(sport)),
                Some(10)
            );
        }
        assert_eq!(
            special_leisure_class(|key| (key == "sport").then_some("shooting")),
            Some(11)
        );
        assert!(keep_model_tag(&FeatureType::Leisure, "shooting"));
        assert!(keep_model_tag(&FeatureType::Leisure, "shooting:range"));
        assert!(keep_model_tag(&FeatureType::Leisure, "indoor"));
        assert!(keep_model_tag(&FeatureType::Leisure, "building"));
    }
    #[test]
    fn crossing_and_whistle_tags_keep_national_values() {
        for pairs in [
            vec![
                ("highway", "traffic_signals"),
                ("traffic_signals:direction", "forward"),
            ],
            vec![("railway", "level_crossing"), ("crossing:barrier", "half")],
            vec![("railway:signal:whistle", "DE-ESO:bu4")],
            vec![("railway:signal:whistle", "PL-PKP:w6a")],
            vec![("railway:signal:whistle", "PL-PKP:w6b")],
        ] {
            assert_eq!(
                transport_point_tags(pairs.iter().copied()),
                Some(tags(&pairs))
            );
        }
    }
}

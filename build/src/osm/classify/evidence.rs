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

/// Railway yard area (`railway=yard`): emits as a rail-yard facility.
/// `landuse=railway` alone is the railway corridor (tracks, sidings, station
/// complexes), not a yard, so it never matches. Track ways never reach this
/// predicate (they route Railway first).
pub fn is_railway_yard<'a>(tag: impl Fn(&str) -> Option<&'a str>) -> bool {
    tag("railway") == Some("yard")
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

/// One `sport` token in the engine's spelling (lowercase, `-`/space as `_`),
/// matching `noise-compute::emission::leisure::motorsport_subtype`.
fn normalized_sport_token(token: &str) -> String {
    token.trim().to_ascii_lowercase().replace(['-', ' '], "_")
}

/// True for every `sport` token the engine subtypes as motorsport — the same
/// list as `motorsport_subtype` (osm-extract must not depend on noise-compute,
/// so the list is shared by convention): circuit, motocross, kart, speedway,
/// trial and untyped motor variants, plus radio-controlled (the engine
/// silences those rows; area law would invent a generic leisure source).
fn is_motorsport_token(token: &str) -> bool {
    matches!(
        token,
        "autocross"
            | "rallycross"
            | "car_racing"
            | "formula_one"
            | "road_racing"
            | "drifting"
            | "stockcar"
            | "auto_racing"
            | "stock_car_racing"
            | "drag_racing"
            | "motocross"
            | "supermoto"
            | "enduro"
            | "karting"
            | "kart"
            | "go_kart"
            | "speedway"
            | "trial"
            | "motor"
            | "motorsport"
            | "motor_sports"
            | "motorcycle"
            | "rc_car"
            | "rc_racing"
            | "radiocontrol"
            | "radio_control"
    )
}

pub fn special_leisure_class<'a>(tag: impl Fn(&str) -> Option<&'a str>) -> Option<u8> {
    let sports: Vec<_> = tag("sport")
        .unwrap_or("")
        .split(';')
        .map(normalized_sport_token)
        .collect();
    if tag("leisure") == Some("shooting_ground") || sports.iter().any(|sport| sport == "shooting") {
        Some(11)
    } else if tag("highway") == Some("raceway")
        || sports.iter().any(|sport| is_motorsport_token(sport))
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
#[path = "evidence_tests.rs"]
mod tests;

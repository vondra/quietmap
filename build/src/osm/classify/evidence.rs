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
                || military_lifecycle_key(key)
                || matches!(
                    key,
                    "power"
                        | "substation"
                        | "voltage"
                        | "rating"
                        | "transformer"
                        | "military"
                        | "access"
                        | "hazard"
                        | "boundary"
                        | "area"
                        | "indoor"
                        | "building"
                        | "seamark:type"
                        | "end_date"
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

const MILITARY_LIFECYCLE: [(&str, &str, &str); 12] = [
    ("disused", "disused:military", "disused:landuse"),
    ("abandoned", "abandoned:military", "abandoned:landuse"),
    ("historic", "historic:military", "historic:landuse"),
    ("razed", "razed:military", "razed:landuse"),
    ("demolished", "demolished:military", "demolished:landuse"),
    (
        "construction",
        "construction:military",
        "construction:landuse",
    ),
    ("proposed", "proposed:military", "proposed:landuse"),
    ("was", "was:military", "was:landuse"),
    ("former", "former:military", "former:landuse"),
    ("removed", "removed:military", "removed:landuse"),
    ("ruins", "ruins:military", "ruins:landuse"),
    ("destroyed", "destroyed:military", "destroyed:landuse"),
];

fn military_lifecycle_key(key: &str) -> bool {
    MILITARY_LIFECYCLE.iter().any(|&(prefix, _, _)| {
        key == prefix || key.strip_prefix(prefix).is_some_and(|s| s.starts_with(':'))
    })
}

fn truthy_tag(value: &str) -> bool {
    !matches!(value, "no" | "false" | "0" | "")
}

/// True for the existing emitting industrial, power, quarry, farmyard, landfill, port and rail-yard
/// tags that must retain their emitting path even when co-tagged with `military=*`.
pub fn is_emitting_industrial<'a>(tag: impl Fn(&str) -> Option<&'a str>) -> bool {
    matches!(
        tag("landuse"),
        Some("industrial" | "quarry" | "farmyard" | "landfill" | "port" | "harbour")
    ) || is_railway_yard(&tag)
        || matches!(tag("man_made"), Some("works" | "wastewater_plant"))
        || is_power_or_inactive_industry(&tag)
        || is_turbine(&tag)
}

fn is_military_range_value(value: &str) -> bool {
    matches!(value, "range" | "training_area" | "danger_area")
}

fn has_military_inactive_lookup<'a>(tag: &impl Fn(&str) -> Option<&'a str>) -> bool {
    tag("end_date").is_some_and(truthy_tag)
        || MILITARY_LIFECYCLE.iter().any(|&(prefix, mil_key, lu_key)| {
            tag(prefix).is_some_and(truthy_tag)
                || tag(mil_key).is_some_and(truthy_tag)
                || tag(lu_key).is_some_and(truthy_tag)
        })
}

/// True when any active or lifecycle-prefixed military firing/training/danger tag is present, or
/// when an inactive `landuse=military` site carries a shooting tag, so military facilities never
/// fall through to the civilian 20,000 shots/yr shooting emitter.
pub fn has_military_shooting_veto_lookup<'a>(tag: impl Fn(&str) -> Option<&'a str>) -> bool {
    tag("military").is_some_and(is_military_range_value)
        || MILITARY_LIFECYCLE
            .iter()
            .any(|&(_, mil_key, _)| tag(mil_key).is_some_and(is_military_range_value))
        || (tag("landuse") == Some("military") && has_military_inactive_lookup(&tag))
        || MILITARY_LIFECYCLE
            .iter()
            .any(|&(_, _, lu_key)| tag(lu_key) == Some("military"))
}

pub fn has_military_shooting_veto<'a>(tags: &[(&'a str, &'a str)]) -> bool {
    has_military_shooting_veto_lookup(|key| tags.iter().find(|&&(k, _)| k == key).map(|&(_, v)| v))
}

fn has_indoor_shooting_tag(tags: &[(&str, &str)]) -> bool {
    tags.iter().any(|&(k, v)| {
        (k == "shooting" || k.starts_with("shooting:"))
            && v.split([';', ',', '/']).any(|part| {
                matches!(
                    normalized_sport_token(part).as_str(),
                    "indoor" | "indoor_range" | "virtual"
                )
            })
    })
}

/// One universal admission rule for active open-air military firing ranges, training areas and
/// danger areas: keeps real land ranges, drops barracks/bases/bunkers/bare `landuse=military`,
/// inactive/lifecycle ranges, roofed or indoor ranges, offshore sea boxes, and non-firing UXO/
/// minefield/nuclear/buffer-zone overlays.
pub fn is_active_military_range<'a>(tags: &[(&'a str, &'a str)]) -> bool {
    let tag = |key: &str| tags.iter().find(|&&(k, _)| k == key).map(|&(_, v)| v);
    let Some(military) = tag("military").filter(|v| is_military_range_value(v)) else {
        return false;
    };
    if is_emitting_industrial(tag) || has_military_inactive_lookup(&tag) {
        return false;
    }
    if tag("building").is_some_and(|v| v != "no")
        || tag("indoor").is_some_and(|v| v != "no")
        || has_indoor_shooting_tag(tags)
        || tag("area") == Some("no")
        || tag("seamark:type").is_some()
    {
        return false;
    }
    let firing_hazard = matches!(
        tag("hazard"),
        Some("shooting_range" | "firing_range" | "firing" | "shooting")
    );
    if let Some(hazard) = tag("hazard")
        && !firing_hazard
        && hazard != "yes"
    {
        return false;
    }
    if military == "danger_area" {
        match tag("boundary") {
            Some("protected_area" | "military") => return false,
            Some("hazard") if !firing_hazard => return false,
            Some("civil") if tag("landuse") != Some("military") => return false,
            _ => {}
        }
    }
    true
}

pub fn is_active_military_range_map(tags: &Tags) -> bool {
    let pairs: Vec<(&str, &str)> = tags.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    is_active_military_range(&pairs)
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
    if is_active_military_range_map(tags) {
        return Some(physics::emission::industrial::SOURCE_MILITARY_RANGE);
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

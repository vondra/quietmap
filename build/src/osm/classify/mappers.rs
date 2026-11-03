//! Stateless tag→enum/value mappers shared by way + node spill: leisure class /
//! capacity / opening-hours, road/rail/barrier/aeroway code mappers, and the
//! `width`/`maxspeed` parsers. Pure functions of their inputs — no extract state.

use super::Tags;
use crate::osm::ids;

/// Map the open-air feature to its emission class (the ids in [`ids`]). An open
/// car park wins first — it carries no `sport` and its kind decides how densely
/// its area holds spaces. Then `sport=*`, and otherwise the `leisure=*`/
/// `amenity=*` kind selects a default (playground, pool, pitch, outdoor seating,
/// stadium). Always returns a class (PITCH is the fallback).
pub fn leisure_sport_class(tags: &Tags) -> u8 {
    if let Some(class) = super::special_leisure_class(|key| tags.get(key).map(String::as_str)) {
        return class;
    }
    // A car park carries no `sport`; its own kind decides how many spaces its
    // area holds (`leisure::CAR_PARK` 23.8 m² vs `CAR_PARK_STREET` 13.3 m²).
    // Only open ground reaches the open-air lane; the rest never routes here.
    match super::parking_kind(|key| tags.get(key).map(|s| s.as_str())) {
        Some(super::ParkingKind::OpenStrip) => return ids::LEISURE_CAR_PARK_STREET,
        Some(super::ParkingKind::OpenLot) => return ids::LEISURE_CAR_PARK,
        _ => {}
    }
    // An artificial-turf pitch is booked year-round; grass (or an unmapped
    // surface) follows the seasonal club pattern. Only pitch classes remap —
    // a turf tennis court keeps the tennis duty.
    let agp_surface = tags.get("surface").is_some_and(|s| s == "artificial_turf");
    if let Some(sport) = tags.get("sport") {
        // Multi-value `sport=tennis;padel` → use the retained class
        // precedence; `>=` keeps the LAST maximum, mirroring max_by_key.
        let mut best: Option<(u8, u8)> = None;
        for token in sport.split(';') {
            let token = token.trim().to_ascii_lowercase();
            if let Some(c) = ids::leisure_sport_class_id(token.as_str()) {
                let loud = ids::leisure_sport_precedence(c);
                if best.is_none_or(|(b, _)| loud >= b) {
                    best = Some((loud, c));
                }
            }
        }
        if let Some((_, c)) = best {
            if c == ids::LEISURE_PITCH && agp_surface {
                return ids::LEISURE_AGP;
            }
            return c;
        }
    }
    if tags.get("amenity").map(|s| s.as_str()) == Some("biergarten")
        || tags.contains_key("outdoor_seating")
    {
        return ids::LEISURE_OUTDOOR_SEATING;
    }
    match tags.get("leisure").map(|s| s.as_str()) {
        Some("playground" | "dog_park") => ids::LEISURE_PLAYGROUND,
        Some("swimming_pool" | "swimming_area" | "water_park") => ids::LEISURE_POOL,
        Some("stadium") => ids::LEISURE_STADIUM,
        Some("outdoor_seating") => ids::LEISURE_OUTDOOR_SEATING,
        Some("pitch") if agp_surface => ids::LEISURE_AGP,
        _ => ids::LEISURE_PITCH,
    }
}

/// Parse OSM `opening_hours` to a coarse day-fraction enum:
/// 0 = unknown, 1 = 24/7, 2 = day-only (no late-evening/night), 3 = evening/
/// night-active. A cheap heuristic (full grammar is out of scope): `24/7` → 1;
/// a closing hour ≥ 22:00 or `Su`/late tokens → 3; anything else parseable → 2.
/// Stored as a u8 so a consumer can later refine the period split per object.
pub fn opening_hours_fraction(oh: Option<&str>) -> u8 {
    let Some(raw) = oh else { return 0 };
    let s = raw.trim().to_ascii_lowercase();
    if s.is_empty() {
        return 0;
    }
    if s == "24/7" || s.contains("00:00-24:00") || s.contains("0:00-24:00") {
        return 1;
    }
    // Find the latest closing hour `HH:MM-HH:MM`.
    let mut latest_close = 0u32;
    for tok in s.split([';', ',', ' ']) {
        if let Some((_, close)) = tok.split_once('-')
            && let Some((h, _)) = close.split_once(':')
            && let Ok(hh) = h.trim().parse::<u32>()
        {
            latest_close = latest_close.max(hh);
        }
    }
    if latest_close == 0 {
        return 0; // nothing parseable
    }
    if !(6..22).contains(&latest_close) {
        3
    } else {
        2
    }
}

/// Map highway tag to road_class enum.
///
/// Links on motorway/trunk/primary get their own codes so normalize.rs can
/// apply a ramp traffic coefficient (20 % of mainline default). Secondary/
/// tertiary links stay merged with the mainline — their defaults are already
/// close to urban street flow, so a 20 % discount would under-estimate them.
pub fn road_class(highway: &str) -> u8 {
    match highway {
        "motorway" => 0,
        "trunk" => 1,
        "primary" => 2,
        "secondary" | "secondary_link" => 3,
        "tertiary" | "tertiary_link" => 4,
        "residential" => 5,
        "living_street" => 6,
        "service" => 7,
        "track" => 8,
        "unclassified" => 9,
        "motorway_link" => 10,
        "trunk_link" => 11,
        "primary_link" => 12,
        _ => 5, // defensive fallback (extractor filter shouldn't let others through)
    }
}

/// Map surface tag to surface_type enum.
pub fn surface_type(surface: Option<&str>) -> u8 {
    match surface {
        Some("asphalt") | None => 0,
        Some("sett") => 1,
        Some("cobblestone") | Some("paving_stones") => 2,
        Some("concrete") => 3,
        Some("gravel") | Some("compacted") | Some("unpaved") => 4,
        _ => 0,
    }
}

/// Map railway tag to rail_type enum.
pub fn rail_type(railway: &str) -> u8 {
    match railway {
        "rail" => 0,
        "tram" => 1,
        // Metro shares the light-rail emission family; GTFS metro routes stamp it too.
        "light_rail" | "subway" => 2,
        "narrow_gauge" => 3,
        "funicular" => 4,
        "preserved" => 5,
        _ => 0,
    }
}

/// Whether a way is a railway that carries trains. Subway is included for its above-ground
/// sections (the spill marks the underground ones as tunnel, which emits nothing); a line
/// mapped as disused or abandoned carries no trains.
pub fn railway_carries_trains(
    railway: Option<&str>,
    disused: Option<&str>,
    abandoned: Option<&str>,
) -> bool {
    matches!(
        railway,
        Some(
            "rail" | "tram" | "light_rail" | "subway" | "narrow_gauge" | "funicular" | "preserved"
        )
    ) && disused != Some("yes")
        && abandoned != Some("yes")
}

/// Whether a railway way runs below ground. A plain railway needs the `tunnel` tag; a
/// subway is often mapped underground with only `layer`, `location` or `covered`.
pub fn railway_is_underground(
    railway: &str,
    tunnel: Option<&str>,
    layer: Option<&str>,
    location: Option<&str>,
    covered: Option<&str>,
) -> bool {
    if matches!(tunnel, Some("yes" | "building_passage" | "culvert")) {
        return true;
    }
    railway == "subway"
        && (layer
            .and_then(|value| value.trim().parse::<i32>().ok())
            .is_some_and(|value| value < 0)
            || location == Some("underground")
            || covered == Some("yes"))
}

/// Map usage tag to rail usage enum. 0=main, 1=branch, 2=industrial, 3=untagged or other.
/// An untagged line is not evidence of a main line: it takes the moderate default.
pub fn rail_usage_type(usage: Option<&str>) -> u8 {
    match usage {
        Some("main") => 0,
        Some("branch") => 1,
        Some("industrial") => 2,
        Some("tourism") => 4,
        _ => 3,
    }
}

/// Map barrier material tag to enum.
pub fn barrier_material_type(material: Option<&str>) -> u8 {
    match material {
        Some("concrete") => 0,
        Some("metal") => 1,
        Some("wood") => 2,
        Some("glass") => 3,
        Some("brick") => 4,
        _ => 0,
    }
}

/// Map junction tag to enum. Roundabouts have reduced speed.
pub fn junction_type(junction: Option<&str>) -> u8 {
    match junction {
        Some("roundabout") => 1,
        Some("mini_roundabout") => 2,
        _ => 0,
    }
}

/// Decode OSM travel direction for a road way into the persistent code:
/// 0 = two-way, 1 = single-direction forward, 2 = single-direction reverse
/// (`oneway=-1`/`reverse`). Recognized `oneway` values are `yes|true|1` and
/// `no|false|0` (the explicit `no` overrides every implicit case); anything
/// else (`alternating`, typos, empty) is not single-direction evidence. With
/// no usable explicit value, `junction=roundabout|mini_roundabout` and
/// `highway=motorway|motorway_link` imply forward per OSM convention.
/// Direction describes the mapped carriageway only — it is never a
/// traffic-count basis (a measurement's directional/both-direction meaning
/// belongs to the observing source, not to this tag).
/// [OSM oneway semantics](https://wiki.openstreetmap.org/wiki/Key:oneway)
pub fn oneway_direction(highway: &str, oneway: Option<&str>, junction: Option<&str>) -> u8 {
    match oneway {
        Some("yes" | "true" | "1") => return 1,
        Some("-1" | "reverse") => return 2,
        Some("no" | "false" | "0" | "alternating") => return 0,
        Some(value) if !value.is_empty() => return 0,
        _ => {}
    }
    if matches!(junction, Some("roundabout") | Some("mini_roundabout")) {
        3
    } else if matches!(highway, "motorway" | "motorway_link") {
        4
    } else {
        0
    }
}

/// Map access + motor_vehicle + vehicle tags to enum.
/// 0=yes/untagged, 1=private, 2=no, 3=destination, 4=motor_vehicle_no (legacy, unused by new extracts),
/// 5=permissive, 6=customers, 7=agricultural, 8=forestry.
///
/// Resolution: most specific key wins (`motor_vehicle` > `vehicle` > `access`) —
/// OSM's standard is that motor-vehicle-specific tags override generic access.
pub fn access_type(access: Option<&str>, motor_vehicle: Option<&str>, vehicle: Option<&str>) -> u8 {
    let resolved = motor_vehicle.or(vehicle).or(access);
    match resolved {
        Some("no") => 2,
        Some("private") => 1,
        Some("destination") => 3,
        Some("permissive") => 5,
        Some("customers") => 6,
        Some("agricultural") => 7,
        Some("forestry") => 8,
        _ => 0,
    }
}

/// Map railway service tag to enum.
/// 0=none (normal track), 1=yard, 2=siding, 3=spur, 4=crossover.
/// Pipeline uses this to reduce or zero traffic on service tracks.
pub fn rail_service_type(service: Option<&str>) -> u8 {
    match service {
        Some("yard") => 1,
        Some("siding") => 2,
        Some("spur") => 3,
        Some("crossover") => 4,
        _ => 0,
    }
}

/// Map `railway:traffic_mode` to enum: 0=unknown, 1=passenger, 2=freight, 3=mixed.
/// Planet census 2026-09-25 (planet-260831, 2.82 M railway=rail ways): mixed 207,660,
/// passenger 116,040, freight 91,353 ways; any other value (typos, `both`, `military`)
/// is unknown. `passenger_lines` is a track count, not a mode; `passenger=no` (4,768
/// ways) and `usage=freight` (1,930) are undocumented and negligible, so the mode tag
/// stands alone. The finalizer gives passenger-only lines no freight prior and
/// freight-only lines no passenger prior; measured evidence still wins over the tag.
pub fn rail_traffic_mode(traffic_mode: Option<&str>) -> u8 {
    match traffic_mode {
        Some("passenger") => 1,
        Some("freight") => 2,
        Some("mixed") => 3,
        _ => 0,
    }
}

/// Unified aeroway class used by both airport lines and airport areas.
/// 0=runway, 1=taxiway, 2=apron, 3=helipad, 4=heliport, 5=aerodrome,
/// 6=stopway, 7=airstrip, 255=other.
pub fn aeroway_type(tags: &Tags) -> u8 {
    match tags.get("aeroway").map(|s| s.as_str()) {
        Some("runway") => 0,
        Some("taxiway") => 1,
        Some("apron") => 2,
        Some("helipad") => 3,
        Some("aerodrome") => {
            if matches!(
                tags.get("aerodrome:type").map(|s| s.as_str()),
                Some("heliport")
            ) || tags.get("amenity").map(|s| s.as_str()) == Some("heliport")
            {
                4
            } else {
                5
            }
        }
        Some("stopway") => 6,
        Some("airstrip") => 7,
        _ if tags.get("amenity").map(|s| s.as_str()) == Some("heliport") => 4,
        _ => 255,
    }
}

#[cfg(test)]
#[path = "mappers_leisure_tests.rs"]
mod leisure_tests;
#[cfg(test)]
#[path = "mappers_oneway_direction_tests.rs"]
mod oneway_direction_tests;
#[cfg(test)]
#[path = "mappers_parse_maxspeed_kmh_tests.rs"]
mod parse_maxspeed_kmh_tests;
#[cfg(test)]
#[path = "mappers_parse_width_m_tests.rs"]
mod parse_width_m_tests;
#[cfg(test)]
#[path = "mappers_railway_tests.rs"]
mod railway_tests;

#[cfg(test)]
use super::values::*;

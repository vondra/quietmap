//! Stateless tag→enum/value mappers shared by way + node spill: leisure class /
//! capacity / opening-hours, road/rail/barrier/aeroway code mappers, and the
//! `width`/`maxspeed` parsers. Pure functions of their inputs — no extract state.

use super::Tags;
use crate::ids;

/// Map the leisure feature to its `sport` emission class (the ids in
/// [`ids`]). `sport=*` wins; otherwise the `leisure=*`/`amenity=*` kind
/// selects a default (playground, pool, pitch, outdoor seating, stadium).
/// Always returns a class (PITCH is the fallback).
pub fn leisure_sport_class(tags: &Tags) -> u8 {
    if let Some(sport) = tags.get("sport") {
        // Multi-value `sport=tennis;padel` → take the loudest by the static
        // anchor table; `>=` keeps the LAST maximum, mirroring max_by_key.
        let mut best: Option<(i64, u8)> = None;
        for token in sport.split(';') {
            let token = token.trim().to_ascii_lowercase();
            if let Some(c) = ids::leisure_sport_class_id(token.as_str()) {
                let loud = ids::leisure_loudness_anchor(c);
                if best.is_none_or(|(b, _)| loud >= b) {
                    best = Some((loud, c));
                }
            }
        }
        if let Some((_, c)) = best {
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
        if let Some((_, close)) = tok.split_once('-') {
            if let Some((h, _)) = close.split_once(':') {
                if let Ok(hh) = h.trim().parse::<u32>() {
                    latest_close = latest_close.max(hh);
                }
            }
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
        "light_rail" => 2,
        "narrow_gauge" => 3,
        "funicular" => 4,
        _ => 0,
    }
}

/// Map usage tag to rail usage enum. 0=main, 1=branch, 2=industrial.
pub fn rail_usage_type(usage: Option<&str>) -> u8 {
    match usage {
        Some("main") => 0,
        Some("branch") => 1,
        Some("industrial") => 2,
        _ => 0,
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

/// Parse OSM `width` tag to metres. Returns max across multi-stripe /
/// range fragments. Examples: `45;35;50` → 50, `45-50` → 50,
/// `45,5` → 45.5 (EU decimal), `147 ft` → 44.8.
pub fn parse_width_m(width: Option<&str>) -> Option<f32> {
    let raw = width?.trim();
    if raw.is_empty() {
        return None;
    }
    let lower = raw.to_ascii_lowercase();
    let parse_fragment = |s: &str| -> Option<f32> {
        let s = s.trim();
        let (numeric_part, is_feet) =
            if let Some(stripped) = s.strip_suffix("ft").or_else(|| s.strip_suffix(" ft")) {
                (stripped.trim(), true)
            } else if let Some(stripped) = s.strip_suffix("m").or_else(|| s.strip_suffix(" m")) {
                (stripped.trim(), false)
            } else {
                (s, false)
            };
        // Require a leading digit so non-numeric fragments like "ft" (from
        // splitting "60-ft" → ["60", "ft"]) return None instead of 0.
        if !numeric_part.starts_with(|c: char| c.is_ascii_digit()) {
            return None;
        }
        let numeric: String = numeric_part
            .chars()
            .take_while(|c| c.is_ascii_digit() || matches!(c, '.' | ','))
            .collect();
        let value: f32 = numeric.replace(',', ".").parse().ok()?;
        Some(if is_feet { value * 0.3048 } else { value })
    };

    // Split on `;` (multi-stripe) and `-` (range). NOT `,` — that's the
    // EU decimal separator (`45,5` = 45.5 m, not max(45, 5)).
    lower
        .split([';', '-'])
        .filter_map(parse_fragment)
        .reduce(f32::max)
}

/// `roads.arrow` keeps `speed_limit` as u8; `maxspeed=none` (derestricted,
/// e.g. German Autobahn) stores this sentinel and real limits clamp to 254
/// in `spill.rs` so they can never collide. Owned by the future
/// `noise-compute` transfer (see [`ids`]); duplicated here alone.
pub use crate::ids::SPEED_LIMIT_DERESTRICTED;

/// [`parse_maxspeed_kmh`] result for `maxspeed=none`. Outside the clamped
/// numeric range (≤ 400) by construction, so no real tag value can
/// produce it.
pub const MAXSPEED_NONE: u16 = u16::MAX;

/// Parse OSM `maxspeed` to km/h. Multi-value tags take the first
/// `;`-token (`"50;30"` → 50; conditional/lane variants are out of scope).
/// Units: bare number = km/h, `mph`, `knots`. `walk` → 10 km/h
/// (OSM-wiki convention for walking pace); `none` → [`MAXSPEED_NONE`];
/// `signals` / `variable` / zone tags / garbage → 0 (unknown → class
/// default downstream). Numeric values clamp to 400 km/h (above any
/// legal limit; guards typos like "999").
pub fn parse_maxspeed_kmh(raw: &str) -> u16 {
    let token = raw
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match token.as_str() {
        "none" => return MAXSPEED_NONE,
        "walk" => return 10,
        _ => {}
    }
    let numeric: String = token
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let Ok(value) = numeric.parse::<f64>() else {
        return 0; // "", "signals", "variable", "DE:urban", other garbage
    };
    let kmh = match token[numeric.len()..].trim() {
        "" | "km/h" | "kmh" | "kph" => value,
        "mph" => value * 1.609344,
        "knots" => value * 1.852,
        _ => return 0, // unknown unit
    };
    (kmh.round() as u32).min(400) as u16
}

#[cfg(test)]
mod parse_maxspeed_kmh_tests {
    use super::{parse_maxspeed_kmh, MAXSPEED_NONE};

    #[test]
    fn kmh_bare_int() {
        assert_eq!(parse_maxspeed_kmh("300"), 300);
        assert_eq!(parse_maxspeed_kmh("50 km/h"), 50);
        assert_eq!(parse_maxspeed_kmh(" 50 "), 50);
    }

    #[test]
    fn mph_converts() {
        assert_eq!(parse_maxspeed_kmh("70 mph"), 113);
        assert_eq!(parse_maxspeed_kmh("30 mph"), 48);
        assert_eq!(parse_maxspeed_kmh("30mph"), 48);
        assert_eq!(parse_maxspeed_kmh("125 mph"), 201);
    }

    #[test]
    fn knots_converts() {
        assert_eq!(parse_maxspeed_kmh("10 knots"), 19);
    }

    #[test]
    fn none_returns_sentinel() {
        assert_eq!(parse_maxspeed_kmh("none"), MAXSPEED_NONE);
    }

    #[test]
    fn walk_is_10() {
        assert_eq!(parse_maxspeed_kmh("walk"), 10);
    }

    #[test]
    fn non_numeric_states_are_unknown() {
        assert_eq!(parse_maxspeed_kmh("signals"), 0);
        assert_eq!(parse_maxspeed_kmh("variable"), 0);
        assert_eq!(parse_maxspeed_kmh(""), 0);
        assert_eq!(parse_maxspeed_kmh("DE:urban"), 0);
        assert_eq!(parse_maxspeed_kmh("50 apples"), 0);
    }

    #[test]
    fn multi_value_takes_first_token() {
        assert_eq!(parse_maxspeed_kmh("50;30"), 50);
    }

    #[test]
    fn clamps_at_400() {
        assert_eq!(parse_maxspeed_kmh("999"), 400);
    }
}

#[cfg(test)]
mod parse_width_m_tests {
    use super::parse_width_m;

    fn near(a: Option<f32>, b: f32) -> bool {
        a.is_some_and(|v| (v - b).abs() < 0.01)
    }

    #[test]
    fn integer_metres() {
        assert!(near(parse_width_m(Some("45")), 45.0));
    }

    #[test]
    fn decimal_metres_dot() {
        assert!(near(parse_width_m(Some("45.5")), 45.5));
    }

    #[test]
    fn decimal_metres_comma_eu() {
        assert!(near(parse_width_m(Some("45,5")), 45.5));
    }

    #[test]
    fn explicit_metre_suffix() {
        assert!(near(parse_width_m(Some("45 m")), 45.0));
        assert!(near(parse_width_m(Some("45m")), 45.0));
    }

    #[test]
    fn feet_suffix_converts_to_metres() {
        assert!(near(parse_width_m(Some("147 ft")), 147.0 * 0.3048));
        assert!(near(parse_width_m(Some("147ft")), 147.0 * 0.3048));
    }

    #[test]
    fn multi_stripe_takes_max() {
        assert!(near(parse_width_m(Some("45;35;50")), 50.0));
    }

    #[test]
    fn range_takes_max() {
        assert!(near(parse_width_m(Some("45-50")), 50.0));
    }

    #[test]
    fn mixed_units_in_multi_stripe() {
        // 150 ft = 45.72 m, max of (45.72, 50) is 50.
        assert!(near(parse_width_m(Some("150 ft;50")), 50.0));
    }

    #[test]
    fn ambiguous_unit_in_range_returns_none_for_unit_fragment() {
        // "60-ft" splits to ["60", "ft"]; "ft" is non-numeric → None.
        // Only the "60" fragment yields a value (60 m, no conversion).
        // Acoustically: bare "60-ft" is malformed OSM; the metre reading
        // is at most ~30% over and bounded by typical runway widths.
        assert!(near(parse_width_m(Some("60-ft")), 60.0));
    }

    #[test]
    fn empty_returns_none() {
        assert_eq!(parse_width_m(Some("")), None);
        assert_eq!(parse_width_m(None), None);
    }

    #[test]
    fn non_numeric_returns_none() {
        assert_eq!(parse_width_m(Some("NA")), None);
        assert_eq!(parse_width_m(Some("unknown")), None);
    }
}

#[cfg(test)]
mod leisure_tests {
    use super::*;
    use crate::classify::ways::is_leisure_area;
    use crate::ids as lz;

    fn tags(pairs: &[(&str, &str)]) -> Tags {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn leisure_sport_class_resolution() {
        assert_eq!(
            leisure_sport_class(&tags(&[("sport", "padel")])),
            lz::LEISURE_PADEL
        );
        assert_eq!(
            leisure_sport_class(&tags(&[("leisure", "playground")])),
            lz::LEISURE_PLAYGROUND
        );
        assert_eq!(
            leisure_sport_class(&tags(&[("amenity", "biergarten")])),
            lz::LEISURE_OUTDOOR_SEATING
        );
        assert_eq!(
            leisure_sport_class(&tags(&[("leisure", "pitch")])),
            lz::LEISURE_PITCH
        );
        // Multi-value sport → loudest (padel > tennis).
        assert_eq!(
            leisure_sport_class(&tags(&[("sport", "tennis;padel")])),
            lz::LEISURE_PADEL
        );
    }

    #[test]
    fn leisure_area_gating() {
        assert!(is_leisure_area(&[("leisure", "playground")]));
        assert!(is_leisure_area(&[("leisure", "pitch")]));
        assert!(is_leisure_area(&[("amenity", "biergarten")]));
        assert!(is_leisure_area(&[("outdoor_seating", "yes")]));
        // Private back-yard pool (no access/public) is NOT a source.
        assert!(!is_leisure_area(&[("leisure", "swimming_pool")]));
        // Public pool is.
        assert!(is_leisure_area(&[
            ("leisure", "swimming_pool"),
            ("access", "public")
        ]));
        // Parks/gardens are not sources.
        assert!(!is_leisure_area(&[("leisure", "garden")]));
    }

    #[test]
    fn opening_hours_day_fraction() {
        assert_eq!(opening_hours_fraction(Some("24/7")), 1);
        assert_eq!(opening_hours_fraction(Some("Mo-Fr 08:00-18:00")), 2);
        assert_eq!(opening_hours_fraction(Some("Mo-Su 11:00-23:00")), 3); // late close
        assert_eq!(opening_hours_fraction(Some("Tu-Su 18:00-02:00")), 3); // after-midnight
        assert_eq!(opening_hours_fraction(None), 0);
        assert_eq!(opening_hours_fraction(Some("")), 0);
    }
}

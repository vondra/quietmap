//! Industrial and power tag values used by the common extracted-site format.

use super::*;

pub(super) fn site_type_from_tags(tags: &Tags) -> u8 {
    let tag = |key: &str| tags.get(key).map(String::as_str);
    // A mapped facility describes its activity more precisely than its land use.
    match (tag("man_made"), tag("railway"), tag("landuse")) {
        (Some("wastewater_plant"), _, _) => 4,
        (Some("works"), _, _) => 3,
        (_, Some("yard"), _) => 5,
        (_, _, Some("quarry")) => 1,
        (_, _, Some("farmyard")) => 2,
        _ => 0,
    }
}

/// Classify industrial site subtype from OSM `industrial=*` and `product=*` tags.
/// Values: 0 unknown (93 dB) · 1 warehouse (75) · 2 factory (95) · 3 mine (99) ·
/// 4 chemical (90) · 5 cement (100) · 6 metal (100) · 7 food (88) · 8 wood (90) ·
/// 9 waste (93) · 10 farm (70) · 11 office (60) · 12 port (92).
pub(super) fn site_subtype_from_tags(tags: &Tags) -> u8 {
    // Check industrial=* tag (most specific)
    if let Some(ind) = tags.get("industrial") {
        match ind.as_str() {
            "warehouse" | "depot" | "logistics" => return 1,
            "factory" | "manufacturing" => return 2,
            "mine" | "quarry" | "open_pit" => return 3,
            "oil" | "chemical" | "refinery" | "gas" => return 4,
            "cement" | "glass" | "brickyard" | "ceramics" => return 5,
            "steelmaking" | "smelting" | "foundry" | "metal" | "aluminium" | "iron" => return 6,
            "brewery" | "winery" | "distillery" | "bakery" | "food" | "slaughterhouse"
            | "sugar" => return 7,
            "sawmill" | "timber" | "lumber" | "paper" | "pulp" | "woodworking" => return 8,
            "scrap_yard" | "recycling" | "waste" | "landfill" => return 9,
            "farm" | "agriculture" | "horticulture" | "greenhouse" => return 10,
            "port" | "shipyard" | "boatyard" | "dock" => return 12,
            _ => {}
        }
    }
    // Check product=* tag
    if let Some(prod) = tags.get("product") {
        let p = prod.to_lowercase();
        if p.contains("cement")
            || p.contains("concrete")
            || p.contains("brick")
            || p.contains("glass")
            || p.contains("ceramic")
            || p.contains("tile")
        {
            return 5;
        }
        if p.contains("steel")
            || p.contains("iron")
            || p.contains("alumin")
            || p.contains("copper")
            || p.contains("metal")
            || p.contains("zinc")
        {
            return 6;
        }
        if p.contains("chemical")
            || p.contains("petrol")
            || p.contains("oil")
            || p.contains("fuel")
            || p.contains("plastic")
            || p.contains("fertiliz")
        {
            return 4;
        }
        if p.contains("food")
            || p.contains("sugar")
            || p.contains("beer")
            || p.contains("wine")
            || p.contains("flour")
            || p.contains("dairy")
            || p.contains("meat")
        {
            return 7;
        }
        if p.contains("wood")
            || p.contains("paper")
            || p.contains("timber")
            || p.contains("lumber")
            || p.contains("pulp")
        {
            return 8;
        }
    }
    // NOTE: wastewater_plant intentionally NOT classified — it has a dedicated
    // source_type=4 profile (89 dB, 24/7) which is more accurate than waste subtype=9 (93 dB).
    if let Some(mm) = tags.get("man_made")
        && mm.as_str() == "works"
    {
        return 2;
    }
    if let Some(lu) = tags.get("landuse") {
        match lu.as_str() {
            "farmyard" | "farmland" => return 10,
            "quarry" => return 3,
            _ => {}
        }
    }
    if tags.get("office").is_some() {
        return 11;
    }
    0 // unknown — will use default 93 dB
}

/// OSM canonical form is a bare number or "N m" (metres). Strip at most ONE
/// metre suffix — `trim_end_matches` would strip repeatedly and turn
/// "2000mm" into 2000 metres; unsupported units now fail the parse and fall
/// back to the caller's default instead of misparsing.
pub(super) fn parse_height(val: &str) -> Option<f32> {
    let val = val.trim_end();
    val.strip_suffix('m').unwrap_or(val).trim_end().parse().ok()
}

pub(super) fn parse_power_kw(val: Option<&str>) -> f32 {
    let v = val.unwrap_or("0");
    if let Some(mw) = v.strip_suffix(" MW").or_else(|| v.strip_suffix("MW")) {
        mw.parse::<f32>().unwrap_or(0.0) * 1000.0
    } else if let Some(kw) = v.strip_suffix(" kW").or_else(|| v.strip_suffix("kW")) {
        kw.parse::<f32>().unwrap_or(0.0)
    } else if let Some(w) = v.strip_suffix(" W").or_else(|| v.strip_suffix("W")) {
        w.parse::<f32>().unwrap_or(0.0) / 1000.0
    } else {
        v.parse::<f32>().unwrap_or(0.0)
    }
}

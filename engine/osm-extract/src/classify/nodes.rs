//! Node classification + node tag extraction: standalone OSM nodes that are
//! noise sources (POI footprint-join, leisure points, wind turbines, airports)
//! and the per-kind key sets they carry into spill. Mirrors [`Node`]/
//! [`DenseNode`] because `osmpbf` exposes the two as distinct tag iterators.

use super::ways::is_leisure_area;
use super::{scope_keeps, FeatureType, Tags};
use osmpbf::{DenseNode, Node};

/// Keys that make a NODE a function POI for the finalize footprint join, or a
/// standalone leisure-area node. Detection is value-aware (only noise-relevant
/// values) to avoid keeping all 33.8 M amenity nodes — the class is
/// resolved later from the spilled tags.
fn node_settlement_kind<'a>(tags: impl Iterator<Item = (&'a str, &'a str)>) -> Option<FeatureType> {
    let mut amenity = None;
    let mut shop = None;
    let mut tourism = None;
    let mut healthcare = None;
    let mut leisure = None;
    let mut outdoor_seating = None;
    let mut access = None;
    let mut sport = None;
    for (k, v) in tags {
        match k {
            "amenity" => amenity = Some(v),
            "shop" => shop = Some(v),
            "tourism" => tourism = Some(v),
            "healthcare" => healthcare = Some(v),
            "leisure" => leisure = Some(v),
            "outdoor_seating" => outdoor_seating = Some(v),
            "access" => access = Some(v),
            "sport" => sport = Some(v),
            _ => {}
        }
    }
    // Leisure-area node (own footprint) takes priority over the join POI: a
    // `leisure=playground` point IS a source, not a reclassifier. Shares the
    // `is_leisure_area` way-gate so a `swimming_pool` node is held to the SAME
    // public/access bar as a way (most pool nodes are private back-yards).
    let leisure_tags: Vec<(&str, &str)> = [
        amenity.map(|v| ("amenity", v)),
        leisure.map(|v| ("leisure", v)),
        outdoor_seating.map(|v| ("outdoor_seating", v)),
        access.map(|v| ("access", v)),
        sport.map(|v| ("sport", v)),
    ]
    .into_iter()
    .flatten()
    .collect();
    if is_leisure_area(&leisure_tags) {
        return Some(FeatureType::Leisure);
    }
    // Function POI for the join — only if some key carries a noise-relevant value.
    if crate::spill::poi_class(amenity, shop, healthcare, tourism).is_some() {
        return Some(FeatureType::Poi);
    }
    None
}

pub fn node_kind_node(node: &Node) -> Option<FeatureType> {
    let ft = node_settlement_kind(node.tags())?;
    scope_keeps(&ft).then_some(ft)
}

pub fn node_kind_dense(node: &DenseNode) -> Option<FeatureType> {
    let ft = node_settlement_kind(node.tags())?;
    scope_keeps(&ft).then_some(ft)
}

/// Tags a POI/leisure node carries into spill (superset of both kinds' needs).
const NODE_SETTLEMENT_KEYS: &[&str] = &[
    "amenity",
    "shop",
    "healthcare",
    "tourism",
    "leisure",
    "sport",
    "outdoor_seating",
    "access",
    "name",
    "capacity",
    "seats",
    "opening_hours",
];

fn extract_node_settlement_tags<'a>(tags: impl Iterator<Item = (&'a str, &'a str)>) -> Tags {
    let mut t = Tags::new();
    for (k, v) in tags {
        if NODE_SETTLEMENT_KEYS.contains(&k) {
            t.insert(k.to_string(), v.to_string());
        }
    }
    t
}

pub fn extract_node_settlement_tags_node(node: &Node) -> Tags {
    extract_node_settlement_tags(node.tags())
}

pub fn extract_node_settlement_tags_dense(node: &DenseNode) -> Tags {
    extract_node_settlement_tags(node.tags())
}

pub fn is_wind_turbine_node(node: &Node) -> bool {
    scope_keeps(&FeatureType::WindTurbine)
        && node.tags().any(|(k, v)| {
            (k == "generator:source" && v == "wind") || (k == "man_made" && v == "wind_turbine")
        })
}

pub fn is_wind_turbine_dense(node: &DenseNode) -> bool {
    scope_keeps(&FeatureType::WindTurbine)
        && node.tags().any(|(k, v)| {
            (k == "generator:source" && v == "wind") || (k == "man_made" && v == "wind_turbine")
        })
}

pub fn extract_turbine_tags_node(node: &Node) -> Tags {
    let mut t = Tags::new();
    for (k, v) in node.tags() {
        if matches!(
            k,
            "name"
                | "height"
                | "generator:output:electricity"
                | "rotor:diameter"
                | "generator:source"
                | "man_made"
        ) {
            t.insert(k.to_string(), v.to_string());
        }
    }
    t
}

pub fn extract_turbine_tags_dense(node: &DenseNode) -> Tags {
    let mut t = Tags::new();
    for (k, v) in node.tags() {
        if matches!(
            k,
            "name"
                | "height"
                | "generator:output:electricity"
                | "rotor:diameter"
                | "generator:source"
                | "man_made"
        ) {
            t.insert(k.to_string(), v.to_string());
        }
    }
    t
}

pub fn is_airport_node(node: &Node) -> bool {
    scope_keeps(&FeatureType::AirportArea)
        && node.tags().any(|(k, v)| {
            (k == "aeroway" && matches!(v, "helipad" | "aerodrome"))
                || (k == "amenity" && v == "heliport")
        })
}

pub fn is_airport_dense(node: &DenseNode) -> bool {
    scope_keeps(&FeatureType::AirportArea)
        && node.tags().any(|(k, v)| {
            (k == "aeroway" && matches!(v, "helipad" | "aerodrome"))
                || (k == "amenity" && v == "heliport")
        })
}

pub fn extract_airport_tags_node(node: &Node) -> Tags {
    let mut t = Tags::new();
    for (k, v) in node.tags() {
        if matches!(
            k,
            "aeroway"
                | "name"
                | "ref"
                | "local_ref"
                | "icao"
                | "iata"
                | "operator"
                | "surface"
                | "width"
                | "access"
                | "aerodrome"
                | "aerodrome:type"
                | "amenity"
        ) {
            t.insert(k.to_string(), v.to_string());
        }
    }
    t
}

pub fn extract_airport_tags_dense(node: &DenseNode) -> Tags {
    let mut t = Tags::new();
    for (k, v) in node.tags() {
        if matches!(
            k,
            "aeroway"
                | "name"
                | "ref"
                | "local_ref"
                | "icao"
                | "iata"
                | "operator"
                | "surface"
                | "width"
                | "access"
                | "aerodrome"
                | "aerodrome:type"
                | "amenity"
        ) {
            t.insert(k.to_string(), v.to_string());
        }
    }
    t
}

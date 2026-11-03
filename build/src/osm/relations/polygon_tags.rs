//! Classify relation-level activity tags independently of their member ways.

use super::*;

/// Decide whether a relation is one of the multipolygon flavours we track.
pub(super) fn classify_multipolygon(rel: &osmpbf::Relation) -> Option<(Vec<FeatureType>, Tags)> {
    let tags: Tags = rel
        .tags()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let tag = |k: &str| tags.get(k).map(String::as_str);
    if tag("type") != Some("multipolygon") {
        return None;
    }
    // A car park is what it is before it is what it is zoned as, and a lot mapped
    // as a relation is not a source: the assembler keeps one outer ring and drops
    // the holes, so an assembled lot would swallow the buildings inside it.
    let parking = crate::osm::classify::parking_kind(tag);
    let ftype = if crate::osm::classify::has_a_building(tag)
        || matches!(
            parking,
            Some(
                crate::osm::classify::ParkingKind::Structure
                    | crate::osm::classify::ParkingKind::Underground
            )
        ) {
        FeatureType::Building
    } else if crate::osm::classify::is_special_leisure(tag) {
        FeatureType::Leisure
    } else if parking.is_some() {
        // Open ground, a stall or a rooftop: not a relation source.
        return None;
    } else if matches!(
        tag("landuse"),
        Some("industrial" | "quarry" | "farmyard" | "landfill" | "port" | "harbour")
    ) || crate::osm::classify::is_railway_yard(tag)
        || matches!(tag("man_made"), Some("works") | Some("wastewater_plant"))
        || crate::osm::classify::is_power_or_inactive_industry(tag)
    {
        FeatureType::Industrial
    } else if matches!(
        tag("aeroway"),
        Some("runway" | "taxiway" | "apron" | "helipad" | "aerodrome" | "stopway" | "airstrip")
    ) || tag("amenity") == Some("heliport")
    {
        FeatureType::AirportArea
    } else if crate::osm::classify::is_functional_area(tag) {
        // Functional AREA relation with no building tag: a mall (shop=mall),
        // hospital or school campus, retail/commercial zone (audit 2026-06).
        FeatureType::Building
    } else {
        return None;
    };
    let types = crate::osm::classify::feature_types(ftype, tag);
    (!types.is_empty()).then_some((types, tags))
}

/// Tags an assembled multipolygon carries into spill. Copied from relation
/// tags (not member ways) so a hospital MP without `building=*` still classifies.
pub fn spill_tags_for_assembled(ftype: &FeatureType, tags: &Tags) -> Tags {
    crate::osm::classify::extract_tags(tags.iter().map(|(k, v)| (k.as_str(), v.as_str())), ftype)
}

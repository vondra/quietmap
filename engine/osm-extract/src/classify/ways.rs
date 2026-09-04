//! Way classification + way tag extraction: maps an OSM `Way`'s tags to a
//! [`FeatureType`] and pulls the per-family keys it carries into spill.

use super::{scope_keeps, FeatureType, Tags};
use osmpbf::Way;

/// Classify a way by its tags. Returns None if not noise-relevant
/// (or out of the `QM_OSM_ONLY` scope).
pub fn classify_way(way: &Way) -> Option<FeatureType> {
    let ft = classify_way_unscoped(way)?;
    scope_keeps(&ft).then_some(ft)
}

fn classify_way_unscoped(way: &Way) -> Option<FeatureType> {
    let tags = way.tags().collect::<Vec<_>>();
    let tag = |k: &str| tags.iter().find(|(key, _)| *key == k).map(|(_, v)| *v);

    // Road
    if let Some(
        "motorway" | "trunk" | "primary" | "secondary" | "tertiary" | "residential"
        | "living_street" | "service" | "track" | "unclassified" | "motorway_link" | "trunk_link"
        | "primary_link" | "secondary_link" | "tertiary_link",
    ) = tag("highway")
    {
        return Some(FeatureType::Road);
    }

    // Railway
    if let Some("rail" | "tram" | "light_rail" | "narrow_gauge" | "funicular") = tag("railway") {
        return Some(FeatureType::Railway);
    }

    // Areas = airfield polygons (apron / helipad / aerodrome +
    // closed-ring runway/taxi). Lines = the aeroway network downstream
    // snaps ADS-B legs onto. `airstrip` covers unlicensed grass/private
    // strips (common on small aerodromes). Disused/lifecycle prefixes
    // are intentionally excluded — zero traffic, zero emission.
    if let Some(aeroway) = tag("aeroway") {
        match aeroway {
            "apron" | "helipad" | "aerodrome" => return Some(FeatureType::AirportArea),
            "runway" | "taxiway" | "stopway" | "airstrip" => {
                return Some(FeatureType::AirportLine);
            }
            _ => {}
        }
    }
    if tag("amenity") == Some("heliport") {
        return Some(FeatureType::AirportArea);
    }

    // Noise barrier
    if let Some(barrier) = tag("barrier") {
        if barrier == "noise_barrier" || barrier == "sound_barrier" {
            return Some(FeatureType::Barrier);
        }
    }
    if tag("wall") == Some("noise_barrier") || tag("man_made") == Some("noise_barrier") {
        return Some(FeatureType::Barrier);
    }

    // Wind turbine (way — rare but possible as closed polygon)
    if tag("generator:source") == Some("wind") || tag("man_made") == Some("wind_turbine") {
        return Some(FeatureType::WindTurbine);
    }

    // Building (takes priority over leisure: a sports_centre tagged building=*
    // is a roofed building, not an open-air area source).
    if tag("building").is_some() {
        return Some(FeatureType::Building);
    }

    // Leisure AREA (settlement v2 phase 2) — open-air activity sources with no
    // building tag. `swimming_pool` is gated to public/large because the key is
    // dominated by roughly 3 million private back-yard pools.
    if is_leisure_area(&tags) {
        return Some(FeatureType::Leisure);
    }

    // Industrial landuse + power infrastructure (audit 2026-06: substations /
    // landfills / ports were vanishing).
    if let Some("industrial" | "quarry" | "farmyard" | "landfill" | "port" | "harbour") =
        tag("landuse")
    {
        return Some(FeatureType::Industrial);
    }
    if let Some("works" | "wastewater_plant") = tag("man_made") {
        return Some(FeatureType::Industrial);
    }
    if let Some("plant" | "substation") = tag("power") {
        return Some(FeatureType::Industrial);
    }

    // Functional AREA with no `building` tag IS a noise source: a mall master
    // polygon (shop=mall), a hospital ground, a school yard, a retail/commercial
    // zone. FUNCTION is the gate, not just `building=` (audit 2026-06). Reuses
    // poi_class so the area classifies identically to the same function on a
    // building; the overlap with sub-buildings inside it is suppressed in
    // finalize (an area containing real buildings defers to them).
    if is_functional_area(tag) {
        return Some(FeatureType::Building);
    }

    None
}

/// True if a tag set is a functional building AREA worth keeping even without a
/// `building` tag: a noise-relevant `amenity`/`shop`/`healthcare`/`tourism` POI,
/// or a retail/commercial landuse zone. Takes a `tag` lookup so way + relation
/// routing share ONE definition (each already has its own closure).
pub(crate) fn is_functional_area<'a>(tag: impl Fn(&str) -> Option<&'a str>) -> bool {
    crate::spill::poi_class(
        tag("amenity"),
        tag("shop"),
        tag("healthcare"),
        tag("tourism"),
    )
    .is_some()
        || matches!(tag("landuse"), Some("retail" | "commercial"))
}

/// Observability: for a way that classified to `None` (vanished from the map),
/// the noise-relevant tag it carried that arguably SHOULD route it but currently
/// doesn't. Drives the extract's fall-through report so a silent gap is visible.
/// Returns `None` for genuinely irrelevant ways (the overwhelming majority).
pub fn fallthrough_reason(way: &Way) -> Option<String> {
    let tags = way.tags().collect::<Vec<_>>();
    let tag = |k: &str| tags.iter().find(|(key, _)| *key == k).map(|(_, v)| *v);
    // Car parks are vehicle sources (Parkplatzlärm), a separate concern from the
    // building layer — don't report them as a building gap.
    if matches!(
        tag("amenity"),
        Some("parking" | "parking_space" | "parking_entrance")
    ) {
        return None;
    }
    // A functional POI as an AREA (no building wrapper) — mall / hospital / school.
    if crate::spill::poi_class(
        tag("amenity"),
        tag("shop"),
        tag("healthcare"),
        tag("tourism"),
    )
    .is_some()
    {
        let kind = tag("shop")
            .map(|v| format!("shop={v}"))
            .or_else(|| tag("amenity").map(|v| format!("amenity={v}")))
            .unwrap_or_else(|| "poi".into());
        return Some(kind);
    }
    if let Some(v @ ("retail" | "commercial" | "port" | "harbour" | "landfill")) = tag("landuse") {
        return Some(format!("landuse={v}"));
    }
    if let Some(v @ ("plant" | "substation")) = tag("power") {
        return Some(format!("power={v}"));
    }
    None
}

/// True if a tag set describes an open-air leisure AREA source (no `building`).
/// `swimming_pool` is gated to `access=public/yes` or `sport=swimming`/
/// `swimming_area` to drop the roughly 3 million private back-yard pools.
pub(super) fn is_leisure_area(tags: &[(&str, &str)]) -> bool {
    let tag = |k: &str| tags.iter().find(|(key, _)| *key == k).map(|(_, v)| *v);
    if tag("amenity") == Some("biergarten") {
        return true;
    }
    if tag("outdoor_seating").is_some_and(|v| v != "no") {
        return true;
    }
    match tag("leisure") {
        Some(
            "playground" | "pitch" | "sports_centre" | "sports_hall" | "stadium" | "track"
            | "outdoor_seating" | "dog_park",
        ) => true,
        Some("swimming_pool" | "swimming_area" | "water_park") => {
            matches!(tag("access"), Some("public" | "yes")) || tag("sport") == Some("swimming")
        }
        _ => false,
    }
}

/// Extract relevant tags from a way.
pub fn extract_way_tags(way: &Way, ftype: &FeatureType) -> Tags {
    let mut t = Tags::new();
    for (k, v) in way.tags() {
        match ftype {
            FeatureType::Road => {
                if matches!(
                    k,
                    "highway"
                        | "name"
                        | "ref"
                        | "maxspeed"
                        | "surface"
                        | "oneway"
                        | "lanes"
                        | "bridge"
                        | "tunnel"
                        | "toll"
                        | "lit"
                        | "junction"
                        | "access"
                        | "motor_vehicle"
                        | "vehicle"
                ) {
                    t.insert(k.to_string(), v.to_string());
                }
            }
            FeatureType::Railway => {
                if matches!(
                    k,
                    "railway"
                        | "name"
                        | "ref"
                        | "usage"
                        | "maxspeed"
                        | "electrified"
                        | "gauge"
                        | "operator"
                        | "bridge"
                        | "tunnel"
                        | "service"
                        | "highspeed"
                ) {
                    t.insert(k.to_string(), v.to_string());
                }
            }
            FeatureType::AirportArea => {
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
            FeatureType::AirportLine => {
                // Identity (icao / iata / operator / name) flows from
                // the parent aerodrome polygon — line ways rarely carry it.
                if matches!(k, "aeroway" | "ref" | "surface" | "width") {
                    t.insert(k.to_string(), v.to_string());
                }
            }
            FeatureType::Building => {
                // Extract amenity/shop/healthcare/tourism for better classification.
                // WHY: Many buildings are tagged building=yes but have amenity=school,
                // shop=supermarket, etc. Without these tags, schools get classified as
                // residential (type 0) and get wrong emission profile.
                if matches!(
                    k,
                    "building"
                        | "building:use"
                        | "height"
                        | "building:levels"
                        | "name"
                        | "addr:street"
                        | "addr:housenumber"
                        | "amenity"
                        | "shop"
                        | "healthcare"
                        | "tourism"
                        | "leisure"
                        // settlement v2 phase 2: livestock rescues farm_auxiliary
                        // from SILENT; opening_hours → day-fraction.
                        | "animal"
                        | "livestock"
                        | "opening_hours"
                ) {
                    t.insert(k.to_string(), v.to_string());
                }
            }
            FeatureType::Leisure => {
                if matches!(
                    k,
                    "leisure"
                        | "sport"
                        | "amenity"
                        | "outdoor_seating"
                        | "access"
                        | "name"
                        | "capacity"
                        | "seats"
                        | "opening_hours"
                ) {
                    t.insert(k.to_string(), v.to_string());
                }
            }
            // Poi is node-only; ways never classify to it.
            FeatureType::Poi => {}
            FeatureType::Industrial | FeatureType::WindTurbine => {
                if matches!(
                    k,
                    "landuse"
                        | "man_made"
                        | "name"
                        | "operator"
                        | "product"
                        | "industrial"
                        | "generator:source"
                        | "height"
                        | "generator:output:electricity"
                        | "rotor:diameter"
                        | "opening_hours"
                ) {
                    t.insert(k.to_string(), v.to_string());
                }
            }
            FeatureType::Barrier => {
                if matches!(k, "height" | "material" | "barrier") {
                    t.insert(k.to_string(), v.to_string());
                }
            }
        }
    }
    t
}

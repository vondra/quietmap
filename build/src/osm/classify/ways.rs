//! Way classification + way tag extraction: maps an OSM `Way`'s tags to a
//! [`FeatureType`] and pulls the per-family keys it carries into spill.

use super::{FeatureType, is_power_or_inactive_industry, is_railway_yard, is_special_leisure};
use osmpbf::Way;

/// Primary kind is shared by the junction census and selected-node census.
pub fn classify_way(way: &Way) -> Option<FeatureType> {
    classify_way_types(way).into_iter().next()
}

pub fn classify_way_types(way: &Way) -> Vec<FeatureType> {
    let Some(primary) = classify_primary_way(way) else {
        return Vec::new();
    };
    feature_types(primary, |key| {
        way.tags().find(|(k, _)| *k == key).map(|(_, v)| v)
    })
}

pub fn feature_types<'a>(
    primary: FeatureType,
    tag: impl Fn(&str) -> Option<&'a str>,
) -> Vec<FeatureType> {
    let mut types = Vec::new();
    if primary == FeatureType::Building {
        if is_special_leisure(&tag) {
            types.push(FeatureType::Leisure);
        }
        if tag("power") != Some("generator") && is_power_or_inactive_industry(&tag) {
            types.push(FeatureType::Industrial);
        }
    }
    types.insert(0, primary);
    types
}

pub(crate) fn classify_primary_way(way: &Way) -> Option<FeatureType> {
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

    // Railway. Subway is included for its above-ground sections; the spill marks
    // its underground sections as tunnel, which emits nothing.
    if super::railway_carries_trains(tag("railway"), tag("disused"), tag("abandoned")) {
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
    if let Some(barrier) = tag("barrier")
        && (barrier == "noise_barrier" || barrier == "sound_barrier")
    {
        return Some(FeatureType::Barrier);
    }
    if tag("wall") == Some("noise_barrier") || tag("man_made") == Some("noise_barrier") {
        return Some(FeatureType::Barrier);
    }

    // Building (takes priority over leisure: a sports_centre tagged building=*
    // is a roofed building, not an open-air area source).
    if has_a_building(tag) {
        return Some(FeatureType::Building);
    }

    // Wind turbine (way — rare but possible as closed polygon)
    if super::is_turbine(tag) {
        return Some(FeatureType::WindTurbine);
    }

    // Car park with no `building` tag, decided here and not by whatever else the
    // polygon is zoned as. Open ground emits in the open-air lane, but only where
    // OSM mapped its AREA: `parking=lane` and many `street_side` strips are drawn
    // as LINES, and a line's shoelace area would invent a lot out of the block it
    // runs along. A deck or a basement keeps the building lane, where it screens
    // or, under the ground, only emits. A parking NODE has no area at all and
    // stays the function POI it always was.
    match parking_kind(tag) {
        Some(ParkingKind::OpenLot | ParkingKind::OpenStrip) => {
            return is_a_closed_ring(&way.refs().collect::<Vec<_>>())
                .then_some(FeatureType::Leisure);
        }
        Some(ParkingKind::Structure | ParkingKind::Underground) => {
            return Some(FeatureType::Building);
        }
        Some(ParkingKind::NotItsOwnSource) => return None,
        None => {}
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
    // Railway yards emit as rail-yard facilities. Track ways (railway=rail /
    // tram / …) return Railway above; only a mistagged linear railway=yard
    // would land here, as a point facility.
    if is_railway_yard(tag) {
        return Some(FeatureType::Industrial);
    }
    if let Some("works" | "wastewater_plant") = tag("man_made") {
        return Some(FeatureType::Industrial);
    }
    if is_power_or_inactive_industry(tag) {
        return Some(FeatureType::Industrial);
    }

    // Functional AREA with no `building` tag IS a noise source, and — a parking
    // deck aside, which stands — never an obstacle: a mall master polygon
    // (shop=mall), a hospital ground, a school yard, a retail/commercial zone.
    // FUNCTION is the gate, not just `building=`
    // (audit 2026-06). Reuses poi_class so the area classifies identically to
    // the same function on a building; the overlap with sub-buildings inside it
    // is suppressed in finalize (an area containing real buildings defers to them).
    if is_functional_area(tag) {
        return Some(FeatureType::Building);
    }

    None
}

/// True if a tag set is a functional AREA worth keeping as a building row even
/// without a `building` tag: a noise-relevant `amenity`/`shop`/`healthcare`/
/// `tourism` POI, or a retail/commercial landuse zone. Takes a `tag` lookup so
/// way + relation routing share ONE definition (each already has its own
/// closure).
pub(crate) fn is_functional_area<'a>(tag: impl Fn(&str) -> Option<&'a str>) -> bool {
    // A car park is decided by [`parking_kind`], which both routers ask first.
    // Saying so here as well costs one branch and keeps the answer right whoever
    // calls it: `poi_class` types a lot as class 7, and a lot is not a building.
    if parking_kind(&tag).is_some() {
        return false;
    }
    crate::osm::spill::poi_class(
        tag("amenity"),
        tag("shop"),
        tag("healthcare"),
        tag("tourism"),
    )
    .is_some()
        || matches!(tag("landuse"), Some("retail" | "commercial"))
}

/// `building=no` states that there is NO building here, so it must read as an
/// absent tag everywhere: it may not route a row by the tag's mere presence, and
/// it may not make one screen. What the polygon IS can still keep it in the
/// building lane — a deck says so with `parking=multi-storey` — but then it
/// emits without a wall. Anything else, `yes` included, is a building.
pub(crate) fn has_a_building<'a>(tag: impl Fn(&str) -> Option<&'a str>) -> bool {
    !matches!(tag("building"), None | Some("no"))
}

/// The OSM ring test: four vertices or more, the last one repeating the
/// first. An open-air AREA source is only as real as its ring — a line has no
/// area to scale its emission by, whatever the shoelace formula would compute.
pub(crate) fn is_a_closed_ring<T: PartialEq>(refs: &[T]) -> bool {
    refs.len() >= 4 && refs.first() == refs.last()
}

/// What an `amenity=parking*` area is, acoustically. One decision, read by the
/// way router, the relation router and the spill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParkingKind {
    /// Open ground: cars manoeuvre, doors shut and trolleys roll in the open, so
    /// it emits ([`crate::osm::ids::LEISURE_CAR_PARK`]), and nothing stands there, so
    /// it never screens.
    OpenLot,
    /// A street-side or lane strip: the same movements, packed into a strip that
    /// borrows the street as its aisle ([`crate::osm::ids::LEISURE_CAR_PARK_STREET`]).
    OpenStrip,
    /// A deck, a carport row or a block of garages: a STRUCTURE. It keeps the
    /// building lane — it screens, and its cars are inside it, not in the open.
    Structure,
    /// Below the ground: its vent fans emit, but nothing stands over it.
    Underground,
    /// One stall inside a lot (`amenity=parking_space`), or cars on a roof the
    /// ground-level area law cannot carry (`parking=rooftop`). Counting either
    /// would double something already counted.
    NotItsOwnSource,
}

/// [`ParkingKind`] of a tag set, or `None` when it is not a car park at all.
/// A `building=parking|garage|garages|carport` is one too: the building IS the
/// structure, and it can be the one below the ground.
pub(crate) fn parking_kind<'a>(tag: impl Fn(&str) -> Option<&'a str>) -> Option<ParkingKind> {
    let building_is_the_structure = matches!(
        tag("building"),
        Some("parking" | "garage" | "garages" | "carport")
    );
    match tag("amenity") {
        Some("parking") => {}
        Some("parking_space") => return Some(ParkingKind::NotItsOwnSource),
        _ if building_is_the_structure => {}
        _ => return None,
    }
    // Everything reaching this point IS a car park (`amenity=parking`, or a
    // building that is one), so `parking=underground` puts THIS object below the
    // ground. An ordinary building that merely has a basement garage carries no
    // `amenity=parking`, never reaches here, and keeps its wall.
    if tag("location") == Some("underground") || tag("parking") == Some("underground") {
        return Some(ParkingKind::Underground);
    }
    // A car park WITH a building — `building=parking`, or the plain `building=yes`
    // a multi-storey deck usually carries — is that building: it stands.
    if has_a_building(&tag) {
        return Some(ParkingKind::Structure);
    }
    Some(match tag("parking") {
        Some("multi-storey" | "carports" | "garage_boxes" | "sheds") => ParkingKind::Structure,
        Some("rooftop") => ParkingKind::NotItsOwnSource,
        Some("street_side" | "lane") => ParkingKind::OpenStrip,
        _ => ParkingKind::OpenLot,
    })
}

/// True if a tag set describes an open-air leisure AREA source (no `building`).
/// `swimming_pool` is gated to `access=public/yes` or `sport=swimming`/
/// `swimming_area` to drop the roughly 3 million private back-yard pools.
pub(super) fn is_leisure_area(tags: &[(&str, &str)]) -> bool {
    let tag = |k: &str| tags.iter().find(|(key, _)| *key == k).map(|(_, v)| *v);
    if is_special_leisure(tag) {
        return true;
    }
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

#[cfg(test)]
#[path = "ways_tests.rs"]
mod tests;

//! Building and POI class selection from explicit OSM tags.

use super::*;

/// Classify building type from all relevant OSM tags, not just building=*.
/// Priority: amenity/shop/healthcare/tourism > building tag.
/// WHY: OSM mappers often tag building=yes with function in amenity/shop.
///   Without this, a hospital tagged building=yes + amenity=hospital
///   would be classified as residential and get wrong emission profile.
///
/// `farm_auxiliary` co-tagged with a livestock signal is rescued from SILENT
/// back to farm (class 8) — many auxiliaries are sheds (→ SILENT) but a few are
/// barns. Function POIs reuse [`poi_class`] (shared with the finalize join).
pub(super) fn building_type_from_tags(tags: &Tags) -> u8 {
    let get = |k: &str| tags.get(k).map(|s| s.as_str());
    // Power infrastructure stays silent (its Industrial row owns the emission),
    // but a special-leisure building emits: SPEC silences the roofed formula
    // row BECAUSE its building footprint emits, so silencing both leaves an
    // indoor karting hall with no path at all.
    if classify::is_power_building(get) {
        return ids::SETTLEMENT_SILENT;
    }
    // A SPECIFIC structural `building=*` (warehouse, stadium, train_station, …)
    // describes the whole envelope and BEATS an amenity POI tagged inside it: a
    // `building=warehouse` + `amenity=bar` is a warehouse with a staff bar, not a
    // 100 dB "restaurant" the size of the warehouse. Generic envelopes
    // (yes/commercial/retail/house/…) are NOT structural and defer to the POI
    // below, which gives them their function (a `building=yes` + `amenity=cafe`
    // IS a café). Fixes Strahov Stadium / airport terminals / breweries that
    // were stamped restaurant_bar on a 10⁵ m² footprint.
    if let Some(b) = get("building")
        && building_overrides_poi(b)
    {
        return building_type(b);
    }
    if let Some(c) = poi_class(
        get("amenity"),
        get("shop"),
        get("healthcare"),
        get("tourism"),
    ) {
        return c;
    }
    // A retail or commercial LANDUSE zone carries no `building` tag and no POI:
    // it is the unmapped built area of a retail park or an office estate (where
    // buildings ARE mapped inside it, finalize suppresses the zone). Commercial
    // plant, not apartments — the residential fallback below billed every such
    // zone as dwellings, up to the 200-dwelling cap.
    if matches!(get("landuse"), Some("retail" | "commercial")) {
        return 1;
    }
    // farm_auxiliary + livestock → real farm building, not a silent shed.
    if get("building") == Some("farm_auxiliary")
        && (tags.contains_key("animal") || get("livestock").is_some())
    {
        return 8;
    }
    // Fallback to building tag
    tags.get("building").map(|s| building_type(s)).unwrap_or(0)
}

/// A building stands on the row: it screens with its footprint and height.
pub(crate) const GROUND_HOLDS_A_BUILDING: u8 = 0;
/// A functional AREA with no `building` tag — school ground, retail zone, mall
/// polygon. It emits from its ring and never screens, and because it WRAPS what
/// is mapped inside it, finalize drops it where real buildings stand there.
pub(crate) const GROUND_IS_A_FUNCTIONAL_AREA: u8 = 1;
/// A car park below the ground: its vent fans radiate and nothing stands over
/// it, but it wraps nothing — the buildings above it are not its sub-buildings,
/// so the wrapper suppression must leave it alone.
pub(crate) const GROUND_IS_BELOW_A_BUILDING: u8 = 2;

/// Which of the three [`GROUND_*`](GROUND_HOLDS_A_BUILDING) states a Building
/// row is in. `structures-builder-2` writes the two non-zero states without
/// geometry, with `height_m` 0 and no screening ordinal, so they emit from their
/// ring and screen nothing. A deck or a block of garages is NOT one of them: it
/// stands, even where OSM left the `building` tag off it.
pub(super) fn ground_state(tags: &Tags) -> u8 {
    let tag = |k: &str| tags.get(k).map(|s| s.as_str());
    if classify::parking_kind(tag) == Some(classify::ParkingKind::Underground) {
        return GROUND_IS_BELOW_A_BUILDING;
    }
    if !classify::has_a_building(tag) {
        // A deck or a block of garages stands even where OSM left the `building`
        // tag off it — unless the mapper wrote `building=no`, which says outright
        // that nothing stands here.
        if classify::parking_kind(tag) == Some(classify::ParkingKind::Structure)
            && tag("building").is_none()
        {
            return GROUND_HOLDS_A_BUILDING;
        }
        return GROUND_IS_A_FUNCTIONAL_AREA;
    }
    // The building itself is under the ground; `location` is the only tag that
    // says so about a building, and `parking=underground` on an ordinary one
    // only means it HAS a basement garage.
    if tag("location") == Some("underground") {
        GROUND_IS_BELOW_A_BUILDING
    } else {
        GROUND_HOLDS_A_BUILDING
    }
}

/// A `building=*` value naming a large / specific STRUCTURE whose emission is
/// set by the whole envelope, not by an `amenity` POI tagged inside it (a
/// stadium / warehouse / station / church with a bar is NOT a bar). Generic
/// envelopes (`yes`, `commercial`, `retail`, `office`, `residential`, `house`,
/// `apartments`) are deliberately ABSENT — they defer to the POI that gives them
/// their function.
pub(super) fn building_overrides_poi(b: &str) -> bool {
    matches!(
        b,
        "warehouse"
            | "industrial"
            | "manufacture"
            | "stadium"
            | "grandstand"
            | "sports_hall"
            | "sports_centre"
            | "train_station"
            | "transportation"
            | "hospital"
            | "school"
            | "university"
            | "college"
            | "kindergarten"
            | "church"
            | "cathedral"
            | "chapel"
            | "mosque"
            | "synagogue"
            | "temple"
            | "monastery"
            | "hangar"
            | "farm"
            | "barn"
            | "stable"
            | "cowshed"
    )
}

/// The emission class implied by a function POI (`amenity`/`shop`/`healthcare`/
/// `tourism`), or `None` if none of the values are noise-relevant. Shared by the
/// way-level `building_type_from_tags` and the finalize POI-in-footprint join so
/// a standalone `amenity=hospital` node and a `building=yes + amenity=hospital`
/// way classify identically. Returns the settlement class u8.
/// [`poi_class`] resolved from a node/way `Tags` map.
pub fn poi_class_from_tags(tags: &Tags) -> Option<u8> {
    poi_class(
        tags.get("amenity").map(|s| s.as_str()),
        tags.get("shop").map(|s| s.as_str()),
        tags.get("healthcare").map(|s| s.as_str()),
        tags.get("tourism").map(|s| s.as_str()),
    )
}

pub fn poi_class(
    amenity: Option<&str>,
    shop: Option<&str>,
    healthcare: Option<&str>,
    tourism: Option<&str>,
) -> Option<u8> {
    if let Some(a) = amenity {
        match a {
            "school" | "kindergarten" | "university" | "college" | "library" => return Some(3),
            "hospital" | "clinic" | "doctors" | "dentist" | "pharmacy" => return Some(4),
            "place_of_worship" => return Some(5),
            "restaurant" | "bar" | "pub" | "cafe" | "fast_food" | "food_court" | "ice_cream"
            | "nightclub" => return Some(ids::SETTLEMENT_HOSPITALITY),
            "fuel" | "car_wash" => return Some(1), // small-commercial placeholder (PROP-MEAS)
            "parking" | "parking_space" => return Some(ids::SETTLEMENT_PARKING_STRUCTURE),
            "fire_station" | "police" | "townhall" | "courthouse" | "post_office" => {
                return Some(9);
            }
            _ => {}
        }
    }
    if let Some(s) = shop {
        match s {
            "supermarket" | "convenience" | "wholesale" | "greengrocer" | "butcher" | "bakery"
            | "deli" | "frozen_food" | "mall" | "department_store" => {
                return Some(ids::SETTLEMENT_FOOD_RETAIL);
            }
            "" => {}
            _ => return Some(1), // any other shop = commercial
        }
    }
    if let Some(h) = healthcare
        && matches!(h, "hospital" | "clinic")
    {
        return Some(4);
    }
    if let Some(t) = tourism
        && matches!(t, "hotel" | "hostel" | "motel" | "guest_house")
    {
        return Some(6);
    }
    None
}

/// Map a raw `building=*` value to a settlement emission class (the u8s in
/// [`ids`], owned by the future `noise-compute` transfer). Single-family
/// houses → HOUSE (garden + heat pump), `building=supermarket` → FOOD_RETAIL,
/// restaurant/cafe buildings → HOSPITALITY, the silent tail (sheds, roofs,
/// huts — ~18 M objects that wrongly radiated residential) → SILENT.
/// `yes`/`""`/unknown stay 0 = residential-apartments.
pub(super) fn building_type(val: &str) -> u8 {
    match val {
        // Apartments / generic residential keep type 0 (no garden term).
        "residential" | "apartments" | "dormitory" => 0,
        // Single-family houses get the HOUSE class (garden + heat pump).
        "house" | "detached" | "semidetached_house" | "semidetached" | "terrace" | "bungalow"
        | "houseboat" | "cabin" => ids::SETTLEMENT_HOUSE,
        // Retail buildings — malls, shops, supermarkets — are SHOPPING activity
        // (deliveries + refrigeration; the car park is its own source now),
        // the FOOD_RETAIL profile, NOT
        // the office-HVAC commercial one.
        "supermarket" | "retail" => ids::SETTLEMENT_FOOD_RETAIL,
        "restaurant" | "cafe" | "pub" | "bar" | "fast_food" => ids::SETTLEMENT_HOSPITALITY,
        // Transport hubs: HVAC + concourse activity, commercial-grade plant.
        "commercial" | "office" | "kiosk" | "train_station" | "transportation" => 1,
        // data_center: 24/7 chiller plant, closer to a works than an office.
        "industrial" | "warehouse" | "manufacture" | "workshop" | "data_center" => 2,
        "school" | "university" | "college" | "kindergarten" | "education" => 3,
        "hospital" | "clinic" => 4,
        "church" | "cathedral" | "chapel" | "mosque" | "synagogue" | "temple" | "monastery"
        | "religious" | "wayside_shrine" | "presbytery" | "shrine" => 5,
        "hotel" | "hostel" | "motel" => 6,
        "garage" | "garages" | "carport" | "parking" => ids::SETTLEMENT_PARKING_STRUCTURE,
        "farm" | "barn" | "stable" | "sty" | "cowshed" => 8,
        // Large semi-open sports structures: occasional crowds, mostly empty
        // concrete — a moderate public-grade level, NOT a footprint-scaled
        // restaurant. Civic / cultural / tourist public buildings — moderate
        // public-grade level.
        "public" | "civic" | "government" | "government_office" | "cultural" | "fire_station"
        | "castle" | "museum" | "hall" | "funeral_hall" | "historic" | "stadium" | "grandstand"
        | "sports_hall" | "sports_centre" => 9,
        // SILENT tail: uninhabited / unheated / infra structures.
        // `farm_auxiliary` is ambiguous (many are sheds) → SILENT unless the
        // livestock check above overrides it to farm.
        "shed" | "roof" | "hut" | "outbuilding" | "greenhouse" | "static_caravan"
        | "carport_roof" | "ruins" | "ruin" | "construction" | "collapsed" | "service"
        | "allotment_house" | "boathouse" | "bunker" | "tent" | "container" | "storage_tank"
        | "silo" | "hangar" | "conservatory" | "ger" | "farm_auxiliary" | "transformer_tower"
        | "water_tower" | "no" | "bridge" | "tower" | "toilets" | "elevator" | "tech_cab"
        | "guardhouse" | "gatehouse" | "pavilion" | "abandoned" | "stairs" | "staircase"
        | "chimney" | "demolished" | "forestry" | "signal_box" | "security_booth" | "shelter"
        | "proposed" | "ship" => ids::SETTLEMENT_SILENT,
        "yes" | "" => 0, // default to residential-apartments
        _ => 0,
    }
}

/// The envelope-use code: residential 0, commercial 1, industrial 2, outdoor 3.
/// A parking function alone cannot distinguish an open carport from a garage.
pub(super) fn building_use(tags: &Tags) -> u8 {
    let tag = |key: &str| tags.get(key).map(String::as_str);
    if matches!(tag("building"), Some("carport" | "roof"))
        || (tag("amenity") == Some("parking") && tag("parking") == Some("carports"))
    {
        return 3;
    }
    match tag("building:use").unwrap_or("") {
        "residential" => 0,
        "commercial" | "retail" | "office" => 1,
        "industrial" => 2,
        _ => 0,
    }
}

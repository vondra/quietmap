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

pub(crate) fn classify_way_unscoped(way: &Way) -> Option<FeatureType> {
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
    if has_a_building(tag) {
        return Some(FeatureType::Building);
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
                .then_some(FeatureType::Leisure)
        }
        Some(ParkingKind::Structure | ParkingKind::Underground) => {
            return Some(FeatureType::Building)
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
    if let Some("works" | "wastewater_plant") = tag("man_made") {
        return Some(FeatureType::Industrial);
    }
    if let Some("plant" | "substation") = tag("power") {
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
    crate::spill::poi_class(
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

/// The OSM ring test: four node references or more, the last one repeating the
/// first. An open-air AREA source is only as real as its ring — a line has no
/// area to scale its emission by, whatever the shoelace formula would compute.
pub(crate) fn is_a_closed_ring(refs: &[i64]) -> bool {
    refs.len() >= 4 && refs.first() == refs.last()
}

/// What an `amenity=parking*` area is, acoustically. One decision, read by the
/// way router, the relation router and the spill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParkingKind {
    /// Open ground: cars manoeuvre, doors shut and trolleys roll in the open, so
    /// it emits ([`crate::ids::LEISURE_CAR_PARK`]), and nothing stands there, so
    /// it never screens.
    OpenLot,
    /// A street-side or lane strip: the same movements, packed into a strip that
    /// borrows the street as its aisle ([`crate::ids::LEISURE_CAR_PARK_STREET`]).
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

/// Road tags that survive extraction. `old_ref` is kept for the
/// spill-level road-ref fallback (renumbered motorways keep matching
/// old-numbering census sections); `int_ref` stays dropped (census sections
/// are cut against national numbering, not E-roads).
fn keep_road_tag(k: &str) -> bool {
    matches!(
        k,
        "highway"
            | "name"
            | "ref"
            | "old_ref"
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
    )
}

/// Extract relevant tags from a way.
pub fn extract_way_tags(way: &Way, ftype: &FeatureType) -> Tags {
    let mut t = Tags::new();
    for (k, v) in way.tags() {
        match ftype {
            FeatureType::Road => {
                if keep_road_tag(k) {
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
                        | "layer"
                        | "location"
                        | "covered"
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
                        // the zone tag that routed an area with no `building`
                        | "landuse"
                        // a basement garage emits, but nothing stands over it
                        | "location"
                        | "parking"
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
                        // car park: lot or street-side strip (spaces per m²)
                        | "parking"
                        | "outdoor_seating"
                        | "access"
                        | "name"
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

#[cfg(test)]
mod tests {
    use super::{
        has_a_building, is_a_closed_ring, is_functional_area, is_leisure_area, keep_road_tag,
        parking_kind, ParkingKind,
    };

    fn tag_of<'a>(tags: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<&'a str> + 'a {
        move |key: &str| tags.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
    }

    /// Praha, Na Špitálce (way 1342239310): a 2 m wide `parking=street_side`
    /// strip stood in the served world as a 7 m wall, and the popup called the
    /// pavement beside it indoors. Open ground is a SOURCE and never a building
    /// row; a deck or a basement keeps the building lane; a ground with a
    /// function still routes as one.
    #[test]
    fn a_car_park_routes_by_what_it_physically_is() {
        let kind = |tags: &[(&str, &str)]| parking_kind(tag_of(tags));
        let na_spitalce: &[(&str, &str)] = &[("amenity", "parking"), ("parking", "street_side")];
        assert_eq!(kind(na_spitalce), Some(ParkingKind::OpenStrip));
        assert_eq!(kind(&[("amenity", "parking")]), Some(ParkingKind::OpenLot));
        assert_eq!(
            kind(&[("amenity", "parking"), ("parking", "surface")]),
            Some(ParkingKind::OpenLot)
        );
        // A car park that IS a building stands, whatever value the tag carries.
        assert_eq!(
            kind(&[("amenity", "parking"), ("building", "yes")]),
            Some(ParkingKind::Structure)
        );
        assert_eq!(kind(&[("building", "garage")]), Some(ParkingKind::Structure));
        for structure in [
            [("amenity", "parking"), ("parking", "multi-storey")],
            [("amenity", "parking"), ("parking", "carports")],
        ] {
            assert_eq!(kind(&structure), Some(ParkingKind::Structure), "{structure:?}");
        }
        for below in [
            [("amenity", "parking"), ("parking", "underground")],
            [("amenity", "parking"), ("location", "underground")],
        ] {
            assert_eq!(kind(&below), Some(ParkingKind::Underground), "{below:?}");
        }
        // A single stall inside a lot, and cars on a roof: already counted.
        for inside in [
            [("amenity", "parking_space"), ("parking", "surface")],
            [("amenity", "parking"), ("parking", "rooftop")],
        ] {
            assert_eq!(kind(&inside), Some(ParkingKind::NotItsOwnSource), "{inside:?}");
        }
        // Both routers ask `parking_kind` before any land use, so a lot inside a
        // retail or industrial zone is a lot — the zone never claims it.
        assert_eq!(
            kind(&[("amenity", "parking"), ("landuse", "retail")]),
            Some(ParkingKind::OpenLot)
        );
        assert_eq!(
            kind(&[
                ("amenity", "parking"),
                ("parking", "multi-storey"),
                ("landuse", "industrial")
            ]),
            Some(ParkingKind::Structure)
        );
        // The car park routes on its own CLOSED ring, not through the leisure
        // tag gate: that gate is shared with nodes, which have no area at all.
        assert!(!is_leisure_area(na_spitalce));
        assert!(is_a_closed_ring(&[7, 8, 9, 7]));
        assert!(!is_a_closed_ring(&[7, 8, 9, 10]), "a lane drawn as a line has no area");
        assert!(!is_a_closed_ring(&[7, 7]));
        // `building=no` says there is no building: it must not route one by its
        // mere presence.
        assert!(!has_a_building(tag_of(&[("building", "no")])));
        assert!(has_a_building(tag_of(&[("building", "yes")])));
        // A ground with a function is still a building-lane source.
        assert!(is_functional_area(tag_of(&[("amenity", "school")])));
        assert!(is_functional_area(tag_of(&[("landuse", "retail")])));
        assert!(!is_functional_area(tag_of(&[("leisure", "pitch")])));
    }

    #[test]
    fn road_keep_list_carries_old_ref_for_the_fallback() {
        assert!(keep_road_tag("ref"));
        assert!(keep_road_tag("old_ref"));
        assert!(!keep_road_tag("int_ref"));
        assert!(!keep_road_tag("tiger:reviewed"));
    }
}

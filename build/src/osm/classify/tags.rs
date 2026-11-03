//! Retain the OSM fields needed by topology and the common source models.

use super::{FeatureType, Tags, keep_model_tag};
use osmpbf::Way;

/// Road tags that survive extraction. `old_ref` is kept for the
/// spill-level road-ref fallback (renumbered motorways keep matching
/// old-numbering census sections); `int_ref` stays dropped (census sections
/// are cut against national numbering, not E-roads).
pub(super) fn keep_road_tag(k: &str) -> bool {
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
    extract_tags(way.tags(), ftype)
}

pub fn extract_tags<'a>(
    tags: impl Iterator<Item = (&'a str, &'a str)>,
    ftype: &FeatureType,
) -> Tags {
    let mut t = Tags::new();
    for (k, v) in tags {
        if keep_model_tag(ftype, k) {
            t.insert(k.to_string(), v.to_string());
        }
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
                        | "railway"
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

//! Decode PBF blocks and resolve selected features in parallel.

use crate::classify::{self, FeatureType, Tags};
use crate::node_cache::NodeCache;
use crate::relations::RelationManifest;
use crate::transport::TrainRouteRecord;
use anyhow::Result;
use osmpbf::{Blob, BlobDecode, Element};

pub(super) struct PreparedBlob {
    pub(super) ways_seen: u64,
    pub(super) items: Vec<Prepared>,
}

pub(super) enum Prepared {
    Way(PreparedWay),
    Fallthrough(String),
    Point(PreparedPoint),
    Train(TrainRouteRecord),
}

pub(super) struct PreparedWay {
    pub(super) id: i64,
    pub(super) is_relation_member: bool,
    pub(super) class: Option<FeatureType>,
    pub(super) resolved_nodes: Vec<(i64, Option<[f64; 2]>)>,
    pub(super) tags: Tags,
}

pub(super) struct PreparedPoint {
    pub(super) id: i64,
    pub(super) lat: f64,
    pub(super) lon: f64,
    pub(super) turbine: Option<Tags>,
    pub(super) airport: Option<Tags>,
    pub(super) settlement: Option<(FeatureType, Tags)>,
}

/// Decode, classify, and look up node coordinates for one compressed PBF blob.
pub(super) fn prepare_blob(
    blob: &Blob,
    cache: &NodeCache,
    manifest: &RelationManifest,
) -> Result<PreparedBlob> {
    let BlobDecode::OsmData(block) = blob.decode()? else {
        return Ok(PreparedBlob {
            ways_seen: 0,
            items: Vec::new(),
        });
    };
    let mut out = PreparedBlob {
        ways_seen: 0,
        items: Vec::new(),
    };
    for element in block.elements() {
        match element {
            Element::Way(way) => {
                out.ways_seen += 1;
                let is_relation_member = manifest.way_to_relations.contains_key(&way.id());
                let mut way_class = classify::classify_way(&way);
                if way_class.is_none() && !is_relation_member {
                    if let Some(reason) = classify::fallthrough_reason(&way) {
                        out.items.push(Prepared::Fallthrough(reason));
                    }
                    continue;
                }
                let resolved_nodes: Vec<_> = way.refs().map(|id| (id, cache.get(id))).collect();
                // Closed-ring runway/airstrip ways are geometrically polygons;
                // classify as AirportArea before tag extraction so spill columns match.
                if matches!(way_class, Some(FeatureType::AirportLine)) {
                    let coords: Vec<_> = resolved_nodes
                        .iter()
                        .filter_map(|(_, coords)| *coords)
                        .collect();
                    if coords.len() >= 3
                        && (coords[0][0] - coords.last().unwrap()[0]).abs() < 1e-7
                        && (coords[0][1] - coords.last().unwrap()[1]).abs() < 1e-7
                        && way
                            .tags()
                            .any(|(k, v)| k == "aeroway" && matches!(v, "runway" | "airstrip"))
                    {
                        way_class = Some(FeatureType::AirportArea);
                    }
                }
                let tags = way_class
                    .as_ref()
                    .map(|ftype| classify::extract_way_tags(&way, ftype))
                    .unwrap_or_default();
                out.items.push(Prepared::Way(PreparedWay {
                    id: way.id(),
                    is_relation_member,
                    class: way_class,
                    resolved_nodes,
                    tags,
                }));
            }
            Element::Node(node) => {
                if let Some(point) = prepare_point(
                    node.id(),
                    node.lat(),
                    node.lon(),
                    classify::is_wind_turbine_node(&node)
                        .then(|| classify::extract_turbine_tags_node(&node)),
                    classify::is_airport_node(&node)
                        .then(|| classify::extract_airport_tags_node(&node)),
                    classify::node_kind_node(&node)
                        .map(|kind| (kind, classify::extract_node_settlement_tags_node(&node))),
                ) {
                    out.items.push(Prepared::Point(point));
                }
            }
            Element::DenseNode(node) => {
                if let Some(point) = prepare_point(
                    node.id(),
                    node.lat(),
                    node.lon(),
                    classify::is_wind_turbine_dense(&node)
                        .then(|| classify::extract_turbine_tags_dense(&node)),
                    classify::is_airport_dense(&node)
                        .then(|| classify::extract_airport_tags_dense(&node)),
                    classify::node_kind_dense(&node)
                        .map(|kind| (kind, classify::extract_node_settlement_tags_dense(&node))),
                ) {
                    out.items.push(Prepared::Point(point));
                }
            }
            Element::Relation(relation) => {
                if classify::scope_keeps(&FeatureType::Railway) {
                    if let Some(route) = TrainRouteRecord::from_relation(&relation)? {
                        out.items.push(Prepared::Train(route));
                    }
                }
            }
        }
    }
    Ok(out)
}

fn prepare_point(
    id: i64,
    lat: f64,
    lon: f64,
    turbine: Option<Tags>,
    airport: Option<Tags>,
    settlement: Option<(FeatureType, Tags)>,
) -> Option<PreparedPoint> {
    if turbine.is_none() && airport.is_none() && settlement.is_none() {
        return None;
    }
    Some(PreparedPoint {
        id,
        lat,
        lon,
        turbine,
        airport,
        settlement,
    })
}

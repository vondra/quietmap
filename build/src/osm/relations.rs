//! Multipolygon relation assembly.
//!
//! Pass 0: Collect multipolygon membership and repeated linear-way node identities.
//! Pass 1: As ways are processed, cache geometries for relation members.
//!         When all members of a relation are collected, assemble the polygon.

use anyhow::Result;
use osmpbf::{Element, ElementReader, RelMemberType};
use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use crate::osm::classify::{FeatureType, Tags};
use crate::osm::junctions::{JunctionCensus, NodeIdBitmap, NodeIdSet};

/// Info about a relation we care about.
#[derive(Clone)]
pub struct RelationInfo {
    pub feature_types: Vec<FeatureType>,
    pub tags: Tags,
    pub member_ways: Vec<(i64, String)>, // (way_id, role: "outer"/"inner")
}

/// Result of Pass 0: which ways are members of which multipolygon relations.
pub struct RelationManifest {
    /// way_id → list of (relation_id, role)
    pub way_to_relations: HashMap<i64, Vec<(i64, String)>>,
    /// relation_id → RelationInfo
    pub relations: HashMap<i64, RelationInfo>,
}

/// Scan multipolygon relations, linear-way junctions, and selected-layer node IDs.
///
/// Parallel via `osmpbf::par_map_reduce` — decodes PBF blocks across all
/// rayon worker threads, each produces a partial manifest, then merged
/// into a single one. Selected-way node IDs are recorded here so Pass 1 can
/// omit the rest. Relation-member ways that
/// did not themselves classify (typical untagged multipolygon outers) need a
/// second full PBF read; it is skipped when the member set is empty.
pub fn scan_relations_and_junctions(
    pbf_path: &Path,
) -> Result<(RelationManifest, NodeIdBitmap, NodeIdBitmap)> {
    let junctions = JunctionCensus::new()?;
    let needed = NodeIdSet::new()?;
    let reader = ElementReader::from_path(pbf_path)?;

    let manifest = reader.par_map_reduce(
        |element| {
            let mut local = RelationManifest {
                way_to_relations: HashMap::new(),
                relations: HashMap::new(),
            };
            let control_id = match &element {
                Element::Node(node) => {
                    crate::osm::classify::transport_point_tags(node.tags()).map(|_| node.id())
                }
                Element::DenseNode(node) => {
                    crate::osm::classify::transport_point_tags(node.tags()).map(|_| node.id())
                }
                _ => None,
            };
            if let Some(id) = control_id {
                junctions.preserve(id);
            }
            if let Element::Way(ref way) = element {
                // Census is independent of output scope: retained families must
                // have the same segmentation in scoped and full extractions.
                if crate::osm::classify::classify_primary_way(way).is_some_and(|ft| ft.is_linear())
                {
                    for node_id in way.refs() {
                        junctions.record(node_id);
                    }
                }
                // Scoped selected layers: only nodes Pass 2 will look up.
                if crate::osm::classify::classify_way(way).is_some() {
                    for node_id in way.refs() {
                        needed.insert(node_id);
                    }
                }
            }
            if let Element::Relation(rel) = element
                && let Some((ftype, tags)) = classify_multipolygon(&rel)
            {
                let mut member_ways = Vec::new();
                for member in rel.members() {
                    if member.member_type == RelMemberType::Way {
                        let role = member.role().unwrap_or("outer").to_string();
                        member_ways.push((member.member_id, role.clone()));
                        local
                            .way_to_relations
                            .entry(member.member_id)
                            .or_default()
                            .push((rel.id(), role));
                    }
                }
                if !member_ways.is_empty() {
                    local.relations.insert(
                        rel.id(),
                        RelationInfo {
                            feature_types: ftype,
                            tags,
                            member_ways,
                        },
                    );
                }
            }
            local
        },
        || RelationManifest {
            way_to_relations: HashMap::new(),
            relations: HashMap::new(),
        },
        |mut a, b| {
            // Relations are keyed by relation_id; conflicts impossible
            // (each rel appears in exactly one PBF block).
            a.relations.extend(b.relations);
            // way_to_relations: same way can be a member of multiple
            // relations across blocks, so we append instead of replace.
            for (way_id, mut rels) in b.way_to_relations {
                a.way_to_relations
                    .entry(way_id)
                    .or_default()
                    .append(&mut rels);
            }
            a
        },
    )?;

    eprintln!(
        "  Pass 0: {} multipolygon relations, {} member ways",
        manifest.relations.len(),
        manifest.way_to_relations.len()
    );

    eprintln!("  Pass 0: {} protected linear-way nodes", junctions.count());
    eprintln!(
        "  Pass 0: {} selected-way nodes marked for the coordinate cache",
        needed.count()
    );

    if !manifest.way_to_relations.is_empty() {
        let before = needed.count();
        let t_members = Instant::now();
        mark_relation_member_way_nodes(pbf_path, &manifest.way_to_relations, &needed)?;
        eprintln!(
            "  Pass 0: {} relation-member nodes added to the cache filter in {:.1}s (second full PBF read)",
            needed.count().saturating_sub(before),
            t_members.elapsed().as_secs_f64()
        );
    }

    Ok((manifest, junctions.finish(), needed.finish()))
}

/// Second PBF read used only when multipolygon members exist: untagged outer
/// ways never classify, so the selected-way census above would omit their
/// nodes and Pass 2 would drop the assembled ring. Cost is one extra decode
/// of the file (node blobs included); quantified in the Pass 0 log.
fn mark_relation_member_way_nodes(
    pbf_path: &Path,
    members: &HashMap<i64, Vec<(i64, String)>>,
    needed: &NodeIdSet,
) -> Result<()> {
    let reader = ElementReader::from_path(pbf_path)?;
    reader.par_map_reduce(
        |element| {
            if let Element::Way(way) = element
                && members.contains_key(&way.id())
            {
                for node_id in way.refs() {
                    needed.insert(node_id);
                }
            }
        },
        || (),
        |_, _| (),
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "relations_evidence_tests.rs"]
mod evidence_tests;

mod assemble;
mod polygon_tags;
#[cfg(test)]
use assemble::merge_outer_rings;
pub use assemble::{AssembledRelation, RelationAssembler};
use polygon_tags::classify_multipolygon;
pub use polygon_tags::spill_tags_for_assembled;

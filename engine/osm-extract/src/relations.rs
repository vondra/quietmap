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

use crate::classify::{FeatureType, Tags};
use crate::junctions::{JunctionCensus, NodeIdBitmap, NodeIdSet};

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
                    crate::classify::transport_point_tags(node.tags()).map(|_| node.id())
                }
                Element::DenseNode(node) => {
                    crate::classify::transport_point_tags(node.tags()).map(|_| node.id())
                }
                _ => None,
            };
            if let Some(id) = control_id {
                junctions.record(id);
                junctions.record(id);
            }
            if let Element::Way(ref way) = element {
                // Census is independent of output scope: retained families must
                // have the same segmentation in scoped and full extractions.
                if crate::classify::classify_way_unscoped(way).is_some_and(|ft| ft.is_linear()) {
                    for node_id in way.refs() {
                        junctions.record(node_id);
                    }
                }
                // Scoped selected layers: only nodes Pass 2 will look up.
                if crate::classify::classify_way(way).is_some() {
                    for node_id in way.refs() {
                        needed.insert(node_id);
                    }
                }
            }
            if let Element::Relation(rel) = element {
                // QM_OSM_ONLY scope: skip out-of-scope multipolygons here so
                // their members never enter the assembly manifest at all.
                if let Some((ftype, tags)) = classify_multipolygon(&rel) {
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
            if let Element::Way(way) = element {
                if members.contains_key(&way.id()) {
                    for node_id in way.refs() {
                        needed.insert(node_id);
                    }
                }
            }
        },
        || (),
        |_, _| (),
    )?;
    Ok(())
}

/// Decide whether a relation is one of the multipolygon flavours we track.
fn classify_multipolygon(rel: &osmpbf::Relation) -> Option<(Vec<FeatureType>, Tags)> {
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
    let parking = crate::classify::parking_kind(tag);
    let ftype = if crate::classify::has_a_building(tag)
        || matches!(
            parking,
            Some(
                crate::classify::ParkingKind::Structure | crate::classify::ParkingKind::Underground
            )
        ) {
        FeatureType::Building
    } else if crate::classify::is_special_leisure(tag) {
        FeatureType::Leisure
    } else if parking.is_some() {
        // Open ground, a stall or a rooftop: not a relation source.
        return None;
    } else if matches!(
        tag("landuse"),
        Some("industrial" | "quarry" | "farmyard" | "landfill" | "port" | "harbour")
    ) || matches!(tag("man_made"), Some("works") | Some("wastewater_plant"))
        || crate::classify::is_power_or_inactive_industry(tag)
    {
        FeatureType::Industrial
    } else if matches!(
        tag("aeroway"),
        Some("runway" | "taxiway" | "apron" | "helipad" | "aerodrome" | "stopway" | "airstrip")
    ) || tag("amenity") == Some("heliport")
    {
        FeatureType::AirportArea
    } else if crate::classify::is_functional_area(tag) {
        // Functional AREA relation with no building tag: a mall (shop=mall),
        // hospital or school campus, retail/commercial zone (audit 2026-06).
        FeatureType::Building
    } else {
        return None;
    };
    let types = crate::classify::scoped_feature_types(ftype, tag);
    (!types.is_empty()).then_some((types, tags))
}

/// Tags an assembled multipolygon carries into spill. Copied from relation
/// tags (not member ways) so a hospital MP without `building=*` still classifies.
pub fn spill_tags_for_assembled(ftype: &FeatureType, tags: &Tags) -> Tags {
    crate::classify::extract_tags(tags.iter().map(|(k, v)| (k.as_str(), v.as_str())), ftype)
}

/// Accumulates way geometries for relation assembly.
pub struct AssembledRelation {
    pub rings: Vec<Vec<[f64; 2]>>,
    pub tags: Tags,
    pub feature_types: Vec<FeatureType>,
}

pub struct RelationAssembler {
    /// way_id → resolved coordinates
    way_geoms: HashMap<i64, Vec<[f64; 2]>>,
    /// relation_id → how many member ways still missing
    pending_count: HashMap<i64, usize>,
}

impl RelationAssembler {
    pub fn new(manifest: &RelationManifest) -> Self {
        let mut pending_count = HashMap::new();
        for (rel_id, info) in &manifest.relations {
            pending_count.insert(*rel_id, info.member_ways.len());
        }
        RelationAssembler {
            way_geoms: HashMap::new(),
            pending_count,
        }
    }

    /// Record a way's geometry. Returns list of relation_ids that are now complete.
    pub fn add_way(
        &mut self,
        way_id: i64,
        coords: Vec<[f64; 2]>,
        manifest: &RelationManifest,
    ) -> Vec<i64> {
        self.way_geoms.insert(way_id, coords);

        let mut completed = Vec::new();
        if let Some(rels) = manifest.way_to_relations.get(&way_id) {
            for (rel_id, _role) in rels {
                if let Some(count) = self.pending_count.get_mut(rel_id) {
                    *count -= 1;
                    if *count == 0 {
                        completed.push(*rel_id);
                    }
                }
            }
        }
        completed
    }

    /// Assemble a completed relation into a polygon.
    /// Returns (outer parts, tags, feature types) or None if assembly fails.
    pub fn assemble(&self, rel_id: i64, manifest: &RelationManifest) -> Option<AssembledRelation> {
        let info = manifest.relations.get(&rel_id)?;

        // Collect outer rings' coordinates
        let mut outer_coords: Vec<Vec<[f64; 2]>> = Vec::new();
        for (way_id, role) in &info.member_ways {
            if role == "outer" || role.is_empty() {
                if let Some(coords) = self.way_geoms.get(way_id) {
                    outer_coords.push(coords.clone());
                }
            }
        }

        if outer_coords.is_empty() {
            return None;
        }

        // Try to merge outer ways into a single ring
        let merged = merge_outer_rings(outer_coords);
        if merged.is_empty() {
            return None;
        }

        Some(AssembledRelation {
            rings: merged,
            tags: info.tags.clone(),
            feature_types: info.feature_types.clone(),
        })
    }

    /// Remove cached way geometries for a completed relation to free memory.
    pub fn cleanup(&mut self, rel_id: i64, manifest: &RelationManifest) {
        if let Some(info) = manifest.relations.get(&rel_id) {
            for (way_id, _) in &info.member_ways {
                // Only remove if this way isn't needed by other pending relations
                let still_needed = manifest
                    .way_to_relations
                    .get(way_id)
                    .map(|rels| {
                        rels.iter().any(|(rid, _)| {
                            *rid != rel_id && self.pending_count.get(rid).copied().unwrap_or(0) > 0
                        })
                    })
                    .unwrap_or(false);
                if !still_needed {
                    self.way_geoms.remove(way_id);
                }
            }
            self.pending_count.remove(&rel_id);
        }
    }
}

/// Keep each connected outer part separately. Extend either end because the
/// first member need not be the first edge, and OSM member directions vary.
fn merge_outer_rings(ways: Vec<Vec<[f64; 2]>>) -> Vec<Vec<[f64; 2]>> {
    let mut remaining: Vec<_> = ways.into_iter().filter(|way| !way.is_empty()).collect();
    let mut rings = Vec::new();
    while !remaining.is_empty() {
        let mut ring = remaining.remove(0);
        while !(ring.len() >= 3 && points_close(ring[0], *ring.last().unwrap())) {
            let head = ring[0];
            let tail = *ring.last().unwrap();
            let Some((index, at_head, reverse)) =
                remaining.iter().enumerate().find_map(|(index, way)| {
                    let first = way[0];
                    let last = *way.last().unwrap();
                    if points_close(tail, first) {
                        Some((index, false, false))
                    } else if points_close(tail, last) {
                        Some((index, false, true))
                    } else if points_close(head, last) {
                        Some((index, true, false))
                    } else if points_close(head, first) {
                        Some((index, true, true))
                    } else {
                        None
                    }
                })
            else {
                break;
            };
            let mut way = remaining.remove(index);
            if reverse {
                way.reverse();
            }
            if at_head {
                way.pop();
                way.extend(ring);
                ring = way;
            } else {
                ring.extend_from_slice(&way[1..]);
            }
        }
        rings.push(ring);
    }
    rings
}

fn points_close(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-7 && (a[1] - b[1]).abs() < 1e-7
}

#[cfg(test)]
mod evidence_tests {
    use super::*;
    #[test]
    fn outer_parts_and_reversed_members_survive() {
        let a = [0., 0.];
        let b = [0., 1.];
        let c = [1., 1.];
        let d = [2., 2.];
        let e = [2., 3.];
        let f = [3., 3.];
        let rings = merge_outer_rings(vec![vec![b, c], vec![a, b], vec![a, c], vec![d, e, f, d]]);
        assert_eq!(rings.len(), 2);
        assert_eq!(rings[0].first(), rings[0].last());
        assert_eq!(rings[0].len(), 4);
        assert_eq!(rings[1], vec![d, e, f, d]);
    }
}

//! Multipolygon relation assembly.
//!
//! Pass 0: Scan relations → build manifest of which ways belong to which relations.
//! Pass 1: As ways are processed, cache geometries for relation members.
//!         When all members of a relation are collected, assemble the polygon.

use anyhow::Result;
use osmpbf::{Element, ElementReader, RelMemberType};
use std::collections::HashMap;
use std::path::Path;

use crate::classify::{FeatureType, Tags};

/// Info about a relation we care about.
#[derive(Clone)]
pub struct RelationInfo {
    pub feature_type: FeatureType,
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

/// Pass 0: Scan PBF for multipolygon relations containing buildings/industrial/airports.
///
/// Parallel via `osmpbf::par_map_reduce` — decodes PBF blocks across all
/// rayon worker threads, each produces a partial manifest, then merged
/// into a single one. ~4-6× faster than single-threaded on a 24-core box.
pub fn scan_relations(pbf_path: &Path) -> Result<RelationManifest> {
    let reader = ElementReader::from_path(pbf_path)?;

    let manifest = reader.par_map_reduce(
        |element| {
            let mut local = RelationManifest {
                way_to_relations: HashMap::new(),
                relations: HashMap::new(),
            };
            if let Element::Relation(rel) = element {
                // QM_OSM_ONLY scope: skip out-of-scope multipolygons here so
                // their members never enter the assembly manifest at all.
                if let Some((ftype, tags)) =
                    classify_multipolygon(&rel).filter(|(ft, _)| crate::classify::scope_keeps(ft))
                {
                    let mut rel_tags = Tags::new();
                    for (k, v) in &tags {
                        rel_tags.insert(k.clone(), v.clone());
                    }
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
                                feature_type: ftype,
                                tags: rel_tags,
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

    Ok(manifest)
}

/// Decide whether a relation is one of the multipolygon flavours we track.
fn classify_multipolygon(rel: &osmpbf::Relation) -> Option<(FeatureType, Vec<(String, String)>)> {
    let tags: Vec<(String, String)> = rel
        .tags()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let tag = |k: &str| {
        tags.iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.as_str())
    };
    if tag("type") != Some("multipolygon") {
        return None;
    }
    let ftype = if tag("building").is_some() {
        FeatureType::Building
    } else if matches!(
        tag("landuse"),
        Some("industrial" | "quarry" | "farmyard" | "landfill" | "port" | "harbour")
    ) || matches!(tag("man_made"), Some("works") | Some("wastewater_plant"))
        || matches!(tag("power"), Some("plant") | Some("substation"))
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
    Some((ftype, tags))
}

/// Accumulates way geometries for relation assembly.
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
    /// Returns (outer_ring_coords, tags, feature_type) or None if assembly fails.
    pub fn assemble(
        &self,
        rel_id: i64,
        manifest: &RelationManifest,
    ) -> Option<(Vec<[f64; 2]>, Tags, FeatureType)> {
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
        let merged = merge_rings(&outer_coords);
        if merged.is_empty() {
            return None;
        }

        Some((merged, info.tags.clone(), info.feature_type.clone()))
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

/// Merge multiple way coordinate arrays into a single ring.
/// OSM outer ways share endpoints — try to connect them head-to-tail.
fn merge_rings(ways: &[Vec<[f64; 2]>]) -> Vec<[f64; 2]> {
    if ways.len() == 1 {
        return ways[0].clone();
    }

    // Build a chain: each way starts/ends at certain nodes.
    // Try to connect them by matching last point of one to first point of next.
    let mut remaining: Vec<Vec<[f64; 2]>> = ways.to_vec();
    let mut result = remaining.remove(0);

    let max_iters = remaining.len() * remaining.len() + 1;
    let mut iters = 0;

    while !remaining.is_empty() && iters < max_iters {
        iters += 1;
        let tail = *result.last().unwrap();
        // Find a way that connects to our tail
        let mut found = None;
        for (i, way) in remaining.iter().enumerate() {
            if way.is_empty() {
                continue;
            }
            let way_head = way[0];
            let way_tail = *way.last().unwrap();

            if points_close(tail, way_head) {
                // Append way as-is (skip first point — it's the same as our tail)
                found = Some((i, false));
                break;
            } else if points_close(tail, way_tail) {
                // Append way reversed
                found = Some((i, true));
                break;
            }
        }

        match found {
            Some((idx, reverse)) => {
                let mut way = remaining.remove(idx);
                if reverse {
                    way.reverse();
                }
                // Skip first point (duplicate of our tail)
                if !way.is_empty() {
                    result.extend_from_slice(&way[1..]);
                }
            }
            None => {
                // Can't connect — give up on remaining pieces
                break;
            }
        }

        // Check if ring is closed
        if result.len() >= 3 && points_close(result[0], *result.last().unwrap()) {
            break;
        }
    }

    result
}

fn points_close(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-7 && (a[1] - b[1]).abs() < 1e-7
}

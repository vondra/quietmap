//! Assemble outer rings when all referenced member ways have been decoded.

use super::*;

/// Accumulates way geometries for relation assembly.
pub struct AssembledRelation {
    pub rings: Vec<Vec<[f64; 2]>>,
    pub tags: Tags,
    pub feature_types: Vec<FeatureType>,
}

pub struct RelationAssembler {
    /// way_id → resolved coordinates
    pub(super) way_geoms: HashMap<i64, Vec<[f64; 2]>>,
    /// relation_id → how many member ways still missing
    pub(super) pending_count: HashMap<i64, usize>,
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
            if (role == "outer" || role.is_empty())
                && let Some(coords) = self.way_geoms.get(way_id)
            {
                outer_coords.push(coords.clone());
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
                // Completed relations still need their ways until assembly and cleanup.
                let still_needed = manifest
                    .way_to_relations
                    .get(way_id)
                    .map(|rels| {
                        rels.iter()
                            .any(|(rid, _)| *rid != rel_id && self.pending_count.contains_key(rid))
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
pub(super) fn merge_outer_rings(ways: Vec<Vec<[f64; 2]>>) -> Vec<Vec<[f64; 2]>> {
    let mut remaining: Vec<_> = ways.into_iter().filter(|way| !way.is_empty()).collect();
    let mut rings = Vec::new();
    while !remaining.is_empty() {
        let mut ring = remaining.remove(0);
        while !crate::osm::classify::is_a_closed_ring(&ring) {
            let head = ring[0];
            let tail = *ring.last().unwrap();
            let Some((index, at_head, reverse)) =
                remaining.iter().enumerate().find_map(|(index, way)| {
                    if crate::osm::classify::is_a_closed_ring(way) {
                        return None;
                    }
                    let first = way[0];
                    let last = *way.last().unwrap();
                    if tail == first {
                        Some((index, false, false))
                    } else if tail == last {
                        Some((index, false, true))
                    } else if head == last {
                        Some((index, true, false))
                    } else if head == first {
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

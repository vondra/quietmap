//! Assemble outer rings when all referenced member ways have been decoded.

use super::*;

/// Accumulates way geometries for relation assembly.
pub struct AssembledRelation {
    pub rings: Vec<Vec<[f64; 2]>>,
    /// Closed inner hole rings assigned to each outer ring (parallel to `rings`).
    pub holes_per_ring: Vec<Vec<Vec<[f64; 2]>>>,
    /// All closed inner hole rings (retained for horticulture zones).
    pub inner_rings: Vec<Vec<[f64; 2]>>,
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

        let horticulture =
            info.tags.get("landuse").map(String::as_str) == Some("greenhouse_horticulture");
        let keep_holes = info.feature_types.contains(&FeatureType::Industrial)
            && crate::osm::classify::is_active_military_range_map(&info.tags);

        // Collect outer rings' coordinates, plus inner holes for active military ranges and horticulture zones.
        let mut outer_coords: Vec<Vec<[f64; 2]>> = Vec::new();
        let mut inner_coords: Vec<Vec<[f64; 2]>> = Vec::new();
        for (way_id, role) in &info.member_ways {
            if let Some(coords) = self.way_geoms.get(way_id) {
                if role == "outer" || role.is_empty() {
                    outer_coords.push(coords.clone());
                } else if (keep_holes || horticulture) && role == "inner" {
                    inner_coords.push(coords.clone());
                }
            }
        }

        if outer_coords.is_empty() {
            return None;
        }

        // Merge outer ways into rings; when keeping holes, assign each closed inner hole to the
        // smallest (immediate) enclosing outer ring so nested islands keep their own holes.
        let mut merged = merge_outer_rings(outer_coords);
        if horticulture {
            merged.retain(|ring| crate::osm::classify::is_a_closed_ring(ring));
        }
        if merged.is_empty() {
            return None;
        }
        let merged_inners: Vec<Vec<[f64; 2]>> =
            if (keep_holes || horticulture) && !inner_coords.is_empty() {
                merge_outer_rings(inner_coords)
                    .into_iter()
                    .filter(|ring| crate::osm::classify::is_a_closed_ring(ring))
                    .collect()
            } else {
                Vec::new()
            };
        let mut holes_per_ring: Vec<Vec<Vec<[f64; 2]>>> = vec![Vec::new(); merged.len()];
        if keep_holes && !merged_inners.is_empty() {
            let outer_areas: Vec<f64> = merged.iter().map(|r| ring_area_deg2(r)).collect();
            for inner in &merged_inners {
                if let Some(host) = immediate_enclosing_outer(&merged, &outer_areas, inner) {
                    holes_per_ring[host].push(inner.clone());
                }
            }
        }
        let inner_rings = if horticulture {
            merged_inners
        } else {
            Vec::new()
        };

        Some(AssembledRelation {
            rings: merged,
            holes_per_ring,
            inner_rings,
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

/// Ray-cast containment in `[lat, lon]` order with longitude wrapped around the ring's first point.
fn ring_contains_latlon(ring: &[[f64; 2]], point: [f64; 2]) -> bool {
    if ring.len() < 3 {
        return false;
    }
    let reference = ring[0][1];
    let px = reference + crate::osm::geometry::wrapped_longitude_delta(reference, point[1]);
    let py = point[0];
    let mut inside = false;
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let xi = reference + crate::osm::geometry::wrapped_longitude_delta(reference, ring[i][1]);
        let yi = ring[i][0];
        let xj = reference + crate::osm::geometry::wrapped_longitude_delta(reference, ring[j][1]);
        let yj = ring[j][0];
        if ((yi > py) != (yj > py)) && (px < (xj - xi) * (py - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Unsigned planar shoelace area in wrapped degree-squared (used only to rank enclosing outers).
fn ring_area_deg2(ring: &[[f64; 2]]) -> f64 {
    if ring.len() < 3 {
        return 0.0;
    }
    let reference = ring[0][1];
    let mut sum = 0.0;
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let xi = crate::osm::geometry::wrapped_longitude_delta(reference, ring[i][1]);
        let yi = ring[i][0];
        let xj = crate::osm::geometry::wrapped_longitude_delta(reference, ring[j][1]);
        let yj = ring[j][0];
        sum += xj * yi - xi * yj;
        j = i;
    }
    0.5 * sum.abs()
}

/// Immediate (smallest-area) outer ring that encloses `inner`. Probes vertices of `inner` first
/// (so a hole whose first vertex coincides with the outer boundary still matches), falling back
/// to its vertex mean if every vertex lies on the outer boundary.
fn immediate_enclosing_outer(
    outers: &[Vec<[f64; 2]>],
    outer_areas: &[f64],
    inner: &[[f64; 2]],
) -> Option<usize> {
    let inner_area = ring_area_deg2(inner);
    let unique = if inner.len() > 1 && inner.last() == inner.first() {
        &inner[..inner.len() - 1]
    } else {
        inner
    };
    let mut best: Option<(usize, f64)> = None;
    for (idx, outer) in outers.iter().enumerate() {
        let area = outer_areas[idx];
        if area <= inner_area {
            continue;
        }
        let enclosed = unique.iter().any(|&pt| ring_contains_latlon(outer, pt)) || {
            let reference = unique[0][1];
            let n = unique.len() as f64;
            let mean_lon = reference
                + unique
                    .iter()
                    .map(|p| crate::osm::geometry::wrapped_longitude_delta(reference, p[1]))
                    .sum::<f64>()
                    / n;
            let mean_lat = unique.iter().map(|p| p[0]).sum::<f64>() / n;
            ring_contains_latlon(outer, [mean_lat, mean_lon])
        };
        if enclosed && best.is_none_or(|(_, best_area)| area < best_area) {
            best = Some((idx, area));
        }
    }
    best.map(|(idx, _)| idx)
}

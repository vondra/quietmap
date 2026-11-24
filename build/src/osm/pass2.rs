//! Pass 2: parallel blob decode/classify/node-lookup; sequential spill, relations, transport.

use crate::osm::classify::{self, FeatureType, Tags};
mod prepare;
use crate::osm::junctions::NodeIdBitmap;
use crate::osm::microsegment;
use crate::osm::node_cache::NodeCache;
use crate::osm::relations::{self, RelationAssembler, RelationManifest};
use crate::osm::spill::{self, Spiller};
use crate::osm::transport::{self, TransportSpill};
use anyhow::Result;
use osmpbf::BlobReader;
use prepare::{Prepared, PreparedBlob, PreparedPoint, PreparedWay, prepare_blob};
use rayon::prelude::*;
use std::collections::BTreeSet;
use std::path::Path;

pub struct Pass2Stats {
    pub ways_total: u64,
    pub features_total: u64,
    pub rels_assembled: u64,
    pub antimeridian_rings_omitted: u64,
}

/// Parallel decode/classify/lookup in bounded blob batches; apply in file order
/// so spill rows, piece intervals, assembled relations and train routes match
/// the previous serial Pass 2. One Spiller and one TransportSpill on this thread.
pub fn extract_features(
    input: &Path,
    cache: &NodeCache,
    manifest: &RelationManifest,
    junctions: &NodeIdBitmap,
    spiller: &mut Spiller,
    transport: &mut TransportSpill,
) -> Result<Pass2Stats> {
    let mut assembler = RelationAssembler::new(manifest);
    let mut stats = Pass2Stats {
        ways_total: 0,
        features_total: 0,
        rels_assembled: 0,
        antimeridian_rings_omitted: 0,
    };

    let batch_len = rayon::current_num_threads().max(1);
    let mut blobs = BlobReader::from_path(input)?;
    loop {
        let mut batch = Vec::with_capacity(batch_len);
        while batch.len() < batch_len {
            match blobs.next() {
                Some(blob) => batch.push(blob?),
                None => break,
            }
        }
        if batch.is_empty() {
            break;
        }
        let prepared: Vec<PreparedBlob> = batch
            .par_iter()
            .map(|blob| prepare_blob(blob, cache, manifest))
            .collect::<Result<Vec<_>>>()?;
        for blob in prepared {
            apply_prepared(
                blob,
                manifest,
                junctions,
                &mut assembler,
                spiller,
                transport,
                &mut stats,
            )?;
        }
    }
    std::mem::take(&mut transport.controls).finish(spiller)?;
    Ok(stats)
}

fn apply_prepared(
    blob: PreparedBlob,
    manifest: &RelationManifest,
    junctions: &NodeIdBitmap,
    assembler: &mut RelationAssembler,
    spiller: &mut Spiller,
    transport: &mut TransportSpill,
    stats: &mut Pass2Stats,
) -> Result<()> {
    stats.ways_total += blob.ways_seen;
    if stats.ways_total > 0
        && stats.ways_total / 2_000_000 > (stats.ways_total - blob.ways_seen) / 2_000_000
    {
        eprintln!(
            "  {:.1}M ways, {:.1}M features, {} rels assembled...",
            stats.ways_total as f64 / 1e6,
            stats.features_total as f64 / 1e6,
            stats.rels_assembled
        );
    }
    for item in blob.items {
        match item {
            Prepared::Evidence(mut node) => {
                if let Some(tags) = node.power.take() {
                    stats.features_total += emit_node(
                        spiller,
                        &tags,
                        FeatureType::Industrial,
                        node.id,
                        node.lat,
                        node.lon,
                    )?;
                }
                transport.controls.insert(node);
            }
            Prepared::Train(route) => transport.write_train_route(&route)?,
            Prepared::Point(point) => apply_point(point, spiller, stats)?,
            Prepared::Way(way) => {
                apply_way(
                    way, manifest, junctions, assembler, spiller, transport, stats,
                )?;
            }
        }
    }
    Ok(())
}

fn apply_point(point: PreparedPoint, spiller: &mut Spiller, stats: &mut Pass2Stats) -> Result<()> {
    if let Some(tags) = point.turbine {
        stats.features_total += emit_node(
            spiller,
            &tags,
            FeatureType::WindTurbine,
            point.id,
            point.lat,
            point.lon,
        )?;
    }
    if let Some(tags) = point.airport {
        stats.features_total += emit_node(
            spiller,
            &tags,
            FeatureType::AirportArea,
            point.id,
            point.lat,
            point.lon,
        )?;
    }
    if let Some((kind, tags)) = point.settlement {
        stats.features_total +=
            emit_settlement_node(spiller, kind, point.id, point.lat, point.lon, &tags)?;
    }
    Ok(())
}

fn apply_way(
    way: PreparedWay,
    manifest: &RelationManifest,
    junctions: &NodeIdBitmap,
    assembler: &mut RelationAssembler,
    spiller: &mut Spiller,
    transport: &mut TransportSpill,
    stats: &mut Pass2Stats,
) -> Result<()> {
    let coords: Vec<_> = way
        .resolved_nodes
        .iter()
        .filter_map(|(_, coords)| *coords)
        .collect();

    if way.is_relation_member && !coords.is_empty() {
        let completed = assembler.add_way(way.id, coords.clone(), manifest);
        for rel_id in completed {
            if let Some(assembled) = assembler.assemble(rel_id, manifest) {
                let relations::AssembledRelation {
                    rings,
                    holes_per_ring,
                    inner_rings,
                    tags,
                    feature_types: types,
                } = assembled;
                for ftype in types {
                    let extracted_tags = relations::spill_tags_for_assembled(&ftype, &tags);
                    let horticulture = matches!(ftype, FeatureType::Building)
                        && is_horticulture_zone(&extracted_tags);
                    // New source contracts and horticulture zones retain every outer part.
                    // Other families keep their existing first-part behavior.
                    let count = if matches!(ftype, FeatureType::Industrial | FeatureType::Leisure)
                        || horticulture
                    {
                        rings.len()
                    } else {
                        1
                    };
                    // A facility nameplate describes the whole relation: every
                    // part keeps it byte-identical and carries its area share,
                    // so readers evaluate the one facility power and share it
                    // by area instead of radiating the nameplate per part.
                    let part_tags = if matches!(ftype, FeatureType::Industrial) {
                        facility_part_tags(&rings, &extracted_tags)
                    } else {
                        vec![extracted_tags.clone(); rings.len()]
                    };
                    let keep_holes = matches!(ftype, FeatureType::Industrial)
                        && classify::is_active_military_range_map(&extracted_tags);
                    for ((ring, holes), tags) in rings
                        .iter()
                        .zip(holes_per_ring.iter())
                        .zip(part_tags.iter())
                        .take(count)
                    {
                        let (clat, clon) = centroid(ring);
                        let square = crate::osm::geometry::square_of(clat, clon);
                        let safe_ring = ring_for_spill(ring, &mut stats.antimeridian_rings_omitted);
                        let safe_holes: Vec<&[[f64; 2]]> = if keep_holes && safe_ring.is_some() {
                            holes
                                .iter()
                                .filter_map(|hole| {
                                    ring_for_spill(hole, &mut stats.antimeridian_rings_omitted)
                                })
                                .collect()
                        } else {
                            Vec::new()
                        };
                        let squares = if horticulture {
                            squares_of_ring(safe_ring, square)
                        } else {
                            vec![square]
                        };
                        for sq in squares {
                            spiller.emit_polygon_with_holes(
                                &ftype,
                                sq,
                                rel_id,
                                "relation",
                                clat,
                                clon,
                                tags,
                                safe_ring,
                                &safe_holes,
                            )?;
                        }
                        stats.features_total += 1;
                    }
                    if horticulture {
                        let mut hole_tags = extracted_tags.clone();
                        hole_tags.insert("qm:horticulture_hole".to_string(), "1".to_string());
                        for hole in &inner_rings {
                            let (clat, clon) = centroid(hole);
                            let square = crate::osm::geometry::square_of(clat, clon);
                            let safe_ring =
                                ring_for_spill(hole, &mut stats.antimeridian_rings_omitted);
                            for sq in squares_of_ring(safe_ring, square) {
                                spiller.emit_polygon(
                                    &ftype, sq, rel_id, "relation", clat, clon, &hole_tags,
                                    safe_ring,
                                )?;
                            }
                        }
                    }
                }
                stats.rels_assembled += 1;
            }
            assembler.cleanup(rel_id, manifest);
        }
    }

    let Some(ftype) = way.class else {
        return Ok(());
    };
    let is_transport = matches!(ftype, FeatureType::Road | FeatureType::Railway);
    if is_transport {
        transport.observe_way(ftype.name(), &way.resolved_nodes);
        transport
            .controls
            .link_way(way.id, ftype.name(), &way.resolved_nodes, spiller)?;
    }
    let mut piece_squares = BTreeSet::new();
    if ftype.is_linear() {
        if coords.len() >= 2 {
            emit_linear_way(
                way.id,
                &ftype,
                &way.resolved_nodes,
                &way.tags,
                junctions,
                spiller,
                &mut piece_squares,
                stats,
            )?;
        }
    } else if !coords.is_empty() {
        let (clat, clon) = centroid(&coords);
        let square = crate::osm::geometry::square_of(clat, clon);
        let ring = if ftype == FeatureType::Leisure && coords.len() == 2 {
            Some(coords.as_slice())
        } else {
            ring_for_spill(&coords, &mut stats.antimeridian_rings_omitted)
        };
        if !relation_covers_kind(way.id, &ftype, &way.tags, manifest) {
            let squares = if ftype == FeatureType::Building && is_horticulture_zone(&way.tags) {
                squares_of_ring(ring, square)
            } else {
                vec![square]
            };
            for sq in squares {
                spiller.emit_polygon(&ftype, sq, way.id, "way", clat, clon, &way.tags, ring)?;
            }
        }
        for (kind, tags) in &way.additional {
            if !relation_covers_kind(way.id, kind, tags, manifest) {
                spiller.emit_polygon(kind, square, way.id, "way", clat, clon, tags, ring)?;
            }
        }
        stats.features_total += 1;
    }
    if matches!(ftype, FeatureType::Railway) {
        // Every classified railway way is kept, even without pieces: a train route naming it
        // must resolve to `missing_source_ways`, not to an unknown member.
        transport.write_railway_way(way.id, &way.resolved_nodes, &piece_squares)?;
    }
    Ok(())
}

/// Suppress only the family represented by an assembled outer relation. A
/// building member can carry a separate power/sport feature absent on its parent;
/// a real building way is not suppressed by a landuse=greenhouse_horticulture relation;
/// an active military range relation never suppresses an emitting industrial outer
/// way (nor vice versa), and always suppresses civilian shooting on its outer way.
fn relation_covers_kind(
    id: i64,
    kind: &FeatureType,
    way_tags: &classify::Tags,
    manifest: &RelationManifest,
) -> bool {
    let way_has_building = *kind == FeatureType::Building
        && classify::has_a_building(|k| way_tags.get(k).map(String::as_str));
    let way_military =
        *kind == FeatureType::Industrial && classify::is_active_military_range_map(way_tags);
    let way_shooting = *kind == FeatureType::Leisure
        && classify::special_leisure_class(|key| way_tags.get(key).map(String::as_str)) == Some(11);
    manifest.way_to_relations.get(&id).is_some_and(|parents| {
        parents.iter().any(|(id, role)| {
            (role.is_empty() || role == "outer")
                && manifest.relations.get(id).is_some_and(|relation| {
                    let rel_military = relation.feature_types.contains(&FeatureType::Industrial)
                        && classify::is_active_military_range_map(&relation.tags);
                    if way_shooting && rel_military {
                        return true;
                    }
                    relation.feature_types.contains(kind)
                        && (*kind != FeatureType::Industrial || rel_military == way_military)
                        && !(way_has_building && is_horticulture_zone(&relation.tags))
                })
        })
    })
}

#[cfg(test)]
#[path = "pass2_tests.rs"]
mod tests;

mod emit;
use emit::*;

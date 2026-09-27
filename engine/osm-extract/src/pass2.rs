//! Pass 2: parallel blob decode/classify/node-lookup; sequential spill, relations, transport.

use crate::classify::{self, FeatureType, Tags};
mod prepare;
use crate::junctions::NodeIdBitmap;
use crate::microsegment;
use crate::node_cache::NodeCache;
use crate::relations::{self, RelationAssembler, RelationManifest};
use crate::spill::{self, Spiller};
use crate::transport::{self, TransportSpill};
use anyhow::Result;
use osmpbf::BlobReader;
use prepare::{prepare_blob, Prepared, PreparedBlob, PreparedPoint, PreparedWay};
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
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
                    tags,
                    feature_types: types,
                } = assembled;
                for ftype in types {
                    let extracted_tags = relations::spill_tags_for_assembled(&ftype, &tags);
                    // New source contracts retain every outer part. Other
                    // families keep their existing first-part behavior.
                    let count = if matches!(ftype, FeatureType::Industrial | FeatureType::Leisure) {
                        rings.len()
                    } else {
                        1
                    };
                    // A facility nameplate describes the whole relation: share it
                    // by area over the emitting parts, so two 24 MW halves become
                    // two 12 MW sources instead of two 24 MW sources (+3.01 dB).
                    let part_tags = if matches!(ftype, FeatureType::Industrial) {
                        shared_part_tags(&rings, &extracted_tags)
                    } else {
                        vec![extracted_tags.clone(); rings.len()]
                    };
                    for (ring, tags) in rings.iter().zip(part_tags.iter()).take(count) {
                        let (clat, clon) = centroid(ring);
                        let square = grid::square_of(clat, clon);
                        let safe_ring = ring_for_spill(ring, &mut stats.antimeridian_rings_omitted);
                        spiller.emit_polygon(
                            &ftype,
                            square,
                            rel_id,
                            "relation",
                            clat,
                            clon,
                            tags,
                            safe_ring,
                        )?;
                        stats.features_total += 1;
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
        let square = grid::square_of(clat, clon);
        let ring = if ftype == FeatureType::Leisure && coords.len() == 2 {
            Some(coords.as_slice())
        } else {
            ring_for_spill(&coords, &mut stats.antimeridian_rings_omitted)
        };
        if !relation_covers_kind(way.id, &ftype, manifest) {
            spiller.emit_polygon(&ftype, square, way.id, "way", clat, clon, &way.tags, ring)?;
        }
        for (kind, tags) in &way.additional {
            if !relation_covers_kind(way.id, kind, manifest) {
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
/// building member can carry a separate power/sport feature absent on its parent.
fn relation_covers_kind(id: i64, kind: &FeatureType, manifest: &RelationManifest) -> bool {
    manifest.way_to_relations.get(&id).is_some_and(|parents| {
        parents.iter().any(|(id, role)| {
            (role.is_empty() || role == "outer")
                && manifest
                    .relations
                    .get(id)
                    .is_some_and(|relation| relation.feature_types.contains(kind))
        })
    })
}

#[allow(clippy::too_many_arguments)]
fn emit_linear_way(
    way_id: i64,
    ftype: &FeatureType,
    resolved_nodes: &[transport::ResolvedNode],
    tags: &Tags,
    junctions: &NodeIdBitmap,
    spiller: &mut Spiller,
    piece_squares: &mut BTreeSet<u32>,
    stats: &mut Pass2Stats,
) -> Result<()> {
    let max_len = 250.0;
    let segs = microsegment::split_at_junctions(
        resolved_nodes
            .iter()
            .map(|(id, coords)| (*coords, junctions.contains(*id))),
        max_len,
    );
    assert!(
        segs.len() <= i16::MAX as usize + 1,
        "way {way_id} exceeds nonnegative Int16 segment identities",
    );
    // Whole-way metres are summed once here, while the complete chain is in hand.
    let railway_metres =
        matches!(ftype, FeatureType::Railway).then(|| transport::way_metres(resolved_nodes));
    for (idx, interval) in segs.iter().enumerate() {
        let seg = interval.geometry(|index| {
            resolved_nodes[index]
                .1
                .expect("source interval references a resolved node")
        });
        let mid_lat = (seg.0[0] + seg.1[0]) / 2.0;
        let mid_lon = grid::geo::wrapped_longitude_midpoint(seg.0[1], seg.1[1]);
        let square = grid::square_of(mid_lat, mid_lon);
        let piece_tail = matches!(ftype, FeatureType::Road | FeatureType::Railway).then(|| {
            let tail = transport::piece_tail(
                resolved_nodes,
                interval,
                railway_metres.as_ref().map(|metres| metres.as_deref()),
            );
            format!("{}\t{tail}", transport::way_extent(resolved_nodes))
        });
        spiller.emit_segment(
            ftype,
            square,
            way_id,
            idx as i16,
            &seg,
            tags,
            piece_tail.as_deref(),
        )?;
        // Only `railway-ways` records the squares of a way's pieces.
        if railway_metres.is_some() {
            piece_squares.insert(spill::spill_key(square));
        }
        stats.features_total += 1;
    }
    Ok(())
}

/// Spill a classified point source, returning its row count.
fn emit_node(
    spiller: &mut spill::Spiller,
    tags: &classify::Tags,
    ftype: classify::FeatureType,
    osm_id: i64,
    lat: f64,
    lon: f64,
) -> Result<u64> {
    let square = grid::square_of(lat, lon);
    spiller.emit_polygon(&ftype, square, osm_id, "node", lat, lon, tags, None)?;
    Ok(1)
}

/// Spill one settlement NODE: a `Leisure` node becomes a point leisure source
/// (centroid only, no ring → default area), a `Poi` node spills to the
/// finalize footprint-join file. Returns 1 if a row was written, else 0.
fn emit_settlement_node(
    spiller: &mut spill::Spiller,
    kind: classify::FeatureType,
    osm_id: i64,
    lat: f64,
    lon: f64,
    tags: &classify::Tags,
) -> Result<u64> {
    let square = grid::square_of(lat, lon);
    match kind {
        classify::FeatureType::Leisure => {
            spiller.emit_polygon(&kind, square, osm_id, "node", lat, lon, tags, None)?;
            Ok(1)
        }
        classify::FeatureType::Poi => match spill::poi_class_from_tags(tags) {
            Some(class) => {
                spiller.emit_poi(square, lat, lon, class)?;
                Ok(1)
            }
            None => Ok(0),
        },
        _ => Ok(0),
    }
}

pub(crate) fn centroid(coords: &[[f64; 2]]) -> (f64, f64) {
    let coords = if coords.len() > 1 && coords.first() == coords.last() {
        &coords[..coords.len() - 1]
    } else {
        coords
    };
    let n = coords.len() as f64;
    let reference_lon = coords[0][1];
    (
        coords.iter().map(|c| c[0]).sum::<f64>() / n,
        grid::geo::normalize_longitude(
            reference_lon
                + coords
                    .iter()
                    .map(|c| grid::geo::wrapped_longitude_delta(reference_lon, c[1]))
                    .sum::<f64>()
                    / n,
        ),
    )
}

/// Canonical longitudes make a dateline-crossing ring look almost world-wide
/// to current polygon consumers. Preserve its correct centroid but omit the
/// unsafe ring until the shared polygon format can represent wrapped geometry.
pub(crate) fn ring_for_spill<'a>(
    coords: &'a [[f64; 2]],
    antimeridian_rings_omitted: &mut u64,
) -> Option<&'a [[f64; 2]]> {
    if coords.len() < 3 {
        return None;
    }
    let (min_lon, max_lon) = coords.iter().fold(
        (f64::INFINITY, f64::NEG_INFINITY),
        |(min_lon, max_lon), coord| (min_lon.min(coord[1]), max_lon.max(coord[1])),
    );
    if max_lon - min_lon > 180.0 {
        *antimeridian_rings_omitted += 1;
        None
    } else {
        Some(coords)
    }
}

/// One tag set per outer part with the facility nameplate shared by area.
/// A relation's `plant:output:electricity` (solar) and `rating` (substation
/// own nameplate) describe the whole facility, not each part: without sharing,
/// a two-part 24 MW solar relation becomes two 24 MW sources (+3.01 dB).
/// Each closed part gets its area fraction of the parsed total, formatted back
/// in the parser's canonical units (MW, MVA); unclosed fragments keep the
/// original tags (the spill drops them) and single-part or untagged relations
/// keep theirs byte-identical. Areas use the same snapped-grid shoelace as the
/// stored `area_m2`, falling back to an equal split when nothing measures.
fn shared_part_tags(rings: &[Vec<[f64; 2]>], tags: &Tags) -> Vec<Tags> {
    let closed: Vec<bool> = rings
        .iter()
        .map(|ring| crate::classify::is_a_closed_ring(ring))
        .collect();
    let emitting = closed.iter().filter(|&&c| c).count();
    if emitting < 2 {
        return vec![tags.clone(); rings.len()];
    }
    let btree: BTreeMap<String, String> = tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let total_mw = square_store::osm_evidence::plant_output_mw(&btree);
    let total_mva = square_store::osm_evidence::transformer_rating_mva(&btree);
    if total_mw.is_none() && total_mva.is_none() {
        return vec![tags.clone(); rings.len()];
    }
    let areas: Vec<f64> = rings
        .iter()
        .zip(closed.iter())
        .map(|(ring, &is_closed)| {
            if !is_closed {
                return 0.0;
            }
            let snapped: Vec<(i32, i32)> =
                ring.iter().map(|c| grid::lonlat_to_grid(c[1], c[0])).collect();
            grid::poly::ring_area_m2(&snapped).unwrap_or(0.0)
        })
        .collect();
    let total_area: f64 = areas.iter().sum();
    rings
        .iter()
        .zip(closed.iter())
        .zip(areas.iter())
        .map(|((_, &is_closed), &area)| {
            if !is_closed {
                return tags.clone();
            }
            let share = if total_area > 0.0 {
                area / total_area
            } else {
                1.0 / emitting as f64
            };
            let mut part = tags.clone();
            if let Some(mw) = total_mw {
                part.insert(
                    "plant:output:electricity".to_string(),
                    format!("{} MW", mw * share),
                );
            }
            if let Some(mva) = total_mva {
                part.insert("rating".to_string(), format!("{} MVA", mva * share));
            }
            part
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn outer_building_keeps_its_independent_source_family() {
        use crate::classify::{FeatureType, Tags};
        use crate::relations::{RelationInfo, RelationManifest};
        let manifest = RelationManifest {
            way_to_relations: [
                (1, vec![(2, "outer".to_owned())]),
                (3, vec![(2, "inner".to_owned())]),
            ]
            .into(),
            relations: [(
                2,
                RelationInfo {
                    feature_types: vec![FeatureType::Building],
                    tags: Tags::new(),
                    member_ways: vec![],
                },
            )]
            .into(),
        };
        assert!(super::relation_covers_kind(
            1,
            &FeatureType::Building,
            &manifest
        ));
        assert!(!super::relation_covers_kind(
            1,
            &FeatureType::Industrial,
            &manifest
        ));
        assert!(!super::relation_covers_kind(
            1,
            &FeatureType::Leisure,
            &manifest
        ));
        assert!(!super::relation_covers_kind(
            3,
            &FeatureType::Building,
            &manifest
        ));
    }

    use super::{centroid, ring_for_spill, shared_part_tags};

    #[test]
    fn multipolygon_nameplate_is_shared_by_area_over_the_parts() {
        use crate::classify::Tags;
        use std::collections::BTreeMap;
        // Two equal closed halves plus one unclosed fragment: the halves split
        // the 24 MW total (12 MW each, energy-conserving), the fragment keeps
        // the original tags (the spill drops it) and a single part stays
        // byte-identical.
        let half = |lon0: f64| {
            vec![
                [50.0, lon0],
                [50.0, lon0 + 0.0001],
                [50.0001, lon0 + 0.0001],
                [50.0001, lon0],
                [50.0, lon0],
            ]
        };
        let tags: Tags =
            [("plant:output:electricity".to_string(), "24 MW".to_string())].into();
        let parts = shared_part_tags(&[half(14.0), half(14.0002)], &tags);
        assert_eq!(parts.len(), 2);
        for part in &parts {
            let btree: BTreeMap<String, String> =
                part.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            assert_eq!(
                square_store::osm_evidence::plant_output_mw(&btree),
                Some(12.0)
            );
        }
        let double = vec![
            [50.0, 14.0],
            [50.0, 14.0002],
            [50.0001, 14.0002],
            [50.0001, 14.0],
            [50.0, 14.0],
        ];
        let open = vec![[50.0, 14.0], [50.0, 14.0001]];
        let parts = shared_part_tags(&[half(14.0), double, open], &tags);
        let mw = |part: &Tags| {
            let btree: BTreeMap<String, String> =
                part.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            square_store::osm_evidence::plant_output_mw(&btree).unwrap()
        };
        // The double-area part carries twice the single-area part (8 + 16 MW,
        // within snapped-grid rounding).
        assert!((mw(&parts[0]) - 8.0).abs() < 0.02, "{}", mw(&parts[0]));
        assert!((mw(&parts[1]) - 16.0).abs() < 0.02, "{}", mw(&parts[1]));
        assert_eq!(parts[2].get("plant:output:electricity").unwrap(), "24 MW");
        let single = shared_part_tags(&[half(14.0)], &tags);
        assert_eq!(single[0].get("plant:output:electricity").unwrap(), "24 MW");
    }

    #[test]
    fn antimeridian_ring_keeps_its_centroid_but_not_unsafe_geometry() {
        let ring = [
            [10.0, 179.99],
            [10.0, -179.99],
            [10.01, -179.99],
            [10.01, 179.99],
            [10.0, 179.99],
        ];
        let (lat, lon) = centroid(&ring);
        assert!((lat - 10.005).abs() < 1e-12);
        assert!((lon + 180.0).abs() < 1e-12, "lon={lon}");
        let mut omitted = 0;
        assert!(ring_for_spill(&ring, &mut omitted).is_none());
        assert_eq!(omitted, 1);
    }
}

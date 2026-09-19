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
            if let Some((ring, tags, ftype)) = assembler.assemble(rel_id, manifest) {
                let extracted_tags = relations::spill_tags_for_assembled(&ftype, &tags);
                let (clat, clon) = centroid(&ring);
                let square = grid::square_of(clat, clon);
                let safe_ring = ring_for_spill(&ring, &mut stats.antimeridian_rings_omitted);
                spiller.emit_polygon(
                    &ftype,
                    square,
                    rel_id,
                    clat,
                    clon,
                    &extracted_tags,
                    safe_ring,
                )?;
                stats.features_total += 1;
                stats.rels_assembled += 1;
            }
            assembler.cleanup(rel_id, manifest);
        }
    }

    let Some(ftype) = way.class else {
        return Ok(());
    };
    // Skip if this way is an outer member of a polygon relation; the relation's
    // assembled multipolygon already covers it. An INNER building is not covered:
    // the assembler keeps outer rings only, so a tagged inner way (a shop inside
    // a campus, a house in a courtyard) is its own object and must be emitted —
    // the parent's own row defers to the buildings mapped inside it.
    if way.is_outer_relation_member
        && matches!(
            ftype,
            FeatureType::Building
                | FeatureType::Industrial
                | FeatureType::AirportArea
                | FeatureType::AirportLine
        )
    {
        return Ok(());
    }

    let is_transport = matches!(ftype, FeatureType::Road | FeatureType::Railway);
    if is_transport {
        transport.observe_way(ftype.name(), &way.resolved_nodes);
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
        let ring = ring_for_spill(&coords, &mut stats.antimeridian_rings_omitted);
        spiller.emit_polygon(&ftype, square, way.id, clat, clon, &way.tags, ring)?;
        stats.features_total += 1;
    }
    if matches!(ftype, FeatureType::Railway) {
        // Every classified railway way is kept, even without pieces: a train route naming it
        // must resolve to `missing_source_ways`, not to an unknown member.
        transport.write_railway_way(way.id, &way.resolved_nodes, &piece_squares)?;
    }
    Ok(())
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
            transport::piece_tail(
                resolved_nodes,
                interval,
                railway_metres.as_ref().map(|metres| metres.as_deref()),
            )
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
    spiller.emit_polygon(&ftype, square, osm_id, lat, lon, tags, None)?;
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
            spiller.emit_polygon(&kind, square, osm_id, lat, lon, tags, None)?;
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

#[cfg(test)]
mod tests {
    use super::{centroid, ring_for_spill};

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

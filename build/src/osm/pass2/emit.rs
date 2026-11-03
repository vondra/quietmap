//! Emit decoded nodes, ways and relation parts with their retained source identity.

use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_linear_way(
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
        let mid_lon = crate::osm::geometry::wrapped_longitude_midpoint(seg.0[1], seg.1[1]);
        let square = crate::osm::geometry::square_of(mid_lat, mid_lon);
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
pub(super) fn emit_node(
    spiller: &mut spill::Spiller,
    tags: &classify::Tags,
    ftype: classify::FeatureType,
    osm_id: i64,
    lat: f64,
    lon: f64,
) -> Result<u64> {
    let square = crate::osm::geometry::square_of(lat, lon);
    spiller.emit_polygon(&ftype, square, osm_id, "node", lat, lon, tags, None)?;
    Ok(1)
}

/// Spill one settlement NODE: a `Leisure` node becomes a point leisure source
/// (centroid only, no ring → default area), a `Poi` node spills to the
/// finalize footprint-join file. Returns 1 if a row was written, else 0.
pub(super) fn emit_settlement_node(
    spiller: &mut spill::Spiller,
    kind: classify::FeatureType,
    osm_id: i64,
    lat: f64,
    lon: f64,
    tags: &classify::Tags,
) -> Result<u64> {
    let square = crate::osm::geometry::square_of(lat, lon);
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
        crate::osm::geometry::normalize_longitude(
            reference_lon
                + coords
                    .iter()
                    .map(|c| crate::osm::geometry::wrapped_longitude_delta(reference_lon, c[1]))
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

/// One tag set per outer part: every closed part keeps the facility's full
/// nameplate byte-identical and carries its area fraction in
/// `qm:facility_share`, so readers evaluate the facility's sound power once
/// and share it by area. Sharing the MVA/MW itself cannot conserve power:
/// the substation law is logarithmic (ten equal parts of 100 MVA read
/// −4 dB), and a part holding the rated transformers would double-count the
/// tag share of the others. Unclosed fragments keep the original tags (the
/// spill drops them) and single parts stay byte-identical (share 1.0 is the
/// reader default). Areas use the same snapped-grid shoelace as the stored
/// `area_m2`; a zero total leaves the parts unshared rather than split.
pub(super) fn facility_part_tags(rings: &[Vec<[f64; 2]>], tags: &Tags) -> Vec<Tags> {
    let closed: Vec<bool> = rings
        .iter()
        .map(|ring| crate::osm::classify::is_a_closed_ring(ring))
        .collect();
    if closed.iter().filter(|&&c| c).count() < 2 {
        return vec![tags.clone(); rings.len()];
    }
    let areas: Vec<f64> = rings
        .iter()
        .zip(closed.iter())
        .map(|(ring, &is_closed)| {
            if !is_closed {
                return 0.0;
            }
            let snapped: Vec<(i32, i32)> = ring
                .iter()
                .map(|c| crate::osm::geometry::lonlat_to_grid(c[1], c[0]))
                .collect();
            crate::osm::geometry::poly::ring_area_m2(&snapped).unwrap_or(0.0)
        })
        .collect();
    let total_area: f64 = areas.iter().sum();
    if total_area <= 0.0 {
        return vec![tags.clone(); rings.len()];
    }
    rings
        .iter()
        .zip(closed.iter())
        .zip(areas.iter())
        .map(|((_, &is_closed), &area)| {
            if !is_closed {
                return tags.clone();
            }
            let mut part = tags.clone();
            part.insert(
                "qm:facility_share".to_string(),
                format!("{}", area / total_area),
            );
            part
        })
        .collect()
}

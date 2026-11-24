//! Compact year-level index of nearby unmodelled sites (active military firing ranges and
//! training areas), built from the same facility input as `sources` and `source-outlines`.

use crate::dev4::{Square, Z30_QUANTUM_M, z30_corner_degrees};
use crate::facilities::{geometry, records::INDUSTRY};
use crate::osm::geometry::poly::{MIN_FOOTPRINT_AREA_M2, ring_area_m2};
use crate::sources::facilities::parse_tags;
use crate::sources::group_key;
use geos::{CoordSeq, Geom, Geometry};
use physics::emission::industrial::SOURCE_MILITARY_RANGE;
use rstar::{AABB, RTree, RTreeObject};
use std::collections::BTreeMap;
use std::path::Path;
use tiles::sources::Layer;
use tiles::unmodelled::{SitePart, SiteRecord, UnmodelledKind, notification_reach_m, write};

/// Shared-boundary tolerance (ground metres) when precomputing whether a mapped sub-sector lies
/// inside a larger range (OSM training sectors often share outer roads with the enclosing estate).
const COVER_EDGE_TOLERANCE_M: f64 = 10.0;
const WORLD_HALF_M: f64 = std::f64::consts::PI * tiles::geo::WGS84_A_M;

fn military_kind(osm_tags: &str) -> Option<UnmodelledKind> {
    let tags = parse_tags(osm_tags);
    match tags.get("military").map(String::as_str) {
        Some("range") => Some(UnmodelledKind::MilitaryRange),
        Some("training_area") => Some(UnmodelledKind::MilitaryTrainingArea),
        Some("danger_area") => Some(UnmodelledKind::MilitaryDangerArea),
        _ => None,
    }
}

fn net_part_area_m2(
    stored_area: Option<f32>,
    outer: &[(i32, i32)],
    holes: &[Vec<(i32, i32)>],
) -> f64 {
    if let Some(area) = stored_area.map(f64::from)
        && area.is_finite()
        && area > 0.0
    {
        return area;
    }
    let outer_area = ring_area_m2(outer).unwrap_or(0.0);
    let holes_area: f64 = holes.iter().filter_map(|h| ring_area_m2(h)).sum();
    (outer_area - holes_area).max(MIN_FOOTPRINT_AREA_M2)
}

fn z30_to_mercator(gx: i32, gy: i32) -> (f64, f64) {
    (
        (i64::from(gx) - (1 << 29)) as f64 * Z30_QUANTUM_M,
        (i64::from(gy) - (1 << 29)) as f64 * Z30_QUANTUM_M,
    )
}

fn ring_to_geos_linear_ring(ring: &[(i32, i32)], ref_x: f64) -> Option<Geometry> {
    if ring.len() < 3 {
        return None;
    }
    let mut coords: Vec<[f64; 2]> = Vec::with_capacity(ring.len() + 1);
    for &(gx, gy) in ring {
        let (mut x, y) = z30_to_mercator(gx, gy);
        let dx = x - ref_x;
        if dx > WORLD_HALF_M {
            x -= 2.0 * WORLD_HALF_M;
        } else if dx < -WORLD_HALF_M {
            x += 2.0 * WORLD_HALF_M;
        }
        coords.push([x, y]);
    }
    if coords.first() != coords.last() {
        coords.push(coords[0]);
    }
    if coords.len() < 4 {
        return None;
    }
    let seq = CoordSeq::new_from_vec(&coords).ok()?;
    Geometry::create_linear_ring(seq).ok()
}

fn repaired(geom: Geometry) -> Option<Geometry> {
    if geom.is_valid().unwrap_or(false) {
        Some(geom)
    } else {
        geom.buffer(0.0, 4).ok()
    }
}

struct SiteGeometry {
    geom: Geometry,
    area_merc: f64,
    cos_lat: f64,
    bbox: AABB<[f64; 2]>,
}

fn build_site_geometry(parts: &[SitePart]) -> Option<SiteGeometry> {
    let &(rx, ry) = parts.first()?.rings.first()?.first()?;
    let (ref_x, _) = z30_to_mercator(rx, ry);
    let (ref_lat, _) = z30_corner_degrees(rx, ry);
    let cos_lat = ref_lat.abs().min(84.5).to_radians().cos().max(0.05);
    let mut polys = Vec::with_capacity(parts.len());
    for part in parts {
        let outer = ring_to_geos_linear_ring(part.rings.first()?, ref_x)?;
        let holes: Vec<Geometry> = part.rings[1..]
            .iter()
            .filter_map(|hole| ring_to_geos_linear_ring(hole, ref_x))
            .collect();
        let poly = Geometry::create_polygon(outer, holes).ok()?;
        let poly = repaired(poly)?;
        if !poly.is_empty().unwrap_or(true) {
            polys.push(poly);
        }
    }
    if polys.is_empty() {
        return None;
    }
    let geom = if polys.len() == 1 {
        polys.pop()?
    } else {
        let multi = Geometry::create_multipolygon(polys).ok()?;
        repaired(multi)?
    };
    let area_merc = geom.area().ok()?;
    if !(area_merc.is_finite() && area_merc > 0.0) {
        return None;
    }
    let bbox = AABB::from_corners(
        [geom.get_x_min().ok()?, geom.get_y_min().ok()?],
        [geom.get_x_max().ok()?, geom.get_y_max().ok()?],
    );
    Some(SiteGeometry {
        geom,
        area_merc,
        cos_lat,
        bbox,
    })
}

struct IndexedSite {
    idx: usize,
    bbox: AABB<[f64; 2]>,
}

impl RTreeObject for IndexedSite {
    type Envelope = AABB<[f64; 2]>;
    fn envelope(&self) -> Self::Envelope {
        self.bbox
    }
}

/// Precompute `covered_by` on `records` using exact GEOS polygon topology (outer rings minus
/// holes, buffered by [`COVER_EDGE_TOLERANCE_M`] for shared roads/boundaries while requiring >50 %
/// unbuffered area overlap so touching exterior ranges and slivers outside the fence are kept).
fn compute_coverage(records: &mut [SiteRecord]) {
    let geoms: Vec<Option<SiteGeometry>> = records
        .iter()
        .map(|rec| build_site_geometry(&rec.parts))
        .collect();
    let entries: Vec<IndexedSite> = geoms
        .iter()
        .enumerate()
        .filter_map(|(idx, g)| {
            Some(IndexedSite {
                idx,
                bbox: g.as_ref()?.bbox,
            })
        })
        .collect();
    let tree = RTree::bulk_load(entries);
    let mut buffered_parents: Vec<Option<Geometry>> = vec![None; records.len()];

    for child_idx in 0..records.len() {
        let Some(child_geom) = geoms[child_idx].as_ref() else {
            continue;
        };
        let mut covering = Vec::new();
        for entry in tree.locate_in_envelope_intersecting(&child_geom.bbox) {
            let parent_idx = entry.idx;
            if parent_idx == child_idx {
                continue;
            }
            let parent_beats = records[parent_idx].area_m2 > records[child_idx].area_m2
                || (records[parent_idx].area_m2 == records[child_idx].area_m2
                    && records[parent_idx].group_key < records[child_idx].group_key);
            if !parent_beats {
                continue;
            }
            let Some(parent_geom) = geoms[parent_idx].as_ref() else {
                continue;
            };
            let tol_merc = COVER_EDGE_TOLERANCE_M / parent_geom.cos_lat;
            // Quick bbox check before GEOS predicates.
            if child_geom.bbox.lower()[0] < parent_geom.bbox.lower()[0] - tol_merc
                || child_geom.bbox.lower()[1] < parent_geom.bbox.lower()[1] - tol_merc
                || child_geom.bbox.upper()[0] > parent_geom.bbox.upper()[0] + tol_merc
                || child_geom.bbox.upper()[1] > parent_geom.bbox.upper()[1] + tol_merc
            {
                continue;
            }
            let parent_buf = buffered_parents[parent_idx].get_or_insert_with(|| {
                parent_geom.geom.buffer(tol_merc, 4).unwrap_or_else(|_| {
                    geos::Geom::clone(&parent_geom.geom).expect("clone geos geometry")
                })
            });
            let covered = parent_buf.covers(&child_geom.geom).unwrap_or(false);
            if !covered {
                continue;
            }
            let overlap = child_geom
                .geom
                .intersection(&parent_geom.geom)
                .and_then(|g| g.area())
                .unwrap_or(0.0);
            if overlap > 0.5 * child_geom.area_merc {
                covering.push(records[parent_idx].group_key);
            }
        }
        records[child_idx].covered_by = covering;
    }
}

/// Group all active military range parts across `squares` by `group_key`, sum their net area
/// before selecting notification reach, precompute nested coverage, and write `<year>/unmodelled-sites`.
pub fn build(
    facilities: &crate::facilities::Dataset,
    squares: &[Square],
    out: &Path,
) -> Result<(), String> {
    struct Grouped {
        kind: UnmodelledKind,
        name: String,
        area_m2: f64,
        parts: Vec<SitePart>,
    }

    let mut grouped: BTreeMap<u64, Grouped> = BTreeMap::new();
    let mut read = 0usize;
    for &square in squares {
        for row in facilities.rows(square, INDUSTRY)? {
            // Filter on source_type before decoding WKB geometry (skips >99.8 % of industrial rows).
            if row.source_type != SOURCE_MILITARY_RANGE
                || row.suppressed != 0
                || row.operating_state == 2
            {
                continue;
            }
            let Some(kind) = military_kind(&row.osm_tags) else {
                continue;
            };
            let rings = geometry::grid_rings(row.geometry.as_deref()).map_err(|e| e.to_string())?;
            let Some(outer) = rings.first() else {
                continue;
            };
            if outer.len() < 3 {
                continue;
            }
            read += 1;
            let part_area = net_part_area_m2(row.area_m2, outer, &rings[1..]);
            let valid_rings: Vec<Vec<(i32, i32)>> =
                rings.into_iter().filter(|r| r.len() >= 3).collect();
            let part = SitePart { rings: valid_rings };
            let key = group_key(&["industry", &row.osm_kind, &row.osm_id.to_string()]);
            let name = row.name.unwrap_or_default();
            let entry = grouped.entry(key).or_insert_with(|| Grouped {
                kind,
                name: name.clone(),
                area_m2: 0.0,
                parts: Vec::new(),
            });
            if entry.name.is_empty() && !name.is_empty() {
                entry.name = name;
            }
            if !entry.parts.contains(&part) {
                entry.area_m2 += part_area;
                entry.parts.push(part);
            }
        }
    }

    let mut records: Vec<SiteRecord> = grouped
        .into_iter()
        .map(|(group_key, site)| SiteRecord {
            group_key,
            layer: Layer::Industry,
            kind: site.kind,
            name: site.name,
            area_m2: site.area_m2,
            reach_m: notification_reach_m(site.area_m2),
            covered_by: Vec::new(),
            parts: site.parts,
        })
        .collect();
    compute_coverage(&mut records);
    let covered_edges: usize = records.iter().map(|r| r.covered_by.len()).sum();
    write(out, &records)?;
    eprintln!(
        "unmodelled sites: {read} input rows, {} grouped sites, {covered_edges} nested cover links",
        records.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dev4::degrees_to_z30;

    fn rect(lat0: f64, lon0: f64, lat1: f64, lon1: f64) -> Vec<(i32, i32)> {
        vec![
            degrees_to_z30(lat0, lon0),
            degrees_to_z30(lat0, lon1),
            degrees_to_z30(lat1, lon1),
            degrees_to_z30(lat1, lon0),
        ]
    }

    fn site(key: u64, area_m2: f64, rings: Vec<Vec<(i32, i32)>>) -> SiteRecord {
        SiteRecord {
            group_key: key,
            layer: Layer::Industry,
            kind: UnmodelledKind::MilitaryTrainingArea,
            name: format!("Site {key}"),
            area_m2,
            reach_m: notification_reach_m(area_m2),
            covered_by: Vec::new(),
            parts: vec![SitePart { rings }],
        }
    }

    #[test]
    fn geos_coverage_suppresses_nested_and_shared_edge_sectors_but_keeps_holes_and_neighbours() {
        // Parent 10 km x 10 km with a 2 km x 2 km central hole.
        let parent = site(
            1,
            100_000_000.0,
            vec![
                rect(50.0, 14.0, 50.1, 14.14),
                rect(50.04, 14.05, 50.06, 14.09),
            ],
        );
        // Sub-sector sharing south/west edge + slight 3 m road offset -> covered.
        let shared = site(2, 6_000_000.0, vec![rect(49.99997, 14.0, 50.03, 14.04)]);
        // Adjacent neighbour touching east edge outside parent -> NOT covered.
        let adjacent = site(3, 10_000_000.0, vec![rect(50.0, 14.14, 50.05, 14.20)]);
        // Tiny external sliver 2 m south of parent fence (~5 m wide) -> NOT covered.
        let sliver = site(4, 500.0, vec![rect(49.99993, 14.05, 49.99998, 14.051)]);
        // Child whose vertices sit on solid ground around the hole, spanning the hole -> NOT covered!
        let over_hole = site(5, 8_000_000.0, vec![rect(50.035, 14.045, 50.065, 14.095)]);

        let mut records = vec![parent, shared, adjacent, sliver, over_hole];
        compute_coverage(&mut records);
        assert!(records[0].covered_by.is_empty());
        assert_eq!(records[1].covered_by, vec![1]);
        assert!(records[2].covered_by.is_empty());
        assert!(records[3].covered_by.is_empty());
        assert!(records[4].covered_by.is_empty());
    }
}

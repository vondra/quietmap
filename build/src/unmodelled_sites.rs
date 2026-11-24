//! Compact year-level index of nearby unmodelled sites (active military firing ranges and
//! training areas), built from the same facility input as `sources` and `source-outlines`.

use crate::dev4::Square;
use crate::osm::geometry::poly::{MIN_FOOTPRINT_AREA_M2, ring_area_m2};
use crate::sources::industry::{IndustrialRow, read_rows};
use physics::emission::industrial::SOURCE_MILITARY_RANGE;
use std::collections::BTreeMap;
use std::path::Path;
use tiles::sources::Layer;
use tiles::unmodelled::{SitePart, SiteRecord, UnmodelledKind, notification_reach_m, write};

fn military_kind(row: &IndustrialRow) -> Option<UnmodelledKind> {
    match row.tags.get("military").map(String::as_str) {
        Some("range") => Some(UnmodelledKind::MilitaryRange),
        Some("training_area") => Some(UnmodelledKind::MilitaryTrainingArea),
        Some("danger_area") => Some(UnmodelledKind::MilitaryDangerArea),
        _ => None,
    }
}

fn net_part_area_m2(row: &IndustrialRow) -> f64 {
    if let Some(area) = row.area_m2
        && area.is_finite()
        && area > 0.0
    {
        return area;
    }
    let outer = ring_area_m2(&row.ring).unwrap_or(0.0);
    let holes: f64 = row.holes.iter().filter_map(|h| ring_area_m2(h)).sum();
    (outer - holes).max(MIN_FOOTPRINT_AREA_M2)
}

/// Group all active military range parts across `squares` by `group_key`, sum their net area
/// before selecting notification reach, and write `<year>/unmodelled-sites`.
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
        for row in read_rows(facilities, square)? {
            if row.source_type != SOURCE_MILITARY_RANGE || row.suppressed || row.ring.len() < 3 {
                continue;
            }
            let Some(kind) = military_kind(&row) else {
                continue;
            };
            read += 1;
            let mut rings = Vec::with_capacity(1 + row.holes.len());
            rings.push(row.ring.clone());
            for hole in &row.holes {
                if hole.len() >= 3 {
                    rings.push(hole.clone());
                }
            }
            let part = SitePart { rings };
            let part_area = net_part_area_m2(&row);
            let entry = grouped.entry(row.group_key()).or_insert_with(|| Grouped {
                kind,
                name: row.name.clone(),
                area_m2: 0.0,
                parts: Vec::new(),
            });
            if entry.name.is_empty() && !row.name.is_empty() {
                entry.name = row.name.clone();
            }
            if !entry.parts.contains(&part) {
                entry.area_m2 += part_area;
                entry.parts.push(part);
            }
        }
    }

    let records: Vec<SiteRecord> = grouped
        .into_iter()
        .map(|(group_key, site)| SiteRecord {
            group_key,
            layer: Layer::Industry,
            kind: site.kind,
            name: site.name,
            area_m2: site.area_m2,
            reach_m: notification_reach_m(site.area_m2),
            parts: site.parts,
        })
        .collect();
    write(out, &records)?;
    eprintln!(
        "unmodelled sites: {read} input rows, {} grouped sites",
        records.len()
    );
    Ok(())
}

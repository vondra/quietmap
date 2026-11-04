//! Construct crossing approaches and merge their shared same-direction soundings.
use super::types::Crossing;
use arrow_array::RecordBatch;
const HORN_RAIL_TYPE: u8 = 6;
const MATCH_GATE_M: f64 = 60.0;
// 49 CFR 222.21: twenty seconds, bounded by one quarter mile.
const SOUNDING_LEAD_S: f64 = 20.0;
const HORN_ZONE_M: f64 = 402.336;
const MERGE_DIR_DOT: f64 = 0.9;
const MERGE_LATERAL_M: f64 = 5.0;
pub(super) struct Approach {
    pub crossing: usize,
    pub approach_idx: usize,
    pub matched_row: u32,
    pub start_lon: f64,
    pub start_lat: f64,
    pub end_lon: f64,
    pub end_lat: f64,
    pub dir_x: f64,
    pub dir_y: f64,
    pub length_m: f64,
    pub soundings: [f64; 3],
}

pub(super) struct BaseRow {
    pub start_lon: f64,
    pub start_lat: f64,
    pub end_lon: f64,
    pub end_lat: f64,
}

pub(super) fn base_rows(base: &RecordBatch) -> Result<(Vec<BaseRow>, Vec<u8>), String> {
    let start_gx = super::write::col_i32(base, "start_gx")?;
    let start_gy = super::write::col_i32(base, "start_gy")?;
    let end_gx = super::write::col_i32(base, "end_gx")?;
    let end_gy = super::write::col_i32(base, "end_gy")?;
    let rail_type = super::write::col_u8(base, "rail_type")?;
    let mut rows = Vec::with_capacity(base.num_rows());
    let mut types = Vec::with_capacity(base.num_rows());
    for row in 0..base.num_rows() {
        let (s_lon, s_lat) =
            crate::osm::geometry::grid_cell_lonlat(start_gx.value(row), start_gy.value(row));
        let (e_lon, e_lat) =
            crate::osm::geometry::grid_cell_lonlat(end_gx.value(row), end_gy.value(row));
        rows.push(BaseRow {
            start_lon: s_lon,
            start_lat: s_lat,
            end_lon: e_lon,
            end_lat: e_lat,
        });
        types.push(rail_type.value(row));
    }
    Ok((rows, types))
}

pub(super) fn match_crossing(
    crossing: &Crossing,
    rows: &[BaseRow],
    types: &[u8],
) -> Option<(u32, f64, f64)> {
    let mut best: Option<(f64, u32, f64, f64)> = None;
    for (row, base) in rows.iter().enumerate() {
        if types[row] == HORN_RAIL_TYPE {
            continue;
        }
        let (dist, cp_lat, cp_lon, _) = crate::osm::geometry::point_to_segment(
            crossing.lat,
            crossing.lon,
            base.start_lat,
            base.start_lon,
            base.end_lat,
            base.end_lon,
        );
        if dist <= MATCH_GATE_M && best.is_none_or(|(d, _, _, _)| dist < d) {
            best = Some((dist, row as u32, cp_lat, cp_lon));
        }
    }
    best.map(|(_, row, cp_lat, cp_lon)| (row, cp_lat, cp_lon))
}

pub(super) fn build_approaches(
    crossing_idx: usize,
    crossing: &Crossing,
    matched_row: u32,
    cp_lat: f64,
    cp_lon: f64,
    rows: &[BaseRow],
) -> [Approach; 2] {
    let base = &rows[matched_row as usize];
    // Local-metre direction of the matched row at the crossing.
    let m_lon = crate::osm::geometry::m_per_deg_lon(cp_lat.to_radians());
    let dx = crate::osm::geometry::wrapped_longitude_delta(base.start_lon, base.end_lon) * m_lon;
    let dy = (base.end_lat - base.start_lat) * crate::osm::geometry::M_PER_DEG_LAT;
    let len = (dx * dx + dy * dy).sqrt().max(1e-6);
    let (ux, uy) = (dx / len, dy / len);
    // Approach length: the train sounds SOUNDING_LEAD_S before arrival, but
    // the whistle post stands at most 1/4 mi out.
    let v_ms = crossing.speed_kmh.unwrap_or(60.0) / 3.6;
    let zone = (v_ms * SOUNDING_LEAD_S).min(HORN_ZONE_M);
    let half = [
        crossing.soundings[0] / 2.0,
        crossing.soundings[1] / 2.0,
        crossing.soundings[2] / 2.0,
    ];
    [1.0, -1.0].map(|sign| {
        let (sx, sy) = (-sign * ux * zone, -sign * uy * zone);
        Approach {
            crossing: crossing_idx,
            approach_idx: if sign > 0.0 { 0 } else { 1 },
            matched_row,
            start_lon: cp_lon + sx / m_lon,
            start_lat: cp_lat + sy / crate::osm::geometry::M_PER_DEG_LAT,
            end_lon: cp_lon,
            end_lat: cp_lat,
            dir_x: sign * ux,
            dir_y: sign * uy,
            length_m: zone,
            soundings: half,
        }
    })
}

pub(super) fn merge_overlaps(approaches: &[Approach]) -> Vec<MergedPiece> {
    let totals: Vec<f64> = approaches
        .iter()
        .map(|a| a.soundings.iter().sum())
        .collect();
    let mut pieces = Vec::new();
    for (idx, approach) in approaches.iter().enumerate() {
        // Split fractions along this approach from overlapping neighbours.
        let mut splits = vec![0.0, 1.0];
        let mut neighbours: Vec<(f64, f64, [f64; 3], usize)> = Vec::new();
        for (other_idx, other) in approaches.iter().enumerate() {
            if other_idx == idx {
                continue;
            }
            if approach.dir_x * other.dir_x + approach.dir_y * other.dir_y < MERGE_DIR_DOT {
                continue;
            }
            // Project the other approach's ends onto this axis (local metres
            // at this approach's start; approaches are ≤ 402 m).
            let m_lon = crate::osm::geometry::m_per_deg_lon(approach.start_lat.to_radians());
            let rel = |lon: f64, lat: f64| {
                (
                    crate::osm::geometry::wrapped_longitude_delta(approach.start_lon, lon) * m_lon,
                    (lat - approach.start_lat) * crate::osm::geometry::M_PER_DEG_LAT,
                )
            };
            let (ox0, oy0) = rel(other.start_lon, other.start_lat);
            let (ox1, oy1) = rel(other.end_lon, other.end_lat);
            let lateral = |x: f64, y: f64| (x * approach.dir_y - y * approach.dir_x).abs();
            if lateral(ox0, oy0) > MERGE_LATERAL_M || lateral(ox1, oy1) > MERGE_LATERAL_M {
                continue;
            }
            let along =
                |x: f64, y: f64| (x * approach.dir_x + y * approach.dir_y) / approach.length_m;
            let (mut a, mut b) = (along(ox0, oy0), along(ox1, oy1));
            if a > b {
                std::mem::swap(&mut a, &mut b);
            }
            if b <= 0.0 || a >= 1.0 {
                continue;
            }
            splits.push(a.clamp(0.0, 1.0));
            splits.push(b.clamp(0.0, 1.0));
            neighbours.push((a, b, other.soundings, other_idx));
        }
        splits.sort_by(f64::total_cmp);
        splits.dedup();
        let mut piece_idx = 0i16;
        for window in splits.windows(2) {
            let (a, b) = (window[0], window[1]);
            if b - a < 1e-6 {
                continue;
            }
            let mid = (a + b) / 2.0;
            let mut soundings = approach.soundings;
            let mut owner = idx;
            for (na, nb, ns, other) in &neighbours {
                if *na <= mid && mid <= *nb {
                    for (period, max) in soundings.iter_mut().enumerate() {
                        *max = max.max(ns[period]);
                    }
                    if totals[*other] > totals[owner]
                        || (totals[*other] == totals[owner] && *other < owner)
                    {
                        owner = *other;
                    }
                }
            }
            if owner != idx {
                continue; // the louder (or earlier) approach emits this window
            }
            pieces.push(MergedPiece {
                approach_idx: idx,
                piece_idx,
                frac0: a,
                frac1: b,
                soundings,
            });
            piece_idx += 1;
        }
    }
    pieces
}

#[derive(Debug)]
pub(super) struct MergedPiece {
    pub approach_idx: usize,
    pub piece_idx: i16,
    pub frac0: f64,
    pub frac1: f64,
    pub soundings: [f64; 3],
}

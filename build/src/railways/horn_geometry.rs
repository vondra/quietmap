//! Construct crossing approaches and merge their shared same-direction soundings.
use super::{model::RailModel, types::Crossing};
use crate::osm::geography::Regions;
use arrow_array::RecordBatch;
const HORN_RAIL_TYPE: u8 = 6;
const MATCH_GATE_M: f64 = 60.0;
// 49 CFR 222.21: twenty seconds, bounded by one quarter mile.
const SOUNDING_LEAD_S: f64 = 20.0;
const HORN_ZONE_M: f64 = 402.336;
const MIN_APPROACH_M: f64 = 1.0;
const MERGE_DIR_DOT: f64 = 0.9;
const MERGE_LATERAL_M: f64 = 5.0;
pub(super) const CROSS_SQUARE_HORN_REACH_M: f64 = 2.0 * HORN_ZONE_M + 2.0 * MATCH_GATE_M;
pub(super) struct Approach {
    pub crossing: usize,
    pub synth_osm: i64,
    pub owned: bool,
    pub approach_idx: usize,
    pub matched_row: u32,
    pub country: [u8; 2],
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
    pub rail_type: u8,
    pub country: [u8; 2],
}

pub(super) fn base_rows(base: &RecordBatch) -> Result<Vec<BaseRow>, String> {
    let start_gx = super::write::col_i32(base, "start_gx")?;
    let start_gy = super::write::col_i32(base, "start_gy")?;
    let end_gx = super::write::col_i32(base, "end_gx")?;
    let end_gy = super::write::col_i32(base, "end_gy")?;
    let rail_type = super::write::col_u8(base, "rail_type")?;
    let country_iso = super::write::col_u16(base, "country_iso")?;
    let mut rows = Vec::with_capacity(base.num_rows());
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
            rail_type: rail_type.value(row),
            country: country_iso.value(row).to_le_bytes(),
        });
    }
    Ok(rows)
}

pub(super) fn match_crossing(
    model: &RailModel,
    crossing: &Crossing,
    rows: &[BaseRow],
) -> Option<(u32, f64, f64)> {
    let mut best: Option<(f64, u32, f64, f64)> = None;
    for (row, base) in rows.iter().enumerate() {
        if base.rail_type == HORN_RAIL_TYPE
            || !model.source_applies_to_row(crossing.source_id, base.country)
        {
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

#[allow(clippy::too_many_arguments)]
pub(super) fn build_approaches(
    crossing_idx: usize,
    crossing: &Crossing,
    matched_row: u32,
    cp_lat: f64,
    cp_lon: f64,
    rows: &[BaseRow],
    regions: Option<&Regions>,
    owned: bool,
) -> Vec<Approach> {
    let base = &rows[matched_row as usize];
    let base_code = u16::from_le_bytes(base.country);
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
    let mut out = Vec::with_capacity(2);
    for sign in [1.0, -1.0] {
        let (sx, sy) = (-sign * ux * zone, -sign * uy * zone);
        let full_start_lon = cp_lon + sx / m_lon;
        let full_start_lat = cp_lat + sy / crate::osm::geometry::M_PER_DEG_LAT;
        let frac = regions.map_or(1.0, |regions| {
            regions.ray_exit_fraction(
                [cp_lat, cp_lon],
                [full_start_lat, full_start_lon],
                base_code,
            )
        });
        let length_m = zone * frac;
        if length_m < MIN_APPROACH_M {
            continue;
        }
        out.push(Approach {
            crossing: crossing_idx,
            synth_osm: crossing.synth_osm,
            owned,
            approach_idx: if sign > 0.0 { 0 } else { 1 },
            matched_row,
            country: base.country,
            start_lon: cp_lon + frac * sx / m_lon,
            start_lat: cp_lat + frac * sy / crate::osm::geometry::M_PER_DEG_LAT,
            end_lon: cp_lon,
            end_lat: cp_lat,
            dir_x: sign * ux,
            dir_y: sign * uy,
            length_m,
            soundings: half,
        });
    }
    out
}

pub(super) fn merge_overlaps(approaches: &[Approach]) -> Vec<MergedPiece> {
    let totals: Vec<f64> = approaches
        .iter()
        .map(|a| a.soundings.iter().sum())
        .collect();
    let mut pieces = Vec::new();
    for (idx, approach) in approaches.iter().enumerate() {
        if !approach.owned {
            continue;
        }
        // Split fractions along this approach from overlapping neighbours.
        let mut splits = vec![0.0, 1.0];
        let mut neighbours: Vec<(f64, f64, [f64; 3], usize)> = Vec::new();
        for (other_idx, other) in approaches.iter().enumerate() {
            if other_idx == idx || approach.country != other.country {
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

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::osm::region_input::{RegionFile, RegionInput};

    pub(in super::super) fn test_model() -> RailModel {
        serde_json::from_value(serde_json::json!({
            "network_bounds": {},
            "residuals": {},
            "spatial_priors": {},
            "sources": [
                {"id": 111, "rank": 4, "edition": 2024, "owner": "US", "countries": ["US"], "residual": false, "whole_line": true},
                {"id": 112, "rank": 4, "edition": 2024, "owner": "CA", "countries": ["CA"], "residual": false, "whole_line": false}
            ],
            "type_priors": [[80.0, 20.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0]],
            "usage_priors": [[80.0, 20.0], [30.0, 5.0], [0.0, 15.0], [40.0, 10.0]],
            "mainline_freight": {},
            "passenger_periods": [0.7, 0.2, 0.1],
            "freight_periods": [0.4, 0.2, 0.4],
            "urban_periods": [0.6, 0.25, 0.15],
            "regional_freight_periods": []
        }))
        .unwrap()
    }

    pub(in super::super) fn us_ca_regions(border_lat: f64) -> Regions {
        let rect = |y0: f64, y1: f64| {
            vec![vec![vec![
                [-113.0, y0],
                [-111.0, y0],
                [-111.0, y1],
                [-113.0, y1],
                [-113.0, y0],
            ]]]
        };
        Regions::from_input(RegionFile {
            regions: vec![
                RegionInput {
                    country: Some("US".into()),
                    continent: 1,
                    disputed: false,
                    polygons: rect(48.0, border_lat),
                },
                RegionInput {
                    country: Some("CA".into()),
                    continent: 1,
                    disputed: false,
                    polygons: rect(border_lat, 50.0),
                },
            ],
        })
        .unwrap()
    }

    pub(in super::super) fn test_crossing(
        id: &str,
        lat: f64,
        lon: f64,
        soundings: [f64; 3],
        speed_kmh: f64,
        source_id: u16,
        synth_osm: i64,
    ) -> Crossing {
        Crossing {
            id: id.into(),
            lat,
            lon,
            soundings,
            speed_kmh: Some(speed_kmh),
            source_id,
            synth_osm,
            total_periods: None,
            passenger_daily: None,
            passenger_lt1: false,
            measured_periods: false,
        }
    }

    #[test]
    fn horn_matching_respects_source_country_at_border() {
        let model = test_model();
        let crossing = test_crossing(
            "088159P",
            48.998_058,
            -111.958_884,
            [2.0, 1.0, 1.0],
            60.0,
            111,
            -1,
        );
        let rows = [
            BaseRow {
                start_lon: -111.9590,
                start_lat: 48.998_170,
                end_lon: -111.9587,
                end_lat: 48.998_170,
                rail_type: 0,
                country: *b"CA",
            },
            BaseRow {
                start_lon: -111.9590,
                start_lat: 48.997_920,
                end_lon: -111.9587,
                end_lat: 48.997_920,
                rail_type: 0,
                country: *b"US",
            },
        ];
        assert_eq!(
            match_crossing(&model, &crossing, &rows).map(|(r, _, _)| r),
            Some(1)
        );
        assert_eq!(match_crossing(&model, &crossing, &rows[..1]), None);
    }

    #[test]
    fn border_horn_clips_at_country_boundary_and_never_merges_across_countries() {
        let border_lat = 48.998_144;
        let regions = us_ca_regions(border_lat);
        let rows = [BaseRow {
            start_lon: -111.958_932,
            start_lat: 48.997_872,
            end_lon: -111.958_932,
            end_lat: 48.998_048,
            rail_type: 0,
            country: *b"US",
        }];
        let crossing = test_crossing(
            "088159P",
            48.998_048,
            -111.958_932,
            [1.0, 0.3, 0.7],
            64.37,
            111,
            -1,
        );
        let approaches = build_approaches(
            0,
            &crossing,
            0,
            48.998_048,
            -111.958_932,
            &rows,
            Some(&regions),
            true,
        );
        assert_eq!(approaches.len(), 2);
        let south = approaches.iter().find(|a| a.start_lat < a.end_lat).unwrap();
        let north = approaches.iter().find(|a| a.start_lat > a.end_lat).unwrap();
        assert!((south.length_m - 357.61).abs() < 0.2);
        assert!(
            north.length_m > 9.5 && north.length_m < 12.0 && north.start_lat <= border_lat + 1e-7
        );

        let us_approach = Approach {
            crossing: 0,
            synth_osm: -1,
            owned: true,
            approach_idx: 0,
            matched_row: 0,
            country: *b"US",
            start_lon: -111.958_932,
            start_lat: 48.997_0,
            end_lon: -111.958_932,
            end_lat: 48.998_0,
            dir_x: 0.0,
            dir_y: 1.0,
            length_m: 110.54,
            soundings: [2.0, 0.0, 0.0],
        };
        let ca_approach = Approach {
            crossing: 1,
            synth_osm: -2,
            country: *b"CA",
            soundings: [5.0, 0.0, 0.0],
            ..us_approach
        };
        let pieces = merge_overlaps(&[us_approach, ca_approach]);
        assert_eq!(pieces.len(), 2);
        assert_eq!(pieces[0].soundings, [2.0, 0.0, 0.0]);
        assert_eq!(pieces[1].soundings, [5.0, 0.0, 0.0]);
    }

    #[test]
    fn cross_square_neighbor_approaches_deduplicate_without_double_emission() {
        let make = |synth_osm, owned, lat0, lat1| Approach {
            crossing: 0,
            synth_osm,
            owned,
            approach_idx: 0,
            matched_row: 0,
            country: *b"US",
            start_lon: -88.35,
            start_lat: lat0,
            end_lon: -88.35,
            end_lat: lat1,
            dir_x: 0.0,
            dir_y: 1.0,
            length_m: 400.0,
            soundings: [1.0, 0.0, 0.0],
        };
        let deg_80 = 80.0 / crate::osm::geometry::M_PER_DEG_LAT;
        let deg_400 = 400.0 / crate::osm::geometry::M_PER_DEG_LAT;
        let pa = merge_overlaps(&[
            make(-160225, false, 36.0, 36.0 + deg_400),
            make(-160224, true, 36.0 + deg_80, 36.0 + deg_80 + deg_400),
        ]);
        let pb = merge_overlaps(&[
            make(-160225, true, 36.0, 36.0 + deg_400),
            make(-160224, false, 36.0 + deg_80, 36.0 + deg_80 + deg_400),
        ]);
        assert_eq!((pa.len(), pb.len()), (1, 2));
        let len_a: f64 = pa.iter().map(|p| (p.frac1 - p.frac0) * 400.0).sum();
        let len_b: f64 = pb.iter().map(|p| (p.frac1 - p.frac0) * 400.0).sum();
        assert!((len_a - 80.0).abs() < 1e-3 && (len_b - 400.0).abs() < 1e-3);
    }
}

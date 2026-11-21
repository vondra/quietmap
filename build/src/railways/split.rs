//! Split a parent acoustic piece at every distinct evidence boundary.

use super::merge::{RowTraffic, overlapping, row_evidence};
use super::square_intervals::Interval;
use super::topology::Piece;
use crate::osm::geometry::lonlat_to_grid;

#[derive(Clone, Copy, Debug)]
pub struct ChildGeom {
    pub start_gx: i32,
    pub start_gy: i32,
    pub end_gx: i32,
    pub end_gy: i32,
    pub length_m: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct ChildRow {
    pub geom: ChildGeom,
    pub traffic: RowTraffic,
    pub foreign: RowTraffic,
    pub resolved_section: Option<RowTraffic>,
}

const CUT_EPS_M: f64 = 1e-4;

pub struct ParentRow {
    pub osm_id: i64,
    pub segment_idx: i16,
    pub original: ChildGeom,
    pub rail_type: u8,
    pub country: [u8; 2],
}

pub fn split_parent(
    model: &super::model::RailModel,
    parent: ParentRow,
    piece: Option<&Piece>,
    intervals: &[Interval],
) -> Result<Vec<ChildRow>, String> {
    let ParentRow {
        osm_id,
        segment_idx,
        original,
        rail_type,
        country,
    } = parent;
    let child = |geom: ChildGeom, from_m: f64, to_m: f64| {
        let (traffic, foreign) = row_evidence(model, intervals, from_m, to_m, rail_type, country);
        let half = 0.5 * (to_m - from_m);
        let resolved_section = overlapping(intervals, from_m, to_m)
            .filter_map(|interval| {
                let rt = interval.resolved_section?;
                let overlap = interval.to_m.min(to_m) - interval.from_m.max(from_m);
                (overlap >= half).then_some((overlap, rt))
            })
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, rt)| rt);
        ChildRow {
            geom,
            traffic,
            foreign,
            resolved_section,
        }
    };
    let Some(piece) = piece else {
        if !intervals.is_empty() {
            return Err(format!(
                "rail interval for {osm_id}:{segment_idx} has no source topology piece"
            ));
        }
        return Ok(vec![child(original, 0.0, original.length_m as f64)]);
    };
    let (piece_from, piece_to) = (piece.from_m, piece.to_m);
    let mut cuts = vec![piece_from, piece_to];
    for interval in intervals {
        cuts.push(interval.from_m.clamp(piece_from, piece_to));
        cuts.push(interval.to_m.clamp(piece_from, piece_to));
    }
    cuts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut unique = Vec::new();
    for cut in cuts {
        if unique
            .last()
            .is_none_or(|previous: &f64| cut - *previous > CUT_EPS_M)
        {
            unique.push(cut);
        }
    }
    let mut children = Vec::new();
    for window in unique.windows(2) {
        let from_m = window[0];
        let to_m = window[1];
        if to_m - from_m <= CUT_EPS_M {
            continue;
        }
        let geom = if unique.len() == 2 {
            original
        } else {
            let start = piece.coordinates_at(from_m);
            let end = piece.coordinates_at(to_m);
            let (start_gx, start_gy) = lonlat_to_grid(start[1], start[0]);
            let (end_gx, end_gy) = lonlat_to_grid(end[1], end[0]);
            ChildGeom {
                start_gx,
                start_gy,
                end_gx,
                end_gy,
                length_m: (to_m - from_m) as f32,
            }
        };
        children.push(child(geom, from_m, to_m));
    }
    if children.is_empty() {
        children.push(child(original, piece_from, piece_to));
    }
    Ok(children)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::railways::{
        merge::{CategoryFlow, STATUS_ESTIMATED, STATUS_KNOWN, STATUS_UNKNOWN},
        model::RailModel,
    };

    fn flow(daily: f64, source_id: u16, status: u8) -> CategoryFlow {
        CategoryFlow {
            periods: [daily * 0.7, daily * 0.2, daily * 0.1],
            missing_calendar_estimate: [0.0; 3],
            status,
            source_id,
            matching: 2,
            stop: 0,
        }
    }

    fn resolved_iv(from_m: f64, to_m: f64, pax: f64, pax_src: u16, frt: f64) -> Interval {
        Interval {
            missing_calendar_estimate: [0.0; 2],
            osm_id: 762074149,
            segment_idx: 0,
            from_m,
            to_m,
            country: *b"US",
            source_id: 111,
            passenger: 0.0,
            freight: 0.0,
            passenger_status: STATUS_UNKNOWN,
            freight_status: STATUS_UNKNOWN,
            matching: 2,
            stop: 0,
            resolved_section: Some(RowTraffic {
                passenger: flow(pax, pax_src, STATUS_ESTIMATED),
                freight: flow(frt, 111, STATUS_ESTIMATED),
            }),
        }
    }

    #[test]
    fn resolved_section_selection_ignores_sub_epsilon_cut_overhang() {
        let model = RailModel::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../years/tables/2024/rail-model.json")
                .as_path(),
        )
        .unwrap();
        let piece = Piece::new(
            0.0,
            0.0,
            169.29,
            vec![[-71.2610, 42.0941], [-71.2617, 42.0956]],
        );
        let parent = ParentRow {
            osm_id: 762074149,
            segment_idx: 0,
            original: ChildGeom {
                start_gx: 0,
                start_gy: 0,
                end_gx: 100,
                end_gy: 100,
                length_m: 169.29,
            },
            rail_type: 0,
            country: *b"US",
        };
        let timetable = Interval {
            missing_calendar_estimate: [0.0; 2],
            osm_id: 762074149,
            segment_idx: 0,
            from_m: 114.2,
            to_m: 169.29,
            country: *b"US",
            source_id: 2079,
            passenger: 13.37,
            freight: 0.0,
            passenger_status: STATUS_KNOWN,
            freight_status: STATUS_UNKNOWN,
            matching: 2,
            stop: 0,
            resolved_section: None,
        };
        // First resolved interval overhangs 114.2 by 1e-5 m (< CUT_EPS_M), so split deduplicates
        // the cut to 114.2; child [114.2, 169.29] must take the second resolved interval, not the first.
        let both = split_parent(
            &model,
            ParentRow { ..parent },
            Some(&piece),
            &[
                resolved_iv(0.0, 114.20001, 0.0, 111, 5.0),
                resolved_iv(114.20001, 169.29, 13.37, 2079, 0.0),
                timetable.clone(),
            ],
        )
        .unwrap();
        assert_eq!(both.len(), 2);
        let first = both[0].resolved_section.unwrap();
        let second = both[1].resolved_section.unwrap();
        assert_eq!(first.passenger.source_id, 111);
        assert_eq!(second.passenger.source_id, 2079);
        assert!((second.passenger.periods.iter().sum::<f64>() - 13.37).abs() < 1e-6);

        // When the second child has no resolved section, a 1e-5 m overhang must not leak onto it.
        let one = split_parent(
            &model,
            parent,
            Some(&piece),
            &[resolved_iv(0.0, 114.20001, 0.0, 111, 5.0), timetable],
        )
        .unwrap();
        assert_eq!(one.len(), 2);
        assert!(one[0].resolved_section.is_some());
        assert!(one[1].resolved_section.is_none());
    }
}

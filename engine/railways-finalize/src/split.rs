//! Split a parent acoustic piece at every distinct evidence boundary.

use crate::merge::{row_traffic, RowTraffic};
use crate::sidecar::Interval;
use crate::topology::Piece;
use grid::lonlat_to_grid;
use noise_compute::square_country_city::SquareCountryCity;

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
}

const CUT_EPS_M: f64 = 1e-4;

pub fn split_parent(
    osm_id: i64,
    segment_idx: i16,
    original: ChildGeom,
    piece: Option<&Piece>,
    intervals: &[Interval],
    rail_type: u8,
    usage: u8,
    service: u8,
    square_country_city: SquareCountryCity,
) -> Result<Vec<ChildRow>, String> {
    let allow_defaults = intervals.is_empty();
    let Some(piece) = piece else {
        if !allow_defaults {
            return Err(format!(
                "sidecar interval for {osm_id}:{segment_idx} has no source topology piece"
            ));
        }
        return Ok(vec![ChildRow {
            geom: original,
            traffic: row_traffic(
                intervals,
                0.0,
                original.length_m as f64,
                rail_type,
                usage,
                service,
                square_country_city,
                true,
            ),
        }]);
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
            let start = piece.way.coordinates_at(from_m);
            let end = piece.way.coordinates_at(to_m);
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
        children.push(ChildRow {
            geom,
            traffic: row_traffic(
                intervals,
                from_m,
                to_m,
                rail_type,
                usage,
                service,
                square_country_city,
                allow_defaults,
            ),
        });
    }
    if children.is_empty() {
        children.push(ChildRow {
            geom: original,
            traffic: row_traffic(
                intervals,
                piece_from,
                piece_to,
                rail_type,
                usage,
                service,
                square_country_city,
                allow_defaults,
            ),
        });
    }
    Ok(children)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merge::STATUS_UNKNOWN;
    use crate::sidecar::Interval;
    use crate::topology::Piece;

    fn piece() -> Piece {
        Piece {
            from_m: 0.0,
            to_m: 100.0,
            way: std::sync::Arc::new(
                crate::topology::WayGeometry::new(vec![[50.0, 14.0], [50.0, 14.001]]).unwrap(),
            ),
        }
    }

    #[test]
    fn interior_cut_emits_three_children_and_keeps_parent_index() {
        let intervals = [Interval {
            osm_id: 7,
            segment_idx: 0,
            from_m: 20.0,
            to_m: 60.0,
            source_id: 100,
            passenger: 4.0,
            freight: 0.0,
            passenger_status: 2,
            freight_status: STATUS_UNKNOWN,
            matching: 2,
        }];
        let original = ChildGeom {
            start_gx: 1,
            start_gy: 2,
            end_gx: 3,
            end_gy: 4,
            length_m: 100.0,
        };
        let cz = SquareCountryCity {
            continent: noise_compute::square_country_city::Continent::Europe,
            country_iso: *b"CZ",
            city_id: 0,
        };
        let children =
            split_parent(7, 0, original, Some(&piece()), &intervals, 0, 0, 0, cz).unwrap();
        assert_eq!(children.len(), 3);
        assert!((children[0].geom.length_m - 20.0).abs() < 1e-6);
        assert!((children[1].geom.length_m - 40.0).abs() < 1e-6);
        let middle = children[1].traffic.passenger.periods.iter().sum::<f64>();
        let left = children[0].traffic.passenger.periods.iter().sum::<f64>();
        assert!(middle > 3.9);
        assert!(left < 0.01 || children[0].traffic.passenger.source_id == 0);
    }
}

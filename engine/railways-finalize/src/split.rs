//! Split a parent acoustic piece at every distinct evidence boundary.

use crate::merge::{row_evidence, RowTraffic};
use crate::square_intervals::Interval;
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
    pub foreign: RowTraffic,
}

const CUT_EPS_M: f64 = 1e-4;

pub fn split_parent(
    osm_id: i64,
    segment_idx: i16,
    original: ChildGeom,
    piece: Option<&Piece>,
    intervals: &[Interval],
    rail_type: u8,
    square_country_city: SquareCountryCity,
) -> Result<Vec<ChildRow>, String> {
    let child = |geom: ChildGeom, from_m: f64, to_m: f64| {
        let (traffic, foreign) = row_evidence(
            intervals,
            from_m,
            to_m,
            rail_type,
            square_country_city,
        );
        ChildRow { geom, traffic, foreign }
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

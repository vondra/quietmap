//! The common HPMS facility scope, road-class support and duplicated two-way-section interpretation.
use crate::roads::observation::{CountBasis, Observation};
use std::collections::HashMap;

pub(super) struct Section {
    pub row: Observation,
    pub rank: u8,
    pub facility: Option<u8>,
    pub centroid: [f64; 2],
}

pub(super) fn class_mask(rank: u8, facility: Option<u8>) -> u16 {
    (0u8..13)
        .filter(|class| {
            let ramp = *class >= 10;
            let mapped = if *class <= 4 {
                *class
            } else if ramp {
                *class - 10
            } else {
                6
            };
            ramp == (facility == Some(4)) && mapped.abs_diff(rank) <= 1
        })
        .map(|class| 1 << class)
        .sum()
}

pub(super) fn relabel_twins(sections: &mut [Section], radius_m: f64, total_ratio_tolerance: f64) {
    let mut grid = HashMap::<(i32, i32), Vec<usize>>::new();
    let cell = |p: [f64; 2]| ((p[0] * 100.0).floor() as i32, (p[1] * 100.0).floor() as i32);
    for (i, s) in sections
        .iter()
        .enumerate()
        .filter(|(_, s)| s.facility == Some(6))
    {
        grid.entry(cell(s.centroid)).or_default().push(i);
    }
    let mut relabeled = 0_u64;
    for i in 0..sections.len() {
        if sections[i].facility != Some(1) {
            continue;
        }
        let s = &sections[i];
        let (x, y) = cell(s.centroid);
        let dy = (radius_m / 110_540.0 * 100.0).ceil() as i32;
        let dx = (f64::from(dy) / s.centroid[1].to_radians().cos()).ceil() as i32;
        let twin = (y - dy..=y + dy).any(|y| {
            (x - dx..=x + dx).any(|x| {
                grid.get(&(x, y)).into_iter().flatten().any(|j| {
                    let t = &sections[*j];
                    t.rank == s.rank
                        && (s.row.total.unwrap() / t.row.total.unwrap() - 1.0).abs()
                            <= total_ratio_tolerance
                        && crate::roads::great_circle_distance(s.centroid, t.centroid) <= radius_m
                })
            })
        });
        if twin {
            sections[i].row.count_basis = CountBasis::BothDirections;
            relabeled += 1;
        }
    }
    eprintln!(
        "road-counts: HPMS duplicated section totals: {relabeled} inventory directions relabeled"
    );
}

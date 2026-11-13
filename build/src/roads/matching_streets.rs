//! Select the street represented by a municipal counter while letting adjacent excluded roads compete.

use super::{
    matching::Candidate,
    matching_distance::{distance, road_points},
    matching_targets::name,
    observation::Observation,
};
use anyhow::Result;
use rayon::prelude::*;
use rstar::{AABB, RTree};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
};

#[derive(Clone)]
struct Street {
    name: String,
    class: u8,
    distance: f64,
}

fn class_rank(class: u8) -> u8 {
    if class >= 10 {
        (class - 10) * 2 + 1
    } else {
        class * 2
    }
}

pub fn assign(
    rows: &[Observation],
    tree: &RTree<Candidate>,
    mapped: &Path,
) -> Result<HashMap<usize, HashSet<String>>> {
    if !rows.iter().any(|r| r.target.street_assignment.is_some()) {
        return Ok(HashMap::new());
    }
    let squares = super::topology::squares(mapped)?
        .into_values()
        .collect::<Vec<_>>();
    let found = squares
        .par_iter()
        .map(|path| square_streets(rows, tree, path))
        .collect::<Result<Vec<_>>>()?;
    // In square order, as the first of equally distant streets keeps its class.
    let mut nearby = HashMap::<usize, Vec<Street>>::new();
    for (index, street) in found.into_iter().flatten() {
        nearby.entry(index).or_default().push(street);
    }
    Ok(nearby
        .into_iter()
        .map(|(index, candidates)| (index, selected(&rows[index], candidates)))
        .collect())
}

/// Every street a square's roads offer the counters around them, in road order.
fn square_streets(
    rows: &[Observation],
    tree: &RTree<Candidate>,
    path: &Path,
) -> Result<Vec<(usize, Street)>> {
    let mut found = Vec::new();
    for batch in super::input::load(path)? {
        for road in super::input::roads(&batch)? {
            let points = road_points(&road);
            let envelope = AABB::from_corners(
                [
                    points[0][0].min(points[2][0]),
                    points[0][1].min(points[2][1]),
                ],
                [
                    points[0][0].max(points[2][0]),
                    points[0][1].max(points[2][1]),
                ],
            );
            for candidate in tree.locate_in_envelope_intersecting(&envelope) {
                let row = &rows[candidate.index];
                let Some(assignment) = &row.target.street_assignment else {
                    continue;
                };
                if row.country != road.country.0
                    || assignment.candidate_classes & (1 << road.class) == 0
                {
                    continue;
                }
                let d = distance(row, &road, &points);
                if d <= row.search_radius_m {
                    found.push((
                        candidate.index,
                        Street {
                            name: name(&road.name),
                            class: road.class,
                            distance: d,
                        },
                    ));
                }
            }
        }
    }
    Ok(found)
}

fn selected(row: &Observation, candidates: Vec<Street>) -> HashSet<String> {
    let assignment = row.target.street_assignment.as_ref().unwrap();
    let point = row.geometry.iter().map(Vec::len).sum::<usize>() == 1;
    let nearest = candidates
        .iter()
        .map(|c| c.distance)
        .fold(f64::INFINITY, f64::min);
    let mut groups = BTreeMap::<String, Street>::new();
    for candidate in candidates {
        if point && candidate.distance > nearest + assignment.point_band_m {
            continue;
        }
        groups
            .entry(candidate.name.clone())
            .and_modify(|previous| {
                if point {
                    if class_rank(candidate.class) < class_rank(previous.class) {
                        previous.class = candidate.class;
                    }
                    previous.distance = previous.distance.min(candidate.distance);
                } else if candidate.distance < previous.distance {
                    *previous = candidate.clone();
                }
            })
            .or_insert(candidate);
    }
    if point {
        let hint = name(&assignment.street_hint);
        let hint = regex::Regex::new(" (i{1,3}|iv|v)$")
            .unwrap()
            .replace(&hint, "");
        groups
            .into_values()
            .min_by(|a, b| {
                class_rank(a.class)
                    .cmp(&class_rank(b.class))
                    .then_with(|| (b.name == hint).cmp(&(a.name == hint)))
                    .then_with(|| a.distance.total_cmp(&b.distance))
                    .then_with(|| a.name.cmp(&b.name))
            })
            .map(|c| HashSet::from([c.name]))
            .unwrap_or_default()
    } else {
        let primary = groups
            .values()
            .min_by(|a, b| a.distance.total_cmp(&b.distance));
        if primary.is_none_or(|r| row.road_class_mask & (1 << r.class) == 0) {
            return HashSet::new();
        }
        groups
            .into_values()
            .filter(|r| {
                r.distance <= assignment.line_near_m && row.road_class_mask & (1 << r.class) != 0
            })
            .map(|r| r.name)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_excluded_motorway_still_owns_its_counter() {
        let mut row = super::super::tests::observation();
        row.geometry = vec![vec![[0.0, 0.0]]];
        row.road_class_mask = 1 << 5;
        row.target.street_assignment = Some(super::super::observation_target::StreetAssignment {
            candidate_classes: 33,
            point_band_m: 15.0,
            line_near_m: 28.0,
            street_hint: "".into(),
        });
        let selected = selected(
            &row,
            vec![
                Street {
                    name: "side street".into(),
                    class: 5,
                    distance: 1.0,
                },
                Street {
                    name: "motorway".into(),
                    class: 0,
                    distance: 5.0,
                },
            ],
        );
        assert!(selected.contains("motorway"));
        assert!(!selected.contains("side street"));
    }
}

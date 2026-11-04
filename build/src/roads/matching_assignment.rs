//! Locate an unnamed point counter on one original way only when other road classes leave it unambiguous.

use super::{
    matching::Candidate,
    matching_distance::{distance, road_points},
    observation::Observation,
};
use anyhow::Result;
use rstar::{AABB, RTree};
use std::{collections::HashMap, path::Path};

struct Nearest {
    way: i64,
    class: u8,
    distance: f64,
    nearby_classes: u16,
}

pub fn assign(
    rows: &[Observation],
    tree: &RTree<Candidate>,
    targets: &super::matching_targets::Targets,
    mapped: &Path,
) -> Result<HashMap<usize, i64>> {
    if !rows.iter().any(|r| r.target.way_assignment.is_some()) {
        return Ok(HashMap::new());
    }
    let mut nearest = HashMap::<usize, Nearest>::new();
    let published = rows
        .iter()
        .enumerate()
        .filter_map(|(index, row)| {
            row.target
                .way_assignment
                .as_ref()
                .and(row.target.source_way)
                .map(|way| (way, index))
        })
        .fold(
            HashMap::<i64, Vec<usize>>::new(),
            |mut map, (way, index)| {
                map.entry(way).or_default().push(index);
                map
            },
        );
    let mut published_matches = HashMap::new();
    for (square, path) in super::topology::squares(mapped)? {
        for batch in super::input::load(&path)? {
            for road in super::input::roads(&batch)? {
                if let Some(indices) = published.get(&road.way_id) {
                    for &index in indices {
                        let row = &rows[index];
                        if (row.country == 0 || row.country == road.country.0)
                            && row.road_class_mask & (1 << road.class) != 0
                        {
                            published_matches.insert(index, road.way_id);
                        }
                    }
                }
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
                    let Some(assignment) = &row.target.way_assignment else {
                        continue;
                    };
                    if row.country != 0 && road.country.0 != row.country {
                        continue;
                    }
                    if !assignment.require_class_clearance
                        && (row.road_class_mask & (1 << road.class) == 0
                            || targets
                                .accepts(candidate.index, row, &targets.query(&road), points[1])
                                .is_none())
                    {
                        continue;
                    }
                    let distance = distance(row, &road, &points);
                    if distance > assignment.clearance_m {
                        continue;
                    }
                    let best = nearest.entry(candidate.index).or_insert(Nearest {
                        way: road.way_id,
                        class: road.class,
                        distance,
                        nearby_classes: 0,
                    });
                    best.nearby_classes |= 1 << road.class;
                    if distance < best.distance
                        || distance == best.distance && road.way_id < best.way
                    {
                        best.distance = distance;
                        best.way = road.way_id;
                        best.class = road.class;
                    }
                }
            }
        }
        eprintln!("road-counter-ways: {}/{}", square.x, square.y);
    }
    let mut assignments: HashMap<usize, i64> = nearest
        .into_iter()
        .filter_map(|(index, best)| {
            let row = &rows[index];
            (best.distance
                <= row
                    .target
                    .way_assignment
                    .as_ref()
                    .unwrap()
                    .maximum_distance_m
                && (!row
                    .target
                    .way_assignment
                    .as_ref()
                    .unwrap()
                    .require_class_clearance
                    || best.nearby_classes == 1 << best.class)
                && row.road_class_mask & (1 << best.class) != 0)
                .then_some((index, best.way))
        })
        .collect();
    assignments.extend(published_matches);
    Ok(assignments)
}

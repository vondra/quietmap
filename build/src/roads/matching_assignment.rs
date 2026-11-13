//! Locate an unnamed point counter on one original way only when other road classes leave it unambiguous.

use super::{
    matching::Candidate,
    matching_distance::{road_points, segment_distance},
    observation::Observation,
};
use crate::dev4::Square;
use anyhow::Result;
use rayon::prelude::*;
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
    let squares = super::topology::squares(mapped)?
        .into_iter()
        .collect::<Vec<_>>();
    let found = squares
        .par_iter()
        .map(|(square, path)| square_ways(rows, tree, targets, &published, *square, path))
        .collect::<Result<Vec<_>>>()?;
    // In square order, as the first of equally near pieces of one way keeps its class.
    let mut published_matches = HashMap::new();
    for square in found {
        published_matches.extend(square.published);
        for (index, way, class, distance) in square.near {
            let best = nearest.entry(index).or_insert(Nearest {
                way,
                class,
                distance,
                nearby_classes: 0,
            });
            best.nearby_classes |= 1 << class;
            if distance < best.distance || distance == best.distance && way < best.way {
                best.distance = distance;
                best.way = way;
                best.class = class;
            }
        }
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

/// A square's evidence for counter ways, in road order.
struct SquareWays {
    /// (counter, way) for each piece of a counter's published way.
    published: Vec<(usize, i64)>,
    /// (counter, way, class, distance) for each piece near an unnamed point counter.
    near: Vec<(usize, i64, u8, f64)>,
}

fn square_ways(
    rows: &[Observation],
    tree: &RTree<Candidate>,
    targets: &super::matching_targets::Targets,
    published: &HashMap<i64, Vec<usize>>,
    square: Square,
    path: &Path,
) -> Result<SquareWays> {
    let mut found = SquareWays {
        published: Vec::new(),
        near: Vec::new(),
    };
    for batch in super::input::load(path)? {
        for road in super::input::roads(&batch)? {
            if let Some(indices) = published.get(&road.way_id) {
                for &index in indices {
                    let row = &rows[index];
                    if (row.country == 0 || row.country == road.country.0)
                        && row.road_class_mask & (1 << road.class) != 0
                    {
                        found.published.push((index, road.way_id));
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
                let distance = segment_distance(row.geometry[0][0], points[0], points[2]);
                if distance > assignment.clearance_m {
                    continue;
                }
                found
                    .near
                    .push((candidate.index, road.way_id, road.class, distance));
            }
        }
    }
    eprintln!("road-counter-ways: {}/{}", square.x, square.y);
    Ok(found)
}

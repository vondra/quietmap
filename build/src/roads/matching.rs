//! One spatial/ref/direction matcher with source precedence for every normalized road observation.

use super::{
    input::Road,
    matching_distance::{distance, longitude_delta, road_points},
    model::Model,
    observation::Observation,
};
use anyhow::Result;
use rstar::{AABB, RTree, RTreeObject};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};
use unicode_normalization::UnicodeNormalization;

pub(super) struct Candidate {
    pub index: usize,
    envelope: AABB<[f64; 2]>,
}
impl RTreeObject for Candidate {
    type Envelope = AABB<[f64; 2]>;
    fn envelope(&self) -> Self::Envelope {
        self.envelope
    }
}

pub struct Matcher {
    observations: Vec<Observation>,
    tree: RTree<Candidate>,
    targets: super::matching_targets::Targets,
    by_way: HashMap<i64, Vec<usize>>,
    assigned: HashMap<usize, i64>,
    assigned_streets: HashMap<usize, HashSet<String>>,
    measured_countries: HashSet<u16>,
}

pub fn canonical_reference(value: &str) -> String {
    value
        .nfkc()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_uppercase)
        .collect()
}

impl Matcher {
    pub fn has_measurements(&self) -> bool {
        !self.measured_countries.is_empty()
    }
    pub fn has_measurements_in(&self, country: u16) -> bool {
        self.measured_countries.contains(&0) || self.measured_countries.contains(&country)
    }
    pub fn new(observations: Vec<Observation>) -> Result<Self> {
        let observations = observations
            .into_iter()
            .filter(|row| !row.sample.fit_only)
            .collect::<Vec<_>>();
        let targets = super::matching_targets::Targets::new(&observations)?;
        let mut candidates = Vec::new();
        for (index, row) in observations.iter().enumerate() {
            for part in &row.geometry {
                let origin = part[0][0];
                let west = part
                    .iter()
                    .map(|p| origin + longitude_delta(p[0] - origin))
                    .fold(f64::INFINITY, f64::min);
                let east = part
                    .iter()
                    .map(|p| origin + longitude_delta(p[0] - origin))
                    .fold(f64::NEG_INFINITY, f64::max);
                let south = part.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min)
                    - row.search_radius_m / 110_540.0;
                let north = part.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max)
                    + row.search_radius_m / 110_540.0;
                let latitude = south.abs().max(north.abs()).min(90.0);
                let reach =
                    (row.search_radius_m / (110_540.0 * latitude.to_radians().cos())).min(360.0);
                for shift in [-360.0, 0.0, 360.0] {
                    candidates.push(Candidate {
                        index,
                        envelope: AABB::from_corners(
                            [west - reach + shift, south],
                            [east + reach + shift, north],
                        ),
                    });
                }
            }
        }
        let mut by_way = HashMap::<i64, Vec<usize>>::new();
        for (i, row) in observations.iter().enumerate() {
            if let Some(id) = row.target.source_way {
                by_way.entry(id).or_default().push(i);
            }
        }
        let measured_countries = observations
            .iter()
            .filter(|r| r.measured)
            .map(|r| r.country)
            .collect();
        Ok(Self {
            observations,
            tree: RTree::bulk_load(candidates),
            targets,
            by_way,
            assigned: HashMap::new(),
            assigned_streets: HashMap::new(),
            measured_countries,
        })
    }

    pub fn assign_counter_ways(&mut self, mapped: &Path) -> Result<()> {
        self.assigned_streets =
            super::matching_streets::assign(&self.observations, &self.tree, mapped)?;
        self.assigned = super::matching_assignment::assign(
            &self.observations,
            &self.tree,
            &self.targets,
            mapped,
        )?;
        for (&index, &way) in &self.assigned {
            self.by_way.entry(way).or_default().push(index);
        }
        Ok(())
    }

    pub fn apply(&self, road: &mut Road, model: &Model) -> Result<()> {
        let points = road_points(road);
        let midpoint = points[1];
        let mut candidates = self
            .tree
            .locate_in_envelope_intersecting(&AABB::from_corners(
                [
                    points[0][0].min(points[2][0]),
                    points[0][1].min(points[2][1]),
                ],
                [
                    points[0][0].max(points[2][0]),
                    points[0][1].max(points[2][1]),
                ],
            ))
            .map(|c| c.index)
            .collect::<Vec<_>>();
        let query = self.targets.query(road);
        self.targets.candidates(road, &query, &mut candidates);
        candidates.extend(self.by_way.get(&road.way_id).into_iter().flatten().copied());
        candidates.sort_unstable();
        candidates.dedup();
        let mut best: Option<(usize, super::matching_targets::IdentityScore, f64)> = None;
        let mut conflicting = false;
        let mut best_support = f64::INFINITY;
        for index in candidates {
            let row = &self.observations[index];
            if row
                .target
                .required_oneway
                .is_some_and(|required| required != (road.direction != 0))
            {
                continue;
            }
            if row.target.excluded_segments.iter().any(|segment| {
                super::segment_distance(midpoint, segment[0], segment[1])
                    < row.target.exclusion_radius_m
            }) {
                continue;
            }
            if row.target.street_assignment.is_some()
                && !self
                    .assigned_streets
                    .get(&index)
                    .is_some_and(|names| names.contains(&super::matching_targets::name(&road.name)))
            {
                continue;
            }
            let published_way = row.target.source_way == Some(road.way_id);
            let assigned_way = self.assigned.get(&index) == Some(&road.way_id);
            let same_way = published_way
                || assigned_way
                    && row
                        .target
                        .way_assignment
                        .as_ref()
                        .is_some_and(|a| a.extend_along_way);
            if row.target.way_assignment.is_some() && !assigned_way {
                continue;
            }
            if row.country != 0 && row.country != road.country.0
                || row.road_class_mask & (1 << road.class) == 0
            {
                continue;
            }
            let support = if same_way {
                Some(((u16::MAX, std::cmp::Reverse(0)), f64::INFINITY))
            } else {
                self.targets.accepts(index, row, &query, midpoint)
            };
            let Some((identity_priority, radius)) = support else {
                continue;
            };
            let distance = if same_way && row.target.way_assignment.is_some() {
                super::matching_distance::segment_distance(row.geometry[0][0], points[0], points[2])
            } else if published_way {
                super::matching_distance::distance_without_heading(row, road, &points)
            } else {
                distance(row, road, &points)
            };
            if !distance.is_finite() || (!row.geometry.is_empty() && distance >= radius) {
                continue;
            }
            let support_distance = if best.is_some_and(|(previous, priority, previous_distance)| {
                row.source_rank == self.observations[previous].source_rank
                    && identity_priority == priority
                    && distance == previous_distance
            }) {
                super::matching_distance::endpoint_support(row, &points)
            } else {
                f64::INFINITY
            };
            if let Some((previous, priority, previous_distance)) = best {
                if row.source_rank == self.observations[previous].source_rank
                    && identity_priority == priority
                    && distance == previous_distance
                {
                    best_support = super::matching_distance::endpoint_support(
                        &self.observations[previous],
                        &points,
                    );
                }
                let other = &self.observations[previous];
                if row.source_rank == other.source_rank
                    && identity_priority == priority
                    && distance == previous_distance
                    && support_distance == best_support
                    && (row.vehicle_classes != other.vehicle_classes
                        || row.total != other.total
                        || row.vehicle_groups != other.vehicle_groups
                        || row.count_basis != other.count_basis
                        || row.time_basis != other.time_basis
                        || row.daily_factor != other.daily_factor)
                {
                    conflicting = true;
                }
            }
            if best.is_none_or(|(previous, previous_priority, previous_distance)| {
                row.source_rank > self.observations[previous].source_rank
                    || row.source_rank == self.observations[previous].source_rank
                        && (identity_priority > previous_priority
                            || identity_priority == previous_priority
                                && (distance < previous_distance
                                    || distance == previous_distance
                                        && support_distance < best_support))
            }) {
                best = Some((index, identity_priority, distance));
                conflicting = false;
            }
        }
        if !conflicting && let Some((index, _, _)) = best {
            let row = &self.observations[index];
            let shares = model
                .observation_shares
                .get(&row.source_id)
                .unwrap_or(&model.world);
            let shares = shares[usize::from(road.class)];
            let (counts, estimated) =
                if row.time_basis == super::observation::TimeBasis::WeekdayDaily {
                    row.daily_classes_using_factor(
                        shares,
                        model.weekday_to_annual_factor.unwrap_or(row.daily_factor),
                    )?
                } else {
                    row.daily_classes(shares)?
                };
            road.counts = counts;
            road.estimated = estimated;
            road.observation_estimated = estimated;
            road.source_id = row.source_id;
            road.observation_source_id = row.source_id;
            road.observation = row.identity.clone();
            road.basis = row.count_basis as u8;
            road.count_origin_measured = row.measured;
            road.observation_time_basis = row.time_basis as u8;
        }
        Ok(())
    }
}

//! Carry an agreed observation only through complete, unbranched chains of original OSM endpoints.

mod chains;

use super::{
    continuity_scope::Scope,
    input::Road,
    matching::Matcher,
    model::Model,
    topology::{self, Endpoint},
};
use crate::dev4::Square;
use anyhow::{Result, ensure};
use rayon::prelude::*;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::Path,
};

/// The source id of a row whose count continues a measured one along its road (dev4's 12).
pub(crate) const CONTINUITY_SOURCE: u16 = 12;

#[derive(Clone, PartialEq)]
struct Flow {
    counts: [f64; 4],
    basis: u8,
    observation_estimated: u8,
    observation_time_basis: u8,
    source: u16,
    identity: String,
}

#[derive(Default, Clone)]
struct Agreement {
    flow: Option<Flow>,
    conflict: bool,
    unresolved: bool,
    potential_observations: bool,
}

impl Agreement {
    fn add(&mut self, other: &Self) {
        self.conflict |=
            other.conflict || matches!((&self.flow,&other.flow),(Some(a),Some(b)) if a!=b);
        self.unresolved |= other.unresolved;
        self.potential_observations |= other.potential_observations;
        if self.flow.is_none() {
            self.flow = other.flow.clone();
        }
    }
    fn observed(road: &Road) -> Self {
        Self {
            flow: (road.count_origin_measured
                && road.source_id != CONTINUITY_SOURCE
                && !road.observation.is_empty())
            .then(|| Flow {
                counts: road.counts,
                basis: road.basis,
                observation_estimated: road.observation_estimated,
                observation_time_basis: road.observation_time_basis,
                source: road.observation_source_id,
                identity: road.observation.clone(),
            }),
            conflict: false,
            unresolved: false,
            potential_observations: false,
        }
    }
}

#[derive(Clone)]
struct ChainEnd {
    way: i64,
    class: u8,
    access: u8,
    roundabout: bool,
    reference: String,
    name: String,
    direction: u8,
    arrival: bool,
}

impl ChainEnd {
    fn new(road: &Road, side: usize) -> Self {
        Self {
            way: road.way_id,
            class: road.class,
            access: road.access,
            roundabout: road.roundabout,
            reference: road
                .reference
                .split_whitespace()
                .collect::<String>()
                .to_uppercase(),
            name: road
                .name
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase(),
            direction: road.direction,
            arrival: side == usize::from(road.direction != 2),
        }
    }
    fn continues(&self, other: &Self) -> bool {
        self.class == other.class
            && self.access == other.access
            && !self.roundabout
            && !other.roundabout
            && (self.way == other.way
                || if !self.reference.is_empty() || !other.reference.is_empty() {
                    self.reference == other.reference
                } else {
                    !self.name.is_empty() && self.name == other.name
                })
            && if self.direction == 0 || other.direction == 0 {
                self.direction == other.direction
            } else {
                self.arrival != other.arrival
            }
    }
}

#[derive(Default)]
struct SharedNode {
    degree: usize,
    ends: Vec<(usize, ChainEnd)>,
}

struct Part {
    agreement: Agreement,
    targets: Vec<(i64, i16)>,
    square: Square,
}

struct Fill {
    targets: Vec<(i64, i16)>,
    flow: Flow,
}

#[derive(Default)]
pub struct Plan {
    fills: BTreeMap<Square, Vec<Fill>>,
}

impl Plan {
    pub fn apply(&self, square: Square, roads: &mut [Road]) {
        let by_identity = roads
            .iter()
            .enumerate()
            .map(|(i, r)| ((r.way_id, r.segment_idx), i))
            .collect::<HashMap<_, _>>();
        for fill in self.fills.get(&square).into_iter().flatten() {
            for identity in &fill.targets {
                let road = &mut roads[by_identity[identity]];
                road.counts = fill.flow.counts;
                road.basis = fill.flow.basis;
                road.source_id = CONTINUITY_SOURCE;
                road.observation_source_id = fill.flow.source;
                road.observation = fill.flow.identity.clone();
                road.estimated = 15;
                road.observation_estimated = fill.flow.observation_estimated;
                road.count_origin_measured = true;
                road.observation_time_basis = fill.flow.observation_time_basis;
            }
        }
    }
}

pub fn build(
    root: &Path,
    selected: &BTreeSet<Square>,
    matcher: &Matcher,
    model: &Model,
) -> Result<Plan> {
    if !matcher.has_measurements() {
        return Ok(Plan::default());
    }
    let squares = topology::squares(root)?;
    let scope = Scope::read(&squares, selected)?;
    let squares = squares.into_iter().collect::<Vec<_>>();
    let mut shared = HashMap::<Endpoint, SharedNode>::new();
    let mut missing = HashSet::new();
    for (identity, home_missing) in squares
        .par_iter()
        .map(|(square, path)| chains::foreign_ends(root, *square, path))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
    {
        shared.entry(identity).or_default();
        if home_missing {
            missing.insert(identity);
        }
    }
    let traced_squares = squares
        .par_iter()
        .map(|(square, path)| {
            chains::square_chains(*square, path, &shared, &missing, &scope, matcher, model)
        })
        .collect::<Result<Vec<_>>>()?;
    let mut plan = Plan::default();
    let mut parts = Vec::new();
    // Merged in square order: part indices and each shared endpoint's first three ends follow it.
    for traced in traced_squares {
        for endpoint in traced.shared_ends {
            let node = shared.get_mut(&endpoint).expect("shared end");
            node.degree = (node.degree + 1).min(3);
        }
        if !traced.fills.is_empty() {
            plan.fills
                .entry(traced.square)
                .or_default()
                .extend(traced.fills);
        }
        for (part, borders) in traced.parts {
            for (endpoint, road) in borders {
                let node = shared.get_mut(&endpoint).expect("shared end");
                if node.ends.len() < 3 {
                    node.ends.push((parts.len(), road));
                }
            }
            parts.push(part);
        }
    }
    let mut parent = (0..parts.len()).collect::<Vec<_>>();
    fn representative(parent: &mut [usize], mut row: usize) -> usize {
        while parent[row] != row {
            parent[row] = parent[parent[row]];
            row = parent[row];
        }
        row
    }
    for node in shared.into_values() {
        if node.degree != 2 || node.ends.len() != 2 {
            continue;
        }
        let [(a, first), (b, second)] = node.ends.as_slice() else {
            unreachable!()
        };
        if first.continues(second) {
            let a = representative(&mut parent, *a);
            let b = representative(&mut parent, *b);
            parent[a] = b;
        }
    }
    let mut agreements = HashMap::<usize, Agreement>::new();
    for (index, part) in parts.iter().enumerate() {
        agreements
            .entry(representative(&mut parent, index))
            .or_default()
            .add(&part.agreement);
    }
    for (index, part) in parts.into_iter().enumerate() {
        let agreement = &agreements[&representative(&mut parent, index)];
        ensure!(
            !agreement.unresolved || part.targets.is_empty() || !agreement.potential_observations,
            "continuity requires original topology beyond the supplied geography at {}/{}",
            part.square.x,
            part.square.y
        );
        if !agreement.conflict
            && let Some(flow) = &agreement.flow
            && !part.targets.is_empty()
        {
            plan.fills.entry(part.square).or_default().push(Fill {
                targets: part.targets,
                flow: flow.clone(),
            });
        }
    }
    Ok(plan)
}

//! Carry an agreed observation only through complete, unbranched chains of original OSM endpoints.

use super::{
    input::{self, Road},
    matching::Matcher,
    model::Model,
    topology::{self, Endpoint},
};
use crate::dev4::Square;
use anyhow::{Result, anyhow, ensure};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::Path,
};

const CONTINUITY_SOURCE: u16 = 12;

#[derive(Clone, PartialEq)]
struct Flow {
    counts: [f64; 4],
    basis: u8,
    source: u16,
    identity: String,
}

#[derive(Default, Clone)]
struct Agreement {
    flow: Option<Flow>,
    conflict: bool,
    unresolved: bool,
}

impl Agreement {
    fn add(&mut self, other: &Self) {
        self.conflict |=
            other.conflict || matches!((&self.flow,&other.flow),(Some(a),Some(b)) if a!=b);
        self.unresolved |= other.unresolved;
        if self.flow.is_none() {
            self.flow = other.flow.clone();
        }
    }
    fn observed(road: &Road) -> Self {
        Self {
            flow: (road.measured && !road.observation.is_empty()).then(|| Flow {
                counts: road.counts,
                basis: road.basis,
                source: road.observation_source_id,
                identity: road.observation.clone(),
            }),
            conflict: false,
            unresolved: false,
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
                road.measured = false;
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
    let scope = super::continuity_scope::Scope::read(&squares, selected)?;
    let mut shared = HashMap::<Endpoint, SharedNode>::new();
    let mut missing = HashSet::new();
    for (&square, path) in &squares {
        for ends in topology::read(path)?.into_values() {
            for end in ends {
                if end.home != square {
                    shared.entry(end.identity).or_default();
                    if !root
                        .join("z9")
                        .join(end.home.x.to_string())
                        .join(end.home.y.to_string())
                        .join("square-country-city.bin")
                        .exists()
                    {
                        missing.insert(end.identity);
                    }
                }
            }
        }
    }
    let mut plan = Plan::default();
    let mut parts = Vec::new();
    for (&square, path) in &squares {
        let roads = super::matched_roads(&input::load(path)?, matcher, model)?;
        let topology = topology::read(path)?;
        ensure!(
            roads.len() == topology.len(),
            "road/source topology row count differs"
        );
        let ends = roads
            .iter()
            .map(|r| {
                topology
                    .get(&(r.way_id, r.segment_idx))
                    .map(|v| v.map(|e| e.identity))
                    .ok_or_else(|| anyhow!("missing road source identity"))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut incident = HashMap::<Endpoint, Vec<(usize, usize)>>::new();
        for (row, pair) in ends.iter().enumerate() {
            for (side, &endpoint) in pair.iter().enumerate() {
                let local = incident.entry(endpoint).or_default();
                if local.len() < 3 {
                    local.push((row, side));
                }
                if let Some(shared) = shared.get_mut(&endpoint) {
                    shared.degree = (shared.degree + 1).min(3);
                }
            }
        }
        let fillable = |r: &Road| model.continuity_classes.contains(&r.class);
        let mut visited = vec![false; roads.len()];
        for seed in 0..roads.len() {
            if visited[seed] || !fillable(&roads[seed]) {
                continue;
            }
            visited[seed] = true;
            let mut members = vec![seed];
            let mut cursor = 0;
            while cursor < members.len() {
                let row = members[cursor];
                cursor += 1;
                if ends[row][0] == ends[row][1] {
                    continue;
                }
                for (side, &endpoint) in ends[row].iter().enumerate() {
                    if shared.contains_key(&endpoint) {
                        continue;
                    }
                    let touching = &incident[&endpoint];
                    if touching.len() != 2 {
                        continue;
                    }
                    let Some(&(next, next_side)) = touching.iter().find(|(next, _)| *next != row)
                    else {
                        continue;
                    };
                    if !visited[next]
                        && fillable(&roads[next])
                        && ChainEnd::new(&roads[row], side)
                            .continues(&ChainEnd::new(&roads[next], next_side))
                    {
                        visited[next] = true;
                        members.push(next);
                    }
                }
            }
            let mut agreement = Agreement::default();
            let mut targets = Vec::new();
            let mut borders = Vec::new();
            let mut unresolved = false;
            for row in members {
                let road = &roads[row];
                agreement.add(&Agreement::observed(road));
                if (road.source_id == 0 || road.source_id == CONTINUITY_SOURCE) && scope.needs(road)
                {
                    targets.push((road.way_id, road.segment_idx));
                }
                if ends[row][0] != ends[row][1] {
                    for (side, &endpoint) in ends[row].iter().enumerate() {
                        if shared.contains_key(&endpoint) {
                            borders.push((endpoint, ChainEnd::new(road, side)));
                        }
                        unresolved |= missing.contains(&endpoint);
                    }
                }
            }
            agreement.unresolved |= unresolved;
            if borders.is_empty() {
                if !agreement.conflict
                    && let Some(flow) = agreement.flow
                    && !targets.is_empty()
                {
                    plan.fills
                        .entry(square)
                        .or_default()
                        .push(Fill { targets, flow });
                }
            } else {
                let part = parts.len();
                parts.push(Part {
                    agreement,
                    targets,
                    square,
                });
                for (endpoint, road) in borders {
                    let node = shared.get_mut(&endpoint).expect("shared end");
                    if node.ends.len() < 3 {
                        node.ends.push((part, road));
                    }
                }
            }
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
            !agreement.unresolved || part.targets.is_empty(),
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

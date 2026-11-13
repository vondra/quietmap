//! Trace one square's chains of continuing pieces up to the endpoints it shares with other squares.

use super::{Agreement, CONTINUITY_SOURCE, ChainEnd, Fill, Part, SharedNode};
use crate::{
    dev4::Square,
    roads::{
        continuity_scope::Scope,
        input::{self, Road},
        matching::Matcher,
        model::Model,
        topology::{self, Endpoint},
    },
};
use anyhow::{Result, anyhow, ensure};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

/// One square's chains: those complete in it, and the parts ending at endpoints squares share.
pub(super) struct SquareChains {
    pub(super) square: Square,
    pub(super) shared_ends: Vec<Endpoint>,
    pub(super) fills: Vec<Fill>,
    pub(super) parts: Vec<(Part, Vec<(Endpoint, ChainEnd)>)>,
}

/// The endpoints of a square's pieces that lie in another square, and whether it is missing.
pub(super) fn foreign_ends(
    root: &Path,
    square: Square,
    path: &Path,
) -> Result<Vec<(Endpoint, bool)>> {
    let mut foreign = Vec::new();
    for ends in topology::read(path)?.into_values() {
        for end in ends {
            if end.home != square {
                let missing = !root
                    .join("z9")
                    .join(end.home.x.to_string())
                    .join(end.home.y.to_string())
                    .join("square-country-city.bin")
                    .exists();
                foreign.push((end.identity, missing));
            }
        }
    }
    Ok(foreign)
}

pub(super) fn square_chains(
    square: Square,
    path: &Path,
    shared: &HashMap<Endpoint, SharedNode>,
    missing: &HashSet<Endpoint>,
    scope: &Scope,
    matcher: &Matcher,
    model: &Model,
) -> Result<SquareChains> {
    let roads = crate::roads::matched_roads(&input::load(path)?, matcher, model)?;
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
    let mut shared_ends = Vec::new();
    let mut incident = HashMap::<Endpoint, Vec<(usize, usize)>>::new();
    for (row, pair) in ends.iter().enumerate() {
        for (side, &endpoint) in pair.iter().enumerate() {
            let local = incident.entry(endpoint).or_default();
            if local.len() < 3 {
                local.push((row, side));
            }
            if shared.contains_key(&endpoint) {
                shared_ends.push(endpoint);
            }
        }
    }
    let fillable = |r: &Road| model.continuity_classes.contains(&r.class);
    let mut fills = Vec::new();
    let mut parts = Vec::new();
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
            agreement.potential_observations |= matcher.has_measurements_in(road.country.0);
            if (road.source_id == 0 || road.source_id == CONTINUITY_SOURCE) && scope.needs(road) {
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
                fills.push(Fill { targets, flow });
            }
        } else {
            parts.push((
                Part {
                    agreement,
                    targets,
                    square,
                },
                borders,
            ));
        }
    }
    eprintln!(
        "road-continuity: {}/{}: {} mapped pieces",
        square.x,
        square.y,
        roads.len()
    );
    Ok(SquareChains {
        square,
        shared_ends,
        fills,
        parts,
    })
}

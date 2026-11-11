//! Prepare road observations and allocate each physical cross-section through one common model.

mod allocation;
#[cfg(test)]
mod assignment_tests;
pub(crate) mod calibration;
mod continuity;
#[cfg(test)]
mod continuity_country_tests;
mod continuity_scope;
#[cfg(test)]
mod continuity_tests;
#[cfg(test)]
mod count_scope_tests;
pub(crate) mod input;
mod matching;
#[cfg(test)]
mod matching_ambiguity_tests;
mod matching_assignment;
mod matching_distance;
mod matching_streets;
mod matching_targets;
pub(crate) use continuity::CONTINUITY_SOURCE;
pub(crate) use matching_distance::flat_distance;
pub(crate) use matching_distance::great_circle as great_circle_distance;
pub(crate) use matching_distance::segment_distance;
pub(crate) use matching_targets::name as canonical_road_name;
pub(crate) mod model;
pub(crate) mod observation;
pub(crate) mod observation_groups;
pub(crate) mod observation_io;
pub(crate) mod observation_sample;
pub(crate) mod observation_target;
#[cfg(test)]
mod order_tests;
mod settlement;
mod spatial;
mod speed_transitions;
pub(crate) mod speed_validity;
pub(crate) mod tagged_speed_fit;
#[cfg(test)]
mod target_tests;
#[cfg(test)]
mod tests;
mod topology;
mod write;

use crate::{
    dev4::Square,
    year::{Kind, YearInputs},
};
use anyhow::{Result, ensure};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

/// The squares of a prepared or mapped tree that hold roads.
pub(crate) fn prepared_squares(root: &Path) -> Result<Vec<Square>> {
    Ok(topology::squares(root)?.into_keys().collect())
}

fn road_path(root: &Path, square: Square) -> PathBuf {
    root.join("z9")
        .join(square.x.to_string())
        .join(square.y.to_string())
        .join("roads.arrow")
}

fn matched_roads(
    batches: &[arrow_array::RecordBatch],
    matcher: &matching::Matcher,
    model: &model::Model,
) -> Result<Vec<input::Road>> {
    let mut rows = Vec::new();
    for batch in batches {
        let mut roads = input::roads(batch)?;
        for road in &mut roads {
            matcher.apply(road, model)?;
        }
        rows.extend(roads);
    }
    Ok(rows)
}

fn inferred_roads(
    structures: &Path,
    square: Square,
    roads: &mut [input::Road],
    model: &model::Model,
) -> Result<()> {
    settlement::classify(structures, roads)?;
    let structure_path = structures
        .join("z9")
        .join(square.x.to_string())
        .join(square.y.to_string())
        .join("structures.arrow");
    crate::traffic::preparation::apply(roads, &structure_path, &model.demand)?;
    speed_transitions::apply(roads, &model.speed_transitions);
    Ok(())
}

pub fn build(inputs: &YearInputs, squares: &[Square], output: &Path) -> Result<()> {
    ensure!(!output.exists(), "road output must be a new tree");
    ensure!(
        !squares.is_empty() && squares.iter().all(|s| s.x < 512 && s.y < 512),
        "invalid road selection"
    );
    let mapped = inputs
        .required(Kind::OsmGeography)
        .map_err(anyhow::Error::msg)?;
    let model = model::Model::read(inputs)?;
    let mut observations = Vec::new();
    for path in inputs
        .paths(Kind::RoadObservations)
        .map_err(anyhow::Error::msg)?
    {
        observations.extend(observation_io::read(path, inputs)?);
    }
    observation_io::validate(&observations, inputs)?;
    let mut matcher = matching::Matcher::new(observations)?;
    matcher.assign_counter_ways(mapped)?;
    let selected = squares.iter().copied().collect::<BTreeSet<_>>();
    let continuity = continuity::build(mapped, &selected, &matcher, &model)?;
    std::fs::create_dir(output)?;
    for square in selected {
        let path = road_path(mapped, square);
        if !path.exists() {
            continue;
        }
        let batches = input::load(&path)?;
        let mut roads = matched_roads(&batches, &matcher, &model)?;
        continuity.apply(square, &mut roads);
        let structures = inputs
            .required(Kind::Structures)
            .map_err(anyhow::Error::msg)?;
        inferred_roads(structures, square, &mut roads, &model)?;
        let mut neighbors = roads
            .iter()
            .filter(|r| r.direction != 0)
            .cloned()
            .collect::<Vec<_>>();
        for dx in -1..=1 {
            for dy in -1..=1 {
                let y = square.y as i32 + dy;
                if !(0..512).contains(&y) || (dx == 0 && dy == 0) {
                    continue;
                }
                let neighbor = Square {
                    x: (square.x as i32 + dx).rem_euclid(512) as u32,
                    y: y as u32,
                };
                let path = road_path(mapped, neighbor);
                if path.exists() {
                    let mut adjacent = matched_roads(&input::load(&path)?, &matcher, &model)?;
                    continuity.apply(neighbor, &mut adjacent);
                    if adjacent.iter().any(|r| r.direction != 0) {
                        inferred_roads(structures, neighbor, &mut adjacent, &model)?;
                        neighbors.extend(adjacent.into_iter().filter(|r| r.direction != 0));
                    }
                }
            }
        }
        let index = spatial::RoadIndex::new(neighbors);
        let target = road_path(output, square);
        std::fs::create_dir_all(target.parent().expect("square parent"))?;
        write::stage(&target, &batches, &roads, &index, &model)?;
        eprintln!(
            "roads: {}/{}: {} mapped pieces",
            square.x,
            square.y,
            roads.len()
        );
    }
    Ok(())
}

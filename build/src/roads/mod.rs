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
use rayon::prelude::*;
use std::{
    collections::{BTreeMap, BTreeSet},
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

/// The eight neighbours in the order their one-way pieces enter a square's alternatives index,
/// which allocation's candidate order, and so its ties, follow.
fn neighbours_in_index_order(square: Square) -> impl Iterator<Item = Square> {
    (-1..=1).flat_map(move |dx| {
        (-1..=1).filter_map(move |dy| {
            let y = square.y as i32 + dy;
            ((0..512).contains(&y) && (dx, dy) != (0, 0)).then(|| Square {
                x: (square.x as i32 + dx).rem_euclid(512) as u32,
                y: y as u32,
            })
        })
    })
}

/// The year's inputs every square's road preparation reads.
struct Preparation<'a> {
    mapped: &'a Path,
    structures: &'a Path,
    matcher: &'a matching::Matcher,
    continuity: &'a continuity::Plan,
    model: &'a model::Model,
}

impl Preparation<'_> {
    /// A square's sorted source batches and its pieces with their counts and continuations.
    fn matched(
        &self,
        square: Square,
    ) -> Result<Option<(Vec<arrow_array::RecordBatch>, Vec<input::Road>)>> {
        let path = road_path(self.mapped, square);
        if !path.exists() {
            return Ok(None);
        }
        let batches = input::load(&path)?;
        let mut roads = matched_roads(&batches, self.matcher, self.model)?;
        self.continuity.apply(square, &mut roads);
        Ok(Some((batches, roads)))
    }

    /// A square's one-way pieces as its neighbours' alternatives see them: inferred over the
    /// whole square, as settlement and street demand read every piece of it.
    fn one_way_roads(&self, square: Square) -> Result<Vec<input::Road>> {
        let Some((_, mut roads)) = self.matched(square)? else {
            return Ok(Vec::new());
        };
        if roads.iter().any(|r| r.direction != 0) {
            inferred_roads(self.structures, square, &mut roads, self.model)?;
        }
        roads.retain(|r| r.direction != 0);
        // Three rows of these are kept; without this they would keep every piece's room.
        roads.shrink_to_fit();
        Ok(roads)
    }

    /// Writes a square's prepared pieces, split where its own or its neighbours' one-way
    /// carriageways run alongside.
    fn write(
        &self,
        square: Square,
        one_way: &BTreeMap<Square, Vec<input::Road>>,
        output: &Path,
    ) -> Result<()> {
        let Some((batches, mut roads)) = self.matched(square)? else {
            return Ok(());
        };
        inferred_roads(self.structures, square, &mut roads, self.model)?;
        let mut alternatives = roads
            .iter()
            .filter(|r| r.direction != 0)
            .cloned()
            .collect::<Vec<_>>();
        for neighbour in neighbours_in_index_order(square) {
            alternatives.extend(one_way[&neighbour].iter().cloned());
        }
        let index = spatial::RoadIndex::new(alternatives);
        let target = road_path(output, square);
        std::fs::create_dir_all(target.parent().expect("square parent"))?;
        write::stage(&target, &batches, &roads, &index, self.model)?;
        eprintln!(
            "roads: {}/{}: {} mapped pieces",
            square.x,
            square.y,
            roads.len()
        );
        Ok(())
    }
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
    let preparation = Preparation {
        mapped,
        structures: inputs
            .required(Kind::Structures)
            .map_err(anyhow::Error::msg)?,
        matcher: &matcher,
        continuity: &continuity,
        model: &model,
    };
    let mut rows = BTreeMap::<u32, Vec<Square>>::new();
    for square in selected
        .into_iter()
        .filter(|&square| road_path(mapped, square).exists())
    {
        rows.entry(square.y).or_default().push(square);
    }
    // Row by row, each square's one-way pieces are inferred once and kept only while the rows on
    // either side of it are written, so memory holds three rows of them, not the world's.
    let mut one_way = BTreeMap::<Square, Vec<input::Road>>::new();
    for (row, written) in rows {
        one_way.retain(|square, _| square.y + 1 >= row);
        let missing = written
            .iter()
            .flat_map(|&square| neighbours_in_index_order(square))
            .filter(|square| !one_way.contains_key(square))
            .collect::<BTreeSet<_>>();
        let inferred = missing
            .into_par_iter()
            .map(|square| Ok((square, preparation.one_way_roads(square)?)))
            .collect::<Result<Vec<_>>>()?;
        one_way.extend(inferred);
        written
            .par_iter()
            .try_for_each(|&square| preparation.write(square, &one_way, output))?;
    }
    Ok(())
}

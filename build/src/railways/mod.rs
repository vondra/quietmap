//! Match normalized rail movements to original topology, then finalize traffic and crossing approaches.
mod alignment;
pub(crate) mod config;
pub(crate) mod coverage;
mod encode;
mod horn_geometry;
mod horn_write;
mod horns;
mod leg_matcher;
mod local_match;
mod matcher;
mod merge;
mod model;
pub(crate) mod movements;
pub(crate) mod network;
mod parallel_geometry;
mod parallel_tracks;
pub(crate) mod prior_rules;
mod route;
mod search;
mod service;
mod source_routes;
mod split;
mod square_intervals;
mod station_pairs;
mod topology;
pub(crate) mod types;
mod write;
mod yards;

use crate::year::{Kind, YearInputs};
use std::path::Path;

pub fn build(
    inputs: &YearInputs,
    squares: &[crate::dev4::Square],
    output: &Path,
) -> Result<(), String> {
    if output.exists() {
        return Err("railways output must be a new directory".into());
    }
    if squares
        .iter()
        .any(|square| square.x >= 512 || square.y >= 512)
    {
        return Err("invalid railway square".into());
    }
    let model = model::RailModel::read(inputs.required(Kind::RailModel)?)?;
    let geography = inputs.required(Kind::OsmGeography)?;
    let flows = inputs.required(Kind::RailFlows)?;
    let selected = squares
        .iter()
        .map(|square| [square.x as u16, square.y as u16])
        .collect();
    let matched = matcher::read(
        &model,
        geography,
        flows,
        &inputs.manifest.flows.start,
        &inputs.manifest.flows.end,
        &selected,
    )?;
    let mut rows = matched.claims;
    let mut crossings = matched.crossings;
    let mut squares = squares.to_vec();
    squares.sort_unstable();
    squares.dedup();
    for square in squares {
        let relative = format!("z9/{}/{}", square.x, square.y);
        let key = [square.x as u16, square.y as u16];
        let intervals = square_intervals::grouped(rows.remove(&key).unwrap_or_default());
        let rows = write::finalize_square(
            &model,
            &geography.join(&relative),
            &crossings.remove(&key).unwrap_or_default(),
            &intervals,
            &output.join(&relative),
            crate::osm::geometry::Square {
                x: square.x as u16,
                y: square.y as u16,
            },
        )?;
        eprintln!("railways: {relative}: {rows} finalized rows");
    }
    Ok(())
}

#[cfg(test)]
mod fixture_tests;

#[cfg(test)]
mod prior_tests;
#[cfg(test)]
mod registry_tests;
#[cfg(test)]
mod ruler_fixture;

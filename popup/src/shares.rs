//! Each row's share of the click's loudness: every moment's loudness shared among the sources by
//! their A-weighted energy at that moment, summed over the moments as Nden sums them, so the shares
//! add up to the whole (the rows' own Nden do not: two equal steady roads each read about 0.8 of
//! their sum alone). The moments are those of the level distributions ([`crate::percentiles`]):
//! each weather state of a period with every moving line at one of its levels over what holds
//! steady, the flows at the period's mean (the hours' profile moved the last row's Nden by 1.7 % at
//! most, 2026-10-09). A moving line's part comes from two passes over the lines: forward, the
//! distribution of the moments before it; backward, the loudness per energy the lines after it
//! leave in expectation. What holds steady takes its energy's part of every moment.

use crate::distribution::{BINS, Distribution, value_levels};
use crate::loudness::Curve;
use crate::percentiles::{Line, WEATHER_STATES, at_mean_flow, moment};
use crate::selection::LayerSelection;
use crate::update::Contributor;
use physics::bands::{PERIOD_HOURS, PERIOD_PENALTY_DB, PERIODS};
use rayon::prelude::*;
use std::collections::HashMap;
use tiles::sources::Layer;

/// The shares of the visitor's list, fractions of the whole: each listed contributor's, the
/// aircraft row's and the last row's (everything else).
#[derive(Debug, Clone, PartialEq)]
pub struct RowShares {
    pub listed: Vec<f64>,
    pub aircraft: f64,
    pub rest: f64,
}

/// The shares of `listed` (the ground contributors the list names, in order), the aircraft layer
/// (its flights and its airports' ground operations) and the rest, from the click's lines and what
/// they leave steady ([`crate::percentiles::click_lines`]).
pub fn row_shares(
    selections: &[LayerSelection],
    listed: &[&Contributor],
    (lines, remainder): (&[Line], [f64; PERIODS]),
    curves: &[Curve; PERIODS],
) -> RowShares {
    let mut lines = lines.to_vec();
    at_mean_flow(&mut lines);
    let (aircraft, rest) = (listed.len(), listed.len() + 1);
    let rows: HashMap<u64, usize> = listed
        .iter()
        .enumerate()
        .map(|(row, contributor)| (contributor.group_key, row))
        .collect();
    let aircraft_layer = selections
        .iter()
        .find(|selection| selection.layer == Layer::Aircraft);
    // The flights' line has no contributor; the airports' ground operations are the aircraft row's.
    let owners: Vec<usize> = lines
        .iter()
        .map(|line| match rows.get(&line.key) {
            Some(&row) => row,
            None if line.key == u64::MAX => aircraft,
            None if aircraft_layer
                .is_some_and(|layer| layer.contributors.contains_key(&line.key)) =>
            {
                aircraft
            }
            None => rest,
        })
        .collect();
    // What holds steady outside the lines: a listed row without a line its energy, the aircraft
    // layer what its flights and lines leave of it, the rest the remainder.
    let mut steady = vec![[0.0; PERIODS]; listed.len() + 2];
    for (row, contributor) in listed.iter().enumerate() {
        if !lines.iter().any(|line| line.key == contributor.group_key) {
            steady[row] = contributor.energy;
        }
    }
    let aircraft_energy = aircraft_layer.map_or([0.0; PERIODS], |layer| layer.answer_energy());
    for period in 0..PERIODS {
        let in_lines: f64 = lines
            .iter()
            .zip(&owners)
            .filter(|(_, owner)| **owner == aircraft)
            .map(|(line, _)| line.energy[period])
            .sum();
        steady[aircraft][period] = (aircraft_energy[period] - in_lines).max(0.0);
        let named: f64 = steady[..rest].iter().map(|energy| energy[period]).sum();
        steady[rest][period] = (remainder[period] - named).max(0.0);
    }
    let shares = loudness_shares(&lines, &owners, &steady, curves);
    let whole: f64 = shares.iter().sum();
    let fraction = |share: f64| if whole > 0.0 { share / whole } else { 0.0 };
    RowShares {
        listed: shares[..listed.len()]
            .iter()
            .map(|&share| fraction(share))
            .collect(),
        aircraft: fraction(shares[aircraft]),
        rest: fraction(shares[rest]),
    }
}

/// The loudness each owner makes (sone as in Nden: the periods over their hours with the evening
/// and night penalties): the `lines`' by their `owners` (one each) and `steady` per owner.
pub fn loudness_shares(
    lines: &[Line],
    owners: &[usize],
    steady: &[[f64; PERIODS]],
    curves: &[Curve; PERIODS],
) -> Vec<f64> {
    let means: Vec<Vec<[f64; WEATHER_STATES]>> = (0..PERIODS)
        .map(|period| lines.iter().map(|line| line.state_means(period)).collect())
        .collect();
    let parts: Vec<Vec<f64>> = (0..PERIODS * WEATHER_STATES)
        .into_par_iter()
        .map(|task| {
            let (period, state) = (task / WEATHER_STATES, task % WEATHER_STATES);
            let weight = PERIOD_HOURS[period] / 24.0 / WEATHER_STATES as f64;
            moment_shares(
                (lines, &means[period], owners),
                steady,
                &curves[period],
                (period, state),
            )
            .into_iter()
            .map(|share| weight * share)
            .collect()
        })
        .collect();
    (0..steady.len())
        .map(|owner| parts.iter().map(|part| part[owner]).sum())
        .collect()
}

/// The owners' loudness over the moments of one weather `state` of `period`.
fn moment_shares(
    (lines, means, owners): (&[Line], &[[f64; WEATHER_STATES]], &[usize]),
    steady: &[[f64; PERIODS]],
    curve: &Curve,
    (period, state): (usize, usize),
) -> Vec<f64> {
    let total_steady: f64 = steady.iter().map(|energy| energy[period]).sum();
    let moment = moment(lines, means, total_steady, (period, 0, state));
    // Forward: the moments before each moving line, from what holds steady.
    let mut silence = Distribution::empty();
    silence.silent = 1.0;
    let mut before = vec![silence.over(moment.floor)];
    for (_, values) in &moment.moving {
        let next = before[before.len() - 1].with_line(values);
        before.push(next);
    }
    // The loudness per energy at the end, by its bin; silence makes none.
    let penalty = PERIOD_PENALTY_DB[period];
    let mut after = vec![0.0; BINS];
    let mut after_silent = 0.0;
    let mut per_energy_mean = 0.0;
    for (bin, intensity, share) in before[before.len() - 1].bins() {
        after[bin] = curve.at(10.0 * intensity.log10() + penalty) / intensity;
        per_energy_mean += share * after[bin];
    }
    let mut shares = vec![0.0; steady.len()];
    // Backward: each moving line's energy times the loudness per energy its moments end in.
    let mut next = vec![0.0; BINS];
    for (position, (line, values)) in moment.moving.iter().enumerate().rev() {
        let levels = value_levels(values);
        let prefix = &before[position];
        let mut own = 0.0;
        // From a moment at `level_db`: the loudness per energy expected at the end, and the
        // line's energy times it.
        let step = |level_db: f64| {
            let (mut expected, mut owned) = (0.0, 0.0);
            for (&(value, weight), &value_db) in values.iter().zip(&levels) {
                let per_energy = Distribution::sum_bin(level_db, value_db)
                    .map_or(after_silent, |bin| after[bin]);
                expected += weight * per_energy;
                owned += weight * value * per_energy;
            }
            (expected, owned)
        };
        for (bin, intensity, share) in prefix.bins() {
            let (expected, owned) = step(10.0 * intensity.log10());
            next[bin] = expected;
            own += share * owned;
        }
        let (expected_silent, owned_silent) = step(f64::NEG_INFINITY);
        own += prefix.silent * owned_silent;
        shares[owners[*line]] += own;
        std::mem::swap(&mut after, &mut next);
        after_silent = expected_silent;
    }
    for (owner, energy) in steady.iter().enumerate() {
        shares[owner] += energy[period] * per_energy_mean;
    }
    for &(line, mean) in &moment.holding {
        shares[owners[line]] += mean * per_energy_mean;
    }
    shares
}

#[cfg(test)]
#[path = "shares_tests.rs"]
mod tests;

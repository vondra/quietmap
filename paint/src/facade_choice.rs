//! Each building's loudest façade by the popup's rule (`popup::building`), its sums on the card.
//!
//! A building's sources are ranked as a click inside it ranks them, from the candidates the index
//! finds around its façades; every façade's sum of them is evaluated through the painter's batch, as
//! its pixels' pairs are, and the loudest wins; façades the card puts within [`TIE_MARGIN_DB`] of the
//! loudest are summed again on the cores in double precision, as the popup sums them.

use crate::exact::{Pair, Point, evaluate_all};
use crate::paint::{PASS_BLOCKS, metres_at, pixel_of, runs_within};
use crate::passes::PASSES_AT_ONCE;
use crate::points::PointPainter;
use crate::square::Square;
use physics::bands::{energy, lden_energy};
use physics::weather::PlaceWeather;
use popup::building::{
    FACADE_SOURCE_LIMIT, Facade, facade_level, facade_sources, facades, loudest, ranking_gain,
};
use popup::candidates::{Candidate, GROUND_REACH_M, loud_energy};
use popup::evaluate::Scratch;
use rayon::prelude::*;
use std::collections::HashMap;
use std::sync::atomic::Ordering;

/// Façades whose card sum is within this of the loudest's are decided again on the cores. Over
/// 804,022 façades of 67,066 buildings in five squares (an RTX 2080 Ti, 11 November 2026) the card's
/// sums differed from the cores' by a median 2e-5 dB, p99 0.004-0.056 dB and at most 1.4-6.9 dB:
/// its choice differed at 79 buildings (0.12 %), at 36 after summing again the 3,439 (5 %) whose
/// façades came within this; no margin catches the far tail.
const TIE_MARGIN_DB: f64 = 0.01;

/// A building some pixel of the square stands in, with what its façade's choice takes.
pub(crate) struct Building {
    pub footprint: u64,
    pub facades: Vec<Facade>,
    /// Whether it crosses the square's edge, where the square beside answers it too.
    pub crosses_edge: bool,
    /// The candidates its façades sum, in the rule's order.
    pub sources: Vec<u32>,
}

/// The buildings of the square's `indoor` pixels (their places and footprints) that have an
/// exposed façade, by footprint.
pub(crate) fn buildings(
    square: &Square,
    n: usize,
    indoor: &[(usize, u64)],
) -> Result<Vec<Building>, String> {
    let mut first: HashMap<u64, usize> = HashMap::new();
    for &(at, footprint) in indoor {
        first.entry(footprint).or_insert(at);
    }
    let mut firsts: Vec<(u64, usize)> = first.into_iter().collect();
    firsts.sort_unstable();
    let found: Vec<Option<Building>> = (firsts.par_iter())
        .map(|&(footprint, at)| -> Result<_, String> {
            let inside = metres_at(square, n, (at % n) as f64 + 0.5, (at / n) as f64 + 0.5);
            let Some(building) = square.obstacles.enclosing_building(inside)? else {
                return Ok(None);
            };
            let receivers = square.obstacles.facade_receivers(&building)?;
            if receivers.is_empty() {
                return Ok(None);
            }
            let facades = facades(&building, &receivers, &square.ground, &square.obstacles)?;
            // The sources are ranked at the first façade's weather, as a click ranks them at its own.
            let weather = weather_at(square, &facades[0]);
            let crosses_edge = (building.rings.iter())
                .flat_map(|ring| &ring.points)
                .any(|&position| pixel_of(square, n, position).is_none());
            Ok(Some(Building {
                footprint,
                sources: ranked_sources(square, &facades, &weather),
                facades,
                crosses_edge,
            }))
        })
        .collect::<Result<_, String>>()?;
    Ok(found.into_iter().flatten().collect())
}

/// The candidates the rule sums at `facades`, as a click there takes them from every source within
/// the ground reach of a façade (equal bounds in the index's order), the first found from the
/// candidates reaching the façades: any other is loud at none, so when the limit's last is loud no
/// other can rank.
fn ranked_sources(square: &Square, facades: &[Facade], weather: &PlaceWeather) -> Vec<u32> {
    let gain = ranking_gain(facades, weather);
    let (mut low, mut high) = (facades[0].position, facades[0].position);
    for facade in facades {
        for axis in 0..2 {
            low[axis] = low[axis].min(facade.position[axis]);
            high[axis] = high[axis].max(facade.position[axis]);
        }
    }
    let rank = |near: &mut Vec<u32>| -> Vec<(f64, u32)> {
        near.retain(|&index| {
            let candidate = &square.candidates[index as usize];
            (facades.iter())
                .any(|facade| candidate.distance_from(facade.position) <= GROUND_REACH_M)
        });
        let candidates: Vec<&Candidate> = (near.iter())
            .map(|&index| &square.candidates[index as usize])
            .collect();
        (facade_sources(facades, &candidates, &square.attributes, &gain).into_iter())
            .map(|(bound, at)| (bound, near[at]))
            .collect()
    };
    let mut near = Vec::new();
    square.index.reaching(low, high, &mut near);
    let mut ranked = rank(&mut near);
    if ranked.len() < FACADE_SOURCE_LIMIT || !loud_energy(ranked[FACADE_SOURCE_LIMIT - 1].0) {
        square.index.within(low, high, GROUND_REACH_M, &mut near);
        ranked = rank(&mut near);
    }
    ranked.into_iter().map(|(_, index)| index).collect()
}

/// Each building's chosen façade (its index among the building's), every façade's sum of its
/// sources evaluated through the painter's batch, runs of buildings as the pixels' passes go.
pub(crate) fn choose(painter: &PointPainter, buildings: &[Building]) -> Result<Vec<usize>, String> {
    let bounds: Vec<usize> = (buildings.iter())
        .map(|building| building.facades.len() * building.sources.len())
        .collect();
    let most = buildings.len().div_ceil(PASSES_AT_ONCE).min(PASS_BLOCKS);
    let chosen = painter.in_passes(runs_within(&bounds, most), |run| {
        choose_run(painter, &buildings[run])
    })?;
    Ok(chosen.into_iter().flat_map(|(_, run)| run).collect())
}

/// Whether the batch sums a building's façades: one façade needs no sums, and one crossing the
/// square's edge is summed on the cores.
fn summed_by_batch(building: &Building) -> bool {
    building.facades.len() > 1 && !building.crosses_edge
}

fn choose_run(painter: &PointPainter, buildings: &[Building]) -> Result<Vec<usize>, String> {
    let square = painter.square;
    let summed = || {
        buildings
            .iter()
            .filter(|building| summed_by_batch(building))
    };
    let points: Vec<Point> = (summed().flat_map(|building| &building.facades))
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(|facade| facade_point(square, facade))
        .collect::<Result<_, String>>()?;
    // Each summed building's façades' pairs, façade by façade, from its first point and pair.
    let (mut pairs, mut starts) = (Vec::new(), Vec::with_capacity(buildings.len()));
    let mut point = 0u32;
    for building in summed() {
        starts.push((point as usize, pairs.len()));
        for _ in &building.facades {
            pairs.extend((building.sources.iter()).map(|&index| Pair::new(point, index)));
            point += 1;
        }
    }
    painter.pairs.fetch_add(pairs.len(), Ordering::Relaxed);
    let evaluated = evaluate_all(square, &points, &pairs, painter.batch)?;
    let mut starts = starts.into_iter();
    let levels: Vec<Option<Vec<f64>>> = (buildings.iter())
        .map(|building| {
            if !summed_by_batch(building) {
                return None;
            }
            let (point, pair) = starts.next()?;
            let count = building.sources.len();
            Some(
                (0..building.facades.len())
                    .map(|k| {
                        let mut total = [0.0; physics::bands::PERIODS];
                        let at = pair + k * count;
                        for energy in &evaluated.pairs[at..at + count] {
                            for (sum, value) in total.iter_mut().zip(energy.mixed) {
                                *sum += value;
                            }
                        }
                        lden_energy(&total) * points[point + k].reflection()
                    })
                    .collect(),
            )
        })
        .collect();
    (buildings.par_iter().zip(levels))
        .map_init(Scratch::default, |scratch, (building, levels)| {
            decide(square, building, levels.as_deref(), scratch)
        })
        .collect()
}

/// The rule's choice from the batch's `levels`: the loudest, or, where others lie within
/// [`TIE_MARGIN_DB`] of it, the loudest of those summed on the cores. A building crossing the
/// square's edge (no `levels`) is decided on the cores among all its façades, so that the square
/// beside, whose card sums differ in their last digits, chooses alike.
fn decide(
    square: &Square,
    building: &Building,
    levels: Option<&[f64]>,
    scratch: &mut Scratch,
) -> Result<usize, String> {
    let near: Vec<usize> = match levels {
        _ if building.facades.len() == 1 => return Ok(0),
        None => (0..building.facades.len()).collect(),
        Some(levels) => {
            let best = levels[loudest(levels)];
            (0..levels.len())
                .filter(|&k| levels[k] >= best * energy(-TIE_MARGIN_DB))
                .collect()
        }
    };
    if near.len() == 1 {
        return Ok(near[0]);
    }
    let candidates: Vec<&Candidate> = (building.sources.iter())
        .map(|&index| &square.candidates[index as usize])
        .collect();
    let sources: Vec<usize> = (0..candidates.len()).collect();
    let exact = (near.iter())
        .map(|&k| {
            facade_level(
                &building.facades[k],
                &sources,
                &candidates,
                &square.attributes,
                &square.ground,
                &square.obstacles,
                weather_at(square, &building.facades[k]),
                scratch,
            )
        })
        .collect::<Result<Vec<f64>, String>>()?;
    Ok(near[loudest(&exact)])
}

/// The weather at `facade`'s place, which its sum takes on the cores as on the card.
fn weather_at(square: &Square, facade: &Facade) -> PlaceWeather {
    let (lat, lon) = square.frame.to_mercator(facade.position).to_degrees();
    square.weather.place(lat, lon)
}

/// The point a chosen façade is painted at, as the popup answers there.
pub(crate) fn facade_point(square: &Square, facade: &Facade) -> Result<Point, String> {
    Point::with_receiver_properties(square, facade.position, 0, facade.reflection_db)
}

#[cfg(test)]
#[path = "facade_choice_tests.rs"]
mod tests;

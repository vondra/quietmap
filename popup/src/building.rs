//! A click inside a building answers at its loudest façade, without indoor attenuation (as dev4
//! 7c533e17): of the CNOSSOS-EU 2.8 façade receivers, the one where the sources with the greatest
//! bound at any façade sum to the highest Lden wins (ties to the first in canonical order); the
//! ring loop then answers there with every source. At a façade the own building still screens what
//! stands behind it, and its footprint does not count for the reflection bonus (2.8). The map's
//! painter chooses by the same rule, its sums on a card (`paint/src/facade_choice.rs`).

use crate::answer::RECEIVER_HEIGHT_M;
use crate::candidates::{Attributes, Candidate};
use crate::evaluate::{Receiver, Scratch, received_energy};
use crate::obstacles::{FacadeReceiver, Footprint, Scene};
use crate::scene::Ground;
use physics::bands::PERIODS;
use physics::bands::lden_energy;
use physics::bound::{ReceiverBound, received_lden_ceiling, receiver_bound};
use physics::weather::PlaceWeather;
use rayon::prelude::*;
use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;

/// Sources evaluated at every façade to choose the loudest: at 76 buildings near measured stations
/// dev4's 11 chose a façade 0.5 to 3.2 dB below the loudest at five, 100 the loudest at each but
/// one where the flights decide, which the rule leaves out (measured 2026-10-11).
pub const FACADE_SOURCE_LIMIT: usize = 100;

/// Where a building click is answered.
#[derive(Clone, Copy, Debug)]
pub struct Facade {
    /// Click metres, 0.1 m in front of the façade.
    pub position: [f64; 2],
    pub outward_bearing_deg: f64,
    /// Canonical index among the building's exposed receivers.
    pub index: usize,
    pub altitude_m: f64,
    pub reflection_db: f64,
}

/// The building a click stands in and where it is answered.
#[derive(Clone, Copy, Debug)]
pub struct BuildingClick {
    pub footprint_id: u64,
    pub height_m: f64,
    /// Exposed façade receivers of the building.
    pub receivers: usize,
    /// `None`: no façade is exposed and the building is not assessed.
    pub facade: Option<Facade>,
}

/// The exposed `receivers` of `footprint` as the evaluation sees them, in canonical order.
pub fn facades(
    footprint: &Footprint,
    receivers: &[FacadeReceiver],
    ground: &Ground,
    obstacles: &Scene<'_>,
) -> Result<Vec<Facade>, String> {
    (receivers.iter().enumerate())
        .map(|(index, receiver)| {
            Ok(Facade {
                position: receiver.position,
                outward_bearing_deg: receiver.outward_bearing_deg,
                index,
                altitude_m: ground.at(receiver.position)?.height_m + RECEIVER_HEIGHT_M,
                reflection_db: obstacles.reflection_db(receiver.position, Some(footprint.id))?,
            })
        })
        .collect()
}

/// The bound the sources are ranked by: the building's weather with the largest reflection bonus
/// of any of its façades.
pub fn ranking_gain(facades: &[Facade], weather: &PlaceWeather) -> ReceiverBound {
    let largest_reflection = (facades.iter())
        .map(|facade| facade.reflection_db)
        .fold(0.0, f64::max);
    receiver_bound(
        weather.favourable.maximum(),
        largest_reflection,
        weather.alpha_db_per_km,
    )
}

/// A bound and its candidate's place in the ranking's order: the greater bound first, of equal
/// bounds the earlier place.
#[derive(Clone, Copy, PartialEq)]
struct Ranked(f64, usize);

impl Eq for Ranked {}

impl PartialOrd for Ranked {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Ranked {
    fn cmp(&self, other: &Self) -> Ordering {
        other.0.total_cmp(&self.0).then(self.1.cmp(&other.1))
    }
}

/// The sources every façade sums: of `candidates` the [`FACADE_SOURCE_LIMIT`] of greatest bound at
/// any façade, which falls with distance, so at the nearest one, under `gain`; each with that
/// bound's Lden energy and its place in `candidates`, greatest first, equal bounds in their order
/// there. No bound exceeds its ceiling at the gap between the candidate's box and the façades'
/// (`physics::bound::received_lden_ceiling`): candidates are bounded in their ceilings' order until
/// the next ceiling falls below the limit's last bound, as no later one can rank (a city building
/// bounds a few hundred of its tens of thousands).
pub fn facade_sources(
    facades: &[Facade],
    candidates: &[&Candidate],
    attributes: &Attributes,
    gain: &ReceiverBound,
) -> Vec<(f64, usize)> {
    let bound = |index: usize| {
        let candidate = candidates[index];
        let nearest = (facades.iter())
            .map(|facade| candidate.distance_from(facade.position))
            .fold(f64::INFINITY, f64::min);
        let source = &attributes[candidate.attribute];
        Ranked(
            lden_energy(&candidate.bound_at_distance(nearest, source, gain)),
            index,
        )
    };
    let (mut low, mut high) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for facade in facades {
        for axis in 0..2 {
            low[axis] = low[axis].min(facade.position[axis]);
            high[axis] = high[axis].max(facade.position[axis]);
        }
    }
    let largest_gain = gain.gain.iter().copied().fold(0.0, f64::max);
    let mut by_ceiling: BinaryHeap<Reverse<Ranked>> = (candidates.iter().enumerate())
        .map(|(index, candidate)| {
            let [a, b] = candidate.ends_m;
            let gap = |axis: usize| {
                (low[axis] - a[axis].max(b[axis]))
                    .max(a[axis].min(b[axis]) - high[axis])
                    .max(0.0)
            };
            let power = attributes[candidate.attribute].lden_power;
            let distance = gap(0).hypot(gap(1));
            let ceiling = received_lden_ceiling(power, candidate.spread(), distance, largest_gain);
            Reverse(Ranked(ceiling, index))
        })
        .collect();
    // The ranked so far, the one ranking last on top.
    let mut kept: BinaryHeap<Ranked> = BinaryHeap::with_capacity(FACADE_SOURCE_LIMIT + 1);
    while let Some(Reverse(Ranked(ceiling, index))) = by_ceiling.pop() {
        if kept.len() == FACADE_SOURCE_LIMIT && kept.peek().is_some_and(|last| ceiling < last.0) {
            break;
        }
        kept.push(bound(index));
        if kept.len() > FACADE_SOURCE_LIMIT {
            kept.pop();
        }
    }
    (kept.into_sorted_vec().into_iter())
        .map(|Ranked(bound, index)| (bound, index))
        .collect()
}

/// What `facade` receives of the ranked `sources` (places in `candidates`), as Lden energy.
#[allow(clippy::too_many_arguments)]
pub fn facade_level(
    facade: &Facade,
    sources: &[usize],
    candidates: &[&Candidate],
    attributes: &Attributes,
    ground: &Ground,
    obstacles: &Scene<'_>,
    weather: PlaceWeather,
    scratch: &mut Scratch,
) -> Result<f64, String> {
    let receiver = Receiver {
        ground,
        obstacles,
        position: facade.position,
        altitude_m: facade.altitude_m,
        weather,
        reflection_db: facade.reflection_db,
        own_footprint: 0,
    };
    let mut total = [0.0; PERIODS];
    for &index in sources {
        let mut candidate = Candidate::clone(candidates[index]);
        candidate.distance_m = candidate.distance_from(facade.position);
        let source = &attributes[candidate.attribute];
        let received = received_energy(&receiver, &candidate, source, scratch)?;
        for (sum, value) in total.iter_mut().zip(received) {
            *sum += value;
        }
    }
    Ok(lden_energy(&total))
}

/// The rule's choice among the façades' `levels`: the first of the loudest.
pub fn loudest(levels: &[f64]) -> usize {
    let mut winner = 0;
    for (index, level) in levels.iter().enumerate() {
        if *level > levels[winner] {
            winner = index;
        }
    }
    winner
}

/// The loudest façade of `footprint` over the candidates collected so far (all layers).
#[allow(clippy::too_many_arguments)]
pub fn loudest_facade(
    footprint: &Footprint,
    receivers: &[FacadeReceiver],
    candidates: &[&Candidate],
    attributes: &Attributes,
    ground: &Ground,
    obstacles: &Scene<'_>,
    weather: PlaceWeather,
) -> Result<Facade, String> {
    let facades = facades(footprint, receivers, ground, obstacles)?;
    let gain = ranking_gain(&facades, &weather);
    let sources: Vec<usize> = (facade_sources(&facades, candidates, attributes, &gain).into_iter())
        .map(|(_, index)| index)
        .collect();
    let levels = (facades.par_iter())
        .map_init(Scratch::default, |scratch, facade| {
            facade_level(
                facade, &sources, candidates, attributes, ground, obstacles, weather, scratch,
            )
        })
        .collect::<Result<Vec<f64>, String>>()?;
    Ok(facades[loudest(&levels)])
}

#[cfg(test)]
#[path = "building_tests.rs"]
mod tests;

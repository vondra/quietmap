//! A click inside a building answers at its loudest façade, without indoor attenuation (PLAN-z13
//! KEEP, dev4 7c533e17): of the CNOSSOS-EU 2.8 façade receivers, the one where the sources with the
//! greatest bound at any façade sum to the highest Lden wins (ties to the first in canonical order);
//! the ring loop then answers there with every source. At a façade the own building still screens
//! what stands behind it, and its footprint does not count for the reflection bonus (2.8).

use crate::answer::RECEIVER_HEIGHT_M;
use crate::candidates::{Attributes, Candidate};
use crate::evaluate::{Receiver, Scratch, received_energy};
use crate::obstacles::{FacadeReceiver, Footprint, Scene};
use crate::scene::Ground;
use physics::bands::PERIODS;
use physics::bands::lden_energy;
use physics::bound::receiver_bound;
use physics::weather::PlaceWeather;
use rayon::prelude::*;

/// Sources evaluated at every façade to choose the loudest (dev4 `FACADE_SOURCE_LIMIT`).
pub const FACADE_SOURCE_LIMIT: usize = 11;

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

/// A façade receiver standing at `receiver`, as the evaluation sees it.
fn facade_at(
    receiver: &FacadeReceiver,
    index: usize,
    footprint: &Footprint,
    ground: &Ground<'_>,
    obstacles: &Scene<'_>,
) -> Result<Facade, String> {
    Ok(Facade {
        position: receiver.position,
        outward_bearing_deg: receiver.outward_bearing_deg,
        index,
        altitude_m: ground.at(receiver.position)?.height_m + RECEIVER_HEIGHT_M,
        reflection_db: obstacles.reflection_db(receiver.position, Some(footprint.id))?,
    })
}

/// The loudest façade of `footprint` over the candidates collected so far (all layers).
#[allow(clippy::too_many_arguments)]
pub fn loudest_facade(
    footprint: &Footprint,
    receivers: &[FacadeReceiver],
    candidates: &[&Candidate],
    attributes: &Attributes,
    ground: &Ground<'_>,
    obstacles: &Scene<'_>,
    weather: PlaceWeather,
) -> Result<Facade, String> {
    let facades = receivers
        .iter()
        .enumerate()
        .map(|(index, receiver)| facade_at(receiver, index, footprint, ground, obstacles))
        .collect::<Result<Vec<_>, String>>()?;
    // Rank by the greatest bound at any façade: the bound falls with distance, so it is the
    // bound at the nearest receiver, with the largest reflection bonus of any.
    let positions: Vec<[f64; 2]> = facades.iter().map(|facade| facade.position).collect();
    let largest_reflection = facades
        .iter()
        .map(|facade| facade.reflection_db)
        .fold(0.0, f64::max);
    let ranking_gain = receiver_bound(
        weather.favourable.maximum(),
        largest_reflection,
        weather.alpha_db_per_km,
    );
    let mut ranked: Vec<(f64, usize)> = candidates
        .par_iter()
        .enumerate()
        .map(|(index, candidate)| {
            let nearest = positions
                .iter()
                .map(|&position| candidate.distance_from(position))
                .fold(f64::INFINITY, f64::min);
            let source = &attributes[candidate.attribute];
            let bound = candidate.bound_at_distance(nearest, source, &ranking_gain);
            (lden_energy(&bound), index)
        })
        .collect();
    if ranked.len() > FACADE_SOURCE_LIMIT {
        ranked.select_nth_unstable_by(FACADE_SOURCE_LIMIT, |a, b| b.0.total_cmp(&a.0));
        ranked.truncate(FACADE_SOURCE_LIMIT);
    }
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let levels = facades
        .par_iter()
        .map_init(Scratch::default, |scratch, facade| {
            let receiver = Receiver {
                ground,
                obstacles,
                position: facade.position,
                altitude_m: facade.altitude_m,
                weather,
                reflection_db: facade.reflection_db,
            };
            let mut total = [0.0; PERIODS];
            for &(_, index) in &ranked {
                let mut candidate = Candidate::clone(candidates[index]);
                let source = &attributes[candidate.attribute];
                candidate.bound_at(facade.position, source, &ranking_gain);
                let received = received_energy(&receiver, &candidate, source, scratch)?;
                for (sum, value) in total.iter_mut().zip(received) {
                    *sum += value;
                }
            }
            Ok(lden_energy(&total))
        })
        .collect::<Result<Vec<f64>, String>>()?;
    let mut winner = 0;
    for (index, level) in levels.iter().enumerate() {
        if *level > levels[winner] {
            winner = index;
        }
    }
    Ok(facades[winner])
}

//! A click inside a building answers at its loudest façade, without indoor attenuation (as dev4
//! 7c533e17): of the CNOSSOS-EU 2.8 façade receivers, the one where the sources with the greatest
//! bound at any façade sum to the highest Lden wins (ties to the first in canonical order); the
//! ring loop then answers there with every source. At a façade the own building still screens what
//! stands behind it, and its footprint does not count for the reflection bonus (2.8).

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

#[path = "facade_sources.rs"]
mod sources;

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

/// A façade receiver standing at `receiver`, as the evaluation sees it.
fn facade_at(
    receiver: &FacadeReceiver,
    index: usize,
    footprint: &Footprint,
    ground: &Ground,
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
    ground: &Ground,
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
    let largest_reflection = facades
        .iter()
        .map(|facade| facade.reflection_db)
        .fold(0.0, f64::max);
    let ranking_gain = receiver_bound(
        weather.favourable.maximum(),
        largest_reflection,
        weather.alpha_db_per_km,
    );
    let ranked = sources::facade_sources(&facades, candidates, attributes, &ranking_gain);
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
                own_footprint: 0,
            };
            let mut total = [0.0; PERIODS];
            for &(_, index) in &ranked {
                let mut candidate = Candidate::clone(candidates[index]);
                let source = &attributes[candidate.attribute];
                // Ray evaluation reads the distance, never the candidate's ranking bound or order.
                candidate.distance_m = candidate.distance_from(facade.position);
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

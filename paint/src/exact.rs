//! The exact evaluation of (point, source) pairs, the popup's physics: the points a square is
//! evaluated at, and every pair's whole Lden energy computed here or by a card ([`Batch`]).

use crate::square::Square;
use physics::bands::{PERIODS, energy, lden_energy};
use physics::bound::{ReceiverBound, receiver_bound};
use physics::weather::PlaceWeather;
use popup::evaluate::{Receiver, Scratch, period_sums, received_bands};
use rayon::prelude::*;
use tiles::sources::Layer;

pub const LAYERS: usize = Layer::ALL.len();

/// A point evaluated: where it stands and what its rays share.
pub struct Point {
    pub position: [f64; 2],
    pub altitude_m: f64,
    pub weather: PlaceWeather,
    /// The building it stands in (0: none), whose walls do not screen it.
    pub own_footprint: u64,
    /// Its receiver reflection (dB), which every ground source's energy there takes.
    pub reflection_db: f64,
    /// What decides which sources count there (`popup::candidates::counts`).
    pub bound: ReceiverBound,
}

impl Point {
    /// The point at `position` (frame metres), the receiver height above its ground, in the
    /// building it may stand in.
    pub fn at(square: &Square, position: [f64; 2]) -> Result<Self, String> {
        let (lat, lon) = square.frame.to_mercator(position).to_degrees();
        let weather = square.weather.place(lat, lon);
        let own_footprint = square.obstacles.enclosing_building_id(position)?;
        let reflection_db = square.obstacles.reflection_db(position, own_footprint)?;
        Ok(Point {
            position,
            altitude_m: square.ground.at(position)?.height_m + popup::answer::RECEIVER_HEIGHT_M,
            weather,
            own_footprint: own_footprint.unwrap_or(0),
            reflection_db,
            bound: receiver_bound(
                weather.favourable.maximum(),
                reflection_db,
                weather.alpha_db_per_km,
            ),
        })
    }

    /// The receiver this point is for the popup's physics; its reflection is applied after.
    pub fn receiver<'s>(&self, square: &'s Square) -> Receiver<'s, 's> {
        Receiver {
            ground: &square.ground,
            obstacles: &square.obstacles,
            position: self.position,
            altitude_m: self.altitude_m,
            weather: self.weather,
            reflection_db: 0.0,
            own_footprint: self.own_footprint,
        }
    }

    /// The reflection's linear factor.
    pub fn reflection(&self) -> f64 {
        energy(self.reflection_db)
    }
}

/// Exact evaluations of many pairs at once (a card): the energy per period candidate `c` delivers
/// at point `p`, for every `(p, c)` of `pairs`.
pub trait Batch: Sync {
    fn evaluate(
        &self,
        points: &[Point],
        pairs: &[(u32, u32)],
    ) -> Result<Vec<[f64; PERIODS]>, String>;
}

/// The energy per period candidate `index` delivers at `point` on the CPU.
pub fn whole_energy(
    square: &Square,
    point: &Point,
    index: u32,
    scratch: &mut Scratch,
) -> Result<[f64; PERIODS], String> {
    let mut candidate = square.candidates[index as usize].clone();
    candidate.distance_m = candidate.distance_from(point.position);
    let received = received_bands(
        &point.receiver(square),
        &candidate,
        &square.attributes[candidate.attribute],
        scratch,
    )?;
    Ok(period_sums(&received.bands))
}

/// Every pair's energy per period: on the card when there is one, else on the CPU.
pub fn evaluate_periods(
    square: &Square,
    points: &[Point],
    pairs: &[(u32, u32)],
    batch: Option<&dyn Batch>,
) -> Result<Vec<[f64; PERIODS]>, String> {
    match batch {
        Some(batch) => batch.evaluate(points, pairs),
        None => pairs
            .par_iter()
            .map_init(Scratch::default, |scratch, &(at, index)| {
                whole_energy(square, &points[at as usize], index, scratch)
            })
            .collect(),
    }
}

/// Every pair's Lden energy.
pub fn evaluate(
    square: &Square,
    points: &[Point],
    pairs: &[(u32, u32)],
    batch: Option<&dyn Batch>,
) -> Result<Vec<f64>, String> {
    Ok(evaluate_periods(square, points, pairs, batch)?
        .iter()
        .map(lden_energy)
        .collect())
}

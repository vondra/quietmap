//! The exact evaluation of (point, source) pairs, the popup's physics: the points a square is
//! evaluated at, every pair's energy in both weather states and every point's octave bands per
//! layer, computed here or by a card ([`Batch`]).

use crate::square::Square;
use physics::bands::{BANDS, PERIODS, energy, lden_energy};
use physics::bound::{ReceiverBound, receiver_bound};
use physics::weather::PlaceWeather;
use popup::evaluate::{Bands, Receiver, Scratch, period_sums, received_bands};
use rayon::prelude::*;
use tiles::sources::Layer;

pub const LAYERS: usize = Layer::ALL.len();

/// A point evaluated: where it stands and what its rays share. Its weather is the table's at its
/// place, taken when a ray is evaluated here ([`Point::weather`]): a card takes its own, and a
/// square's million points stay small.
pub struct Point {
    pub position: [f64; 2],
    pub altitude_m: f64,
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
        let weather = weather_at(square, position);
        let own_footprint = square.obstacles.enclosing_building_id(position)?;
        let reflection_db = square.obstacles.reflection_db(position, own_footprint)?;
        Ok(Point {
            position,
            altitude_m: square.ground.at(position)?.height_m + popup::answer::RECEIVER_HEIGHT_M,
            own_footprint: own_footprint.unwrap_or(0),
            reflection_db,
            bound: receiver_bound(
                weather.favourable.maximum(),
                reflection_db,
                weather.alpha_db_per_km,
            ),
        })
    }

    /// The weather at this point.
    pub fn weather(&self, square: &Square) -> PlaceWeather {
        weather_at(square, self.position)
    }

    /// The receiver this point is for the popup's physics; its reflection is applied after.
    pub fn receiver<'s>(&self, square: &'s Square) -> Receiver<'s, 's> {
        Receiver {
            ground: &square.ground,
            obstacles: &square.obstacles,
            position: self.position,
            altitude_m: self.altitude_m,
            weather: self.weather(square),
            reflection_db: 0.0,
            own_footprint: self.own_footprint,
        }
    }

    /// The reflection's linear factor.
    pub fn reflection(&self) -> f64 {
        energy(self.reflection_db)
    }
}

/// The weather table's at `position` (frame metres).
fn weather_at(square: &Square, position: [f64; 2]) -> PlaceWeather {
    let (lat, lon) = square.frame.to_mercator(position).to_degrees();
    square.weather.place(lat, lon)
}

/// What a pair delivers per period: mixed by its direction's favourable share (its received bands
/// summed), and in calm air and downwind alone (`popup::evaluate::Path::states`), which the
/// popup's weather states mix.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PairEnergy {
    pub mixed: [f64; PERIODS],
    pub states: [[f64; PERIODS]; 2],
}

/// A point's received octave bands per layer and period, mixed: the spectrum its loudness takes.
pub type PointBands = [Bands; LAYERS];

/// Every pair's energy, in order, and every point's bands.
pub struct Evaluated {
    pub pairs: Vec<PairEnergy>,
    pub bands: Vec<PointBands>,
}

/// Exact evaluations of many pairs at once (a card): what candidate `c` delivers at point `p`, for
/// every `(p, c)` of `pairs`.
pub trait Batch: Sync {
    fn evaluate(&self, points: &[Point], pairs: &[(u32, u32)]) -> Result<Evaluated, String>;
    /// Each pixel's Nden per layer and in total (sone) from its packed sound
    /// (`crate::sone::PixelSound::pack`): `input` all the pixels' floats, `offsets` where each
    /// pixel's begin.
    fn loudness(&self, input: &[f32], offsets: &[u64]) -> Result<Vec<[f32; LAYERS + 1]>, String>;
}

/// What candidate `index` delivers at `point` on the CPU, and its bands.
pub fn whole_energy(
    square: &Square,
    point: &Point,
    index: u32,
    scratch: &mut Scratch,
) -> Result<(PairEnergy, Bands), String> {
    let mut candidate = square.candidates[index as usize].clone();
    candidate.distance_m = candidate.distance_from(point.position);
    let received = received_bands(
        &point.receiver(square),
        &candidate,
        &square.attributes[candidate.attribute],
        scratch,
    )?;
    let energy = PairEnergy {
        mixed: period_sums(&received.bands),
        states: received.path.states,
    };
    Ok((energy, received.bands))
}

/// Adds a pair's `bands` to its point's, in its candidate's layer.
pub fn add_bands(square: &Square, point: &mut PointBands, index: u32, bands: &Bands) {
    let layer = &mut point[square.candidates[index as usize].layer as usize];
    for (total, period) in layer.iter_mut().zip(bands) {
        for (sum, value) in total.iter_mut().zip(period) {
            *sum += value;
        }
    }
}

/// Every pair's energy and every point's bands: on the card when there is one, else on the CPU.
pub fn evaluate_all(
    square: &Square,
    points: &[Point],
    pairs: &[(u32, u32)],
    batch: Option<&dyn Batch>,
) -> Result<Evaluated, String> {
    // A pass without pairs (a square of flights alone) asks the card nothing: its bands' buffer
    // would be allocated and read back for nothing (0.5 s of the Sahara's 0.6 s squares).
    if let Some(batch) = batch.filter(|_| !pairs.is_empty()) {
        return batch.evaluate(points, pairs);
    }
    let each: Vec<(PairEnergy, Bands)> = pairs
        .par_iter()
        .map_init(Scratch::default, |scratch, &(at, index)| {
            whole_energy(square, &points[at as usize], index, scratch)
        })
        .collect::<Result<_, String>>()?;
    let mut bands = vec![[[[0.0; BANDS]; PERIODS]; LAYERS]; points.len()];
    for (&(at, index), (_, pair)) in pairs.iter().zip(&each) {
        add_bands(square, &mut bands[at as usize], index, pair);
    }
    Ok(Evaluated {
        pairs: each.into_iter().map(|(energy, _)| energy).collect(),
        bands,
    })
}

/// Every pair's Lden energy.
pub fn evaluate(
    square: &Square,
    points: &[Point],
    pairs: &[(u32, u32)],
    batch: Option<&dyn Batch>,
) -> Result<Vec<f64>, String> {
    Ok(evaluate_all(square, points, pairs, batch)?
        .pairs
        .iter()
        .map(|energy| lden_energy(&energy.mixed))
        .collect())
}

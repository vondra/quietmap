//! Exact source energies and importance-weighted receiver spectra through shared popup physics.
//!
//! The exact evaluation of (point, source) pairs, the popup's physics: the points a square is
//! evaluated at, every pair's energy in both weather states and every point's octave bands per
//! layer, computed here or by a card ([`Batch`]).
//! Importance weights apply to sampled pairs' spectra, not the energies used for confidence.

use crate::flight_boxes::{FlightBoxes, FlightRequest};
use crate::flights::FlightSums;
use crate::square::Square;
use physics::bands::{BANDS, PERIODS, energy, lden_energy};
use physics::bound::{ReceiverBound, receiver_bound};
use physics::weather::PlaceWeather;
use popup::evaluate::{Bands, Receiver, Scratch, period_sums, received_bands};
use rayon::prelude::*;
use tiles::sources::Layer;

pub const LAYERS: usize = Layer::ALL.len();

/// Pixels in a prepared loudness chunk: its card mixtures occupy about 124 MB.
pub const PIXELS_PER_LOUDNESS_BATCH: usize = 8_192;

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
        let own_footprint = square.obstacles.enclosing_building_id(position)?;
        let reflection_db = square.obstacles.reflection_db(position, own_footprint)?;
        Self::with_receiver_properties(square, position, own_footprint.unwrap_or(0), reflection_db)
    }

    /// Building `footprint`'s façade receiver at `position`, as a click inside the building is
    /// answered (`popup::building`): outdoors, its reflection without the façade's own.
    pub fn on_facade(square: &Square, position: [f64; 2], footprint: u64) -> Result<Self, String> {
        let reflection_db = square.obstacles.reflection_db(position, Some(footprint))?;
        Self::with_receiver_properties(square, position, 0, reflection_db)
    }

    /// A point with a known footprint and reflection, sharing the terrain and weather construction.
    pub fn with_receiver_properties(
        square: &Square,
        position: [f64; 2],
        own_footprint: u64,
        reflection_db: f64,
    ) -> Result<Self, String> {
        let weather = weather_at(square, position);
        Ok(Point {
            position,
            altitude_m: square.ground.at(position)?.height_m + popup::answer::RECEIVER_HEIGHT_M,
            own_footprint,
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

/// A source at a receiver, with its spectrum's importance-sampling weight; its energy stays unweighted for variance.
#[derive(Clone, Copy)]
pub struct Pair {
    pub point: u32,
    pub candidate: u32,
    pub band_weight: f64,
}

impl Pair {
    pub fn new(point: u32, candidate: u32) -> Self {
        Self {
            point,
            candidate,
            band_weight: 1.0,
        }
    }
}

/// Exact evaluations of many pairs at once (a card): what candidate `c` delivers at point `p`, for
/// every `(p, c)` of `pairs`.
pub trait Batch: Sync {
    fn evaluate(&self, points: &[Point], pairs: &[Pair]) -> Result<Evaluated, String>;
    /// One chunk's Nden per layer and in total (sone), at most [`PIXELS_PER_LOUDNESS_BATCH`] points,
    /// from its packed sound
    /// (`crate::sone::PixelSound::pack`): `input` all the pixels' floats, `offsets` where each
    /// pixel's begin.
    fn loudness(&self, input: &[f32], offsets: &[u64]) -> Result<Vec<[f32; LAYERS + 1]>, String>;
    /// The flights of each request (`crate::flight_boxes::FlightBoxes::sums` on the cores).
    fn flight_sums(
        &self,
        square: &Square,
        boxes: &FlightBoxes,
        requests: &[FlightRequest],
    ) -> Result<Vec<[FlightSums; 2]>, String>;
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
pub fn add_bands(square: &Square, point: &mut PointBands, index: u32, bands: &Bands, weight: f64) {
    let layer = &mut point[square.candidates[index as usize].layer as usize];
    for (total, period) in layer.iter_mut().zip(bands) {
        for (sum, value) in total.iter_mut().zip(period) {
            *sum += weight * value;
        }
    }
}

/// Every pair's energy and every point's bands: on the card when there is one, else on the CPU.
pub fn evaluate_all(
    square: &Square,
    points: &[Point],
    pairs: &[Pair],
    batch: Option<&dyn Batch>,
) -> Result<Evaluated, String> {
    // A pass without pairs (a square of flights alone) asks the card nothing: its bands' buffer
    // would be allocated and read back for nothing.
    if let Some(batch) = batch.filter(|_| !pairs.is_empty()) {
        return batch.evaluate(points, pairs);
    }
    let each: Vec<(PairEnergy, Bands)> = pairs
        .par_iter()
        .map_init(Scratch::default, |scratch, pair| {
            whole_energy(
                square,
                &points[pair.point as usize],
                pair.candidate,
                scratch,
            )
        })
        .collect::<Result<_, String>>()?;
    let mut bands = vec![[[[0.0; BANDS]; PERIODS]; LAYERS]; points.len()];
    for (pair, (_, spectrum)) in pairs.iter().zip(&each) {
        add_bands(
            square,
            &mut bands[pair.point as usize],
            pair.candidate,
            spectrum,
            pair.band_weight,
        );
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
    pairs: &[Pair],
    batch: Option<&dyn Batch>,
) -> Result<Vec<f64>, String> {
    Ok(evaluate_all(square, points, pairs, batch)?
        .pairs
        .iter()
        .map(|energy| lden_energy(&energy.mixed))
        .collect())
}

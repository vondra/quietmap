//! The three levels a source's energy at a point is split into by its distance: near (evaluated at
//! every pixel), mid (at the corners of the pixel blocks) and far (at the corners of the coarse
//! cells), by smooth weights that sum to one, so a level is interpolated only where its share
//! changes slowly over the corners' spacing. At each point a level's sources are evaluated from
//! the largest bound, as the popup's selection, until the bounds left out stay under the
//! tolerance of the energy known there; past a number of them the rest is sampled in proportion
//! to the bounds (Hansen-Hurwitz).

use crate::square::Square;
use physics::bands::{PERIODS, lden_energy};
use physics::weather::PlaceWeather;
use popup::candidates::GROUND_REACH_M;
use popup::evaluate::{Receiver, Scratch, period_sums, received_bands};
use tiles::sources::Layer;

pub const LAYERS: usize = Layer::ALL.len();

/// The distances (m) where a share ramps: near falls from 1 at `near[0]` to 0 at `near[1]`, far
/// rises from 0 at `far[0]` to 1 at `far[1]`, mid is the rest.
#[derive(Clone, Copy)]
pub struct Levels {
    pub near: [f64; 2],
    pub far: [f64; 2],
}

fn smooth(distance_m: f64, [from, to]: [f64; 2]) -> f64 {
    let t = ((distance_m - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Levels {
    pub fn near(&self, distance_m: f64) -> f64 {
        1.0 - smooth(distance_m, self.near)
    }

    pub fn mid(&self, distance_m: f64) -> f64 {
        smooth(distance_m, self.near) - smooth(distance_m, self.far)
    }

    pub fn far(&self, distance_m: f64) -> f64 {
        smooth(distance_m, self.far)
    }
}

/// How hard one level's evaluation works at one point.
#[derive(Clone, Copy)]
pub struct Rule {
    /// The bounds left out may reach this fraction of the energy known at the point.
    pub tolerance: f64,
    /// Sources evaluated loudest first before the rest is sampled.
    pub proven: usize,
    /// Draws of the sample.
    pub sample: usize,
}

/// A point evaluated: where it stands and what its rays share.
pub struct Point {
    pub position: [f64; 2],
    pub altitude_m: f64,
    pub weather: PlaceWeather,
    /// The building it stands in (0: none), whose walls do not screen it.
    pub own_footprint: u64,
}

impl Point {
    /// The point at `position` (frame metres), the receiver height above its ground.
    pub fn at(square: &Square, position: [f64; 2]) -> Result<Self, String> {
        let (lat, lon) = square.frame.to_mercator(position).to_degrees();
        Ok(Point {
            position,
            altitude_m: square.ground.at(position)?.height_m + popup::answer::RECEIVER_HEIGHT_M,
            weather: square.release.weather.place(lat, lon),
            own_footprint: square
                .obstacles
                .enclosing_building_id(position)?
                .unwrap_or(0),
        })
    }
}

/// Per-thread buffers.
#[derive(Default)]
pub struct Work {
    scratch: Scratch,
    lists: [Vec<(f64, u32)>; LAYERS],
    cumulative: Vec<f64>,
    drawn: Vec<(u32, f64)>,
    /// The last call's sources evaluated with certainty: their weighted energy and index.
    pub evaluated: Vec<(f64, u32)>,
}

/// The Lden energy candidate `index` delivers at `point`, its share `weight` of its distance.
pub fn energy_of(
    square: &Square,
    point: &Point,
    index: u32,
    weight: &dyn Fn(f64) -> f64,
    work: &mut Work,
) -> Result<f64, String> {
    let receiver = Receiver {
        ground: &square.ground,
        obstacles: &square.obstacles,
        position: point.position,
        altitude_m: point.altitude_m,
        weather: point.weather,
        reflection_db: 0.0,
        own_footprint: point.own_footprint,
    };
    let mut candidate = square.candidates[index as usize].clone();
    candidate.distance_m = candidate.distance_from(point.position);
    let share = weight(candidate.distance_m);
    if share <= 0.0 || candidate.distance_m > GROUND_REACH_M {
        return Ok(0.0);
    }
    let received = received_bands(
        &receiver,
        &candidate,
        &square.attributes[candidate.attribute],
        &mut work.scratch,
    )?;
    let energy: [f64; PERIODS] = period_sums(&received.bands);
    Ok(share * lden_energy(&energy))
}

/// SplitMix64.
struct SplitMix(u64);

impl SplitMix {
    fn unit(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Each layer's Lden energy at `point` of the candidates `indices`, each weighted by
/// `weight(distance)`; `known` is the energy the point has from the other levels, which the
/// tolerance counts.
#[allow(clippy::too_many_arguments)]
pub fn energies(
    square: &Square,
    point: &Point,
    indices: &mut dyn Iterator<Item = u32>,
    weight: &dyn Fn(f64) -> f64,
    known: &[f64; LAYERS],
    rule: &Rule,
    seed: u64,
    work: &mut Work,
) -> Result<[f64; LAYERS], String> {
    for list in &mut work.lists {
        list.clear();
    }
    work.evaluated.clear();
    for index in indices {
        let candidate = &square.candidates[index as usize];
        let distance = candidate.distance_from(point.position);
        if distance > GROUND_REACH_M {
            continue;
        }
        let share = weight(distance);
        if share <= 0.0 {
            continue;
        }
        let bound = share * square.bound(index as usize, distance);
        if bound > 0.0 {
            work.lists[candidate.layer as usize].push((bound, index));
        }
    }
    let mut result = [0.0; LAYERS];
    for layer in 0..LAYERS {
        let mut list = std::mem::take(&mut work.lists[layer]);
        if list.is_empty() {
            work.lists[layer] = list;
            continue;
        }
        let evaluate = |index: u32, work: &mut Work| energy_of(square, point, index, weight, work);
        // The loudest bounds first: only the head is sorted.
        let head = rule.proven.min(list.len());
        if head < list.len() {
            list.select_nth_unstable_by(head, |a, b| b.0.total_cmp(&a.0));
        }
        list[..head].sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut remaining: f64 = list.iter().map(|item| item.0).sum();
        let mut energy = 0.0;
        let mut taken = 0;
        while taken < head && remaining > rule.tolerance * (energy + known[layer]) {
            let value = evaluate(list[taken].1, work)?;
            work.evaluated.push((value, list[taken].1));
            energy += value;
            remaining -= list[taken].0;
            taken += 1;
        }
        let rest = &list[taken..];
        if !rest.is_empty() && remaining > rule.tolerance * (energy + known[layer]) {
            // Hansen-Hurwitz: draws with probability bound / total, each read as energy / p.
            work.cumulative.clear();
            let mut total = 0.0;
            for item in rest {
                total += item.0;
                work.cumulative.push(total);
            }
            let mut random = SplitMix(seed ^ (layer as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15));
            work.drawn.clear();
            for _ in 0..rule.sample {
                let target = random.unit() * total;
                let at = work
                    .cumulative
                    .partition_point(|&sum| sum <= target)
                    .min(rest.len() - 1);
                work.drawn.push((at as u32, 0.0));
            }
            work.drawn.sort_unstable_by_key(|draw| draw.0);
            let mut estimate = 0.0;
            let mut last: Option<(u32, f64)> = None;
            for draw in 0..work.drawn.len() {
                let at = work.drawn[draw].0;
                let value = match last {
                    Some((previous, value)) if previous == at => value,
                    _ => {
                        let (bound, index) = rest[at as usize];
                        let value = evaluate(index, work)? * total / bound;
                        last = Some((at, value));
                        value
                    }
                };
                estimate += value;
            }
            energy += estimate / rule.sample as f64;
        }
        result[layer] = energy;
        work.lists[layer] = list;
    }
    Ok(result)
}

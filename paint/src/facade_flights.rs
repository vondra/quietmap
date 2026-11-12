//! The flights at the façades: each façade sums the square's boxes tier after tier, until the
//! ceilings of those left prove they cannot change its sums by more than 0.1 dB.
//!
//! Every box of the square takes its ceiling over the area of all the façades
//! (`popup::aircraft::ceilings`): the most it can bring to any of them, screened or not, for each
//! period's energy and the flights' lambda's weights (a fine box gives energy, a far one the
//! energy where it is heard and the lambda's weights wherever its tile is, or the fine one where
//! its tile has no far file: `crate::flights::heard_parts`). Ranked by it, the boxes fall into
//! tiers at [`TIER_SHARES`]; every façade sums the first, and one whose sums the tiers left could
//! still change by more than [`LEFT_OUT_AT_MOST`] by their ceilings sums the next. What a façade
//! leaves out is thus at most that share of each of its sums (the rolls, summed whole as a click
//! sums them, among them), in exact arithmetic: the card's single precision adds its own rounding
//! (`qm-paint-gpu check-flights`: within 1e-4 dB), and a last bit of it can tip a façade into
//! another tier, by no more than that share.

use crate::exact::{Batch, Point};
use crate::flight_boxes::{
    FlightBoxes, FlightRequest, FlightWeight, flight_sums, flights_at_points,
};
use crate::flights::{FlightSums, rolls_at};
use crate::square::Square;
use physics::bands::{PERIODS, lden_energy};
use popup::aircraft::boxes::{AIRCRAFT_REACH_M, ReceiverArea};
use popup::aircraft::ceilings::{CeilingArea, box_flight_ceiling};
use popup::aircraft::horizons::Horizons;
use rayon::prelude::*;

/// The share of each of a façade's sums the boxes it leaves out can carry at most: 0.1 dB, a fifth
/// of the map's half-phon step.
const LEFT_OUT_AT_MOST: f64 = 0.023;

/// The cumulative shares of the boxes, ranked by ceiling, at which the tiers end. A façade in New
/// York or a town sums half of them before the ceilings left are within 0.1 dB of its sums, one in
/// Prague mostly a quarter; tiers of a sixteenth and an eighth stopped none of Dobříš's 14,416
/// façades and cost their passes (evidence 2026-11-11 facade-1, session C).
const TIER_SHARES: [f64; 3] = [1.0 / 4.0, 1.0 / 2.0, 1.0];

/// Fewer façades than this sum every box: a box's ceiling costs about a microsecond of a core and
/// its sum at a façade two nanoseconds of the card, of which the tiers save about half.
const TIERED_FROM_FACADES: usize = 512;

/// Façades whose horizons the cores keep through the tiers at once: about 42 KB each, and 15 KB
/// more packed for the card while a tier is summed.
const FACADES_AT_ONCE: usize = 4_096;

/// What a tier's ceilings bound, per period: the energy heard, the lambda's energy, and its
/// energy times lambda.
type Ceilings = [[f64; PERIODS]; 3];

/// The square's boxes in tiers: per tier, its boxes (band 0 of each tile's files) and the ceilings
/// of the boxes after it.
struct Tiers {
    boxes: Vec<FlightBoxes>,
    left: Vec<Ceilings>,
}

/// Every flight at each of `points` (the façades), their boxes in tiers when they are many.
pub(crate) fn flights_at_facades(
    square: &Square,
    boxes: &FlightBoxes,
    points: &[Point],
    batch: Option<&dyn Batch>,
) -> Result<Vec<FlightSums>, String> {
    if points.len() < TIERED_FROM_FACADES || !boxes.occupied.contains(&true) {
        return flights_at_points(square, boxes, points, batch);
    }
    let started = std::time::Instant::now();
    let area = ReceiverArea::from_points(points.iter().map(|p| (p.position, p.altitude_m)));
    let tiers = tiers(square, boxes, &CeilingArea::of(area));
    let ceilings_s = started.elapsed().as_secs_f64();
    let (mut summing, mut summed_s) = ([0usize; TIER_SHARES.len()], [0.0f64; TIER_SHARES.len()]);
    let mut flights = Vec::with_capacity(points.len());
    for points in points.chunks(FACADES_AT_ONCE) {
        let horizons: Vec<Horizons> = (points.par_iter())
            .map(|point| {
                Horizons::build(
                    &square.ground,
                    &square.obstacles,
                    point.position,
                    point.altitude_m,
                    point.own_footprint,
                )
            })
            .collect::<Result<_, String>>()?;
        // The rolls first, which the stop counts too.
        let mut sums: Vec<FlightSums> = (points.par_iter().zip(&horizons))
            .map(|(point, horizons)| rolls_at(square, point, Some(horizons)))
            .collect::<Result<_, String>>()?;
        let mut pending: Vec<usize> = (0..points.len()).collect();
        let tiered = tiers
            .boxes
            .iter()
            .zip(&tiers.left)
            .zip(summing.iter_mut().zip(&mut summed_s));
        for ((tier, left), (summing, summed_s)) in tiered {
            if pending.is_empty() {
                break;
            }
            let tier_started = std::time::Instant::now();
            *summing += pending.len();
            if tier.occupied[0] {
                let requests: Vec<FlightRequest> = (pending.iter())
                    .map(|&k| FlightRequest {
                        point: &points[k],
                        weights: [Some(FlightWeight::One), None],
                        within_m: AIRCRAFT_REACH_M,
                        horizons: Some(&horizons[k]),
                    })
                    .collect();
                for (&k, [tier_sums, _]) in pending
                    .iter()
                    .zip(flight_sums(square, tier, &requests, batch)?)
                {
                    sums[k].add(tier_sums);
                }
            }
            pending.retain(|&k| !left_out_within(&sums[k], left));
            *summed_s += tier_started.elapsed().as_secs_f64();
        }
        flights.extend(sums);
    }
    let tier_boxes = tiers.boxes.iter().map(|tier| {
        tier.lists
            .iter()
            .flatten()
            .map(|bands| bands[0].len())
            .sum::<usize>()
    });
    eprintln!(
        "qm-paint: {} façades' flights in tiers (façades, boxes, s) {:?}, ceilings {ceilings_s:.1} s, all {:.1} s",
        points.len(),
        (summing.iter().zip(tier_boxes).zip(summed_s))
            .map(|((&facades, boxes), seconds)| (facades, boxes, (seconds * 10.0).round() / 10.0))
            .collect::<Vec<_>>(),
        started.elapsed().as_secs_f64()
    );
    Ok(flights)
}

/// Whether the ceilings `left` are within [`LEFT_OUT_AT_MOST`] of each of `sums` (never of a sum
/// that is not finite).
fn left_out_within(sums: &FlightSums, left: &Ceilings) -> bool {
    let summed = [sums.energy, sums.lambda.energy, sums.lambda.energy_lambda];
    if summed.iter().flatten().any(|sum| !sum.is_finite()) {
        return false;
    }
    (0..3).all(|part| {
        (0..PERIODS).all(|period| left[part][period] <= LEFT_OUT_AT_MOST * summed[part][period])
    })
}

/// Every box `boxes` lists, ranked by its ceiling over `area` (its Lden energy, ties by place), in
/// the tiers of [`TIER_SHARES`].
fn tiers(square: &Square, boxes: &FlightBoxes, area: &CeilingArea) -> Tiers {
    // Per box its place (tile, file, index) and ceilings, as f32 rounded up (a positive one to at
    // least the least normal f32).
    let up = |value: f64| match value > 0.0 {
        true => ((value * (1.0 + 1e-6)) as f32).max(f32::MIN_POSITIVE),
        false => 0.0,
    };
    let boxed: Vec<([u32; 3], [[f32; PERIODS]; 3])> = (square.aircraft.par_iter().enumerate())
        .flat_map_iter(|(number, tile)| {
            let lists = &boxes.lists[number];
            [tile.fine.as_ref(), tile.far.as_ref()]
                .into_iter()
                .enumerate()
                .filter_map(|(file, aircraft)| aircraft.map(|aircraft| (file, aircraft)))
                .flat_map(move |(file, aircraft)| {
                    lists[file].iter().flatten().map(move |&index| {
                        let record = aircraft.aircraft_box(index);
                        let ceiling = box_flight_ceiling(&record, tile.tile, &square.frame, area);
                        // A far box weighs the lambda, and a fine one where its tile has no far file.
                        let weighs_lambda = file == 1 || tile.far.is_none();
                        let weighs = |values: [f64; PERIODS]| match weighs_lambda {
                            true => values.map(up),
                            false => [0.0; PERIODS],
                        };
                        let place = [number, file, index].map(|value| value as u32);
                        (
                            place,
                            [
                                ceiling.energy.map(up),
                                weighs(ceiling.energy),
                                weighs(ceiling.energy_lambda),
                            ],
                        )
                    })
                })
        })
        .collect();
    let keys: Vec<f64> = (boxed.par_iter())
        .map(|(_, ceilings)| lden_energy(&ceilings[0].map(f64::from)))
        .collect();
    let mut order: Vec<u32> = (0..boxed.len() as u32).collect();
    order.par_sort_unstable_by(|&a, &b| {
        keys[b as usize]
            .total_cmp(&keys[a as usize])
            .then(a.cmp(&b))
    });
    let mut start = 0;
    let (mut tier_boxes, mut tier_sums) = (Vec::new(), Vec::new());
    for share in TIER_SHARES {
        let end = ((share * order.len() as f64).ceil() as usize).clamp(start, order.len());
        let mut lists: Vec<[[Vec<usize>; 2]; 2]> = (0..square.aircraft.len())
            .map(|_| Default::default())
            .collect();
        let mut sums: Ceilings = [[0.0; PERIODS]; 3];
        for &slot in &order[start..end] {
            let ([number, file, index], ceilings) = &boxed[slot as usize];
            lists[*number as usize][*file as usize][0].push(*index as usize);
            for (sum, ceiling) in sums.iter_mut().flatten().zip(ceilings.iter().flatten()) {
                *sum += f64::from(*ceiling);
            }
        }
        for list in lists.iter_mut().flatten() {
            list[0].sort_unstable();
        }
        let occupied = [end > start, false];
        tier_boxes.push(FlightBoxes {
            lists,
            occupied,
            coarse_m: boxes.coarse_m,
        });
        tier_sums.push(sums);
        start = end;
    }
    // Each tier's left: the tiers after it, summed in f64 over values rounded up, a part per
    // billion more for the sums' own rounding.
    let left = (0..tier_sums.len())
        .map(|tier| {
            std::array::from_fn(|part| {
                std::array::from_fn(|period| {
                    tier_sums[tier + 1..]
                        .iter()
                        .map(|sums| sums[part][period])
                        .sum::<f64>()
                        * (1.0 + 1e-9)
                })
            })
        })
        .collect();
    Tiers {
        boxes: tier_boxes,
        left,
    }
}

#[cfg(test)]
#[path = "facade_flights_tests.rs"]
mod tests;

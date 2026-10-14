//! Aircraft boxes assigned once to the finest distance lattice any receiver in the square requires.

use crate::exact::{Batch, Point};
use crate::flights::{FlightSums, HeardTile, tiles_heard};
use crate::square::{Square, gap_to_tile};
use physics::bands::PERIODS;
use popup::aircraft::FINE_BOXES_WITHIN_M;
use popup::aircraft::boxes::{
    AircraftReceiver, ReceiverArea, minimum_box_slant_m, tile_flight_sums,
};
use popup::aircraft::horizons::Horizons;
use rayon::prelude::*;

/// How a lattice weighs a box at its horizontal distance from the point: wholly, as far (the
/// smoothstep from 1.5 to 3 of the hum's spacings) or as near (the rest).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlightWeight {
    One,
    Far,
    Near,
}

impl FlightWeight {
    /// The weight at `distance_m`, the hum's spacing `coarse_m`.
    pub fn at(self, distance_m: f64, coarse_m: f64) -> f64 {
        let fraction = ((distance_m - 1.5 * coarse_m) / (1.5 * coarse_m)).clamp(0.0, 1.0);
        let far = fraction * fraction * (3.0 - 2.0 * fraction);
        match self {
            FlightWeight::One => 1.0,
            FlightWeight::Far => far,
            FlightWeight::Near => 1.0 - far,
        }
    }
}

/// One point's flights to sum: per band of boxes its weight (`None`: not summed), the boxes heard
/// within `within_m`.
pub struct FlightRequest<'a> {
    pub point: &'a Point,
    pub weights: [Option<FlightWeight>; 2],
    pub within_m: f64,
}

/// Per aircraft tile of the square, of its fine and its far boxes those in each band: near enough
/// to need the hum's lattice, or beyond 8 km of every point and unscreened (every fourth).
pub struct FlightBoxes {
    pub lists: Vec<[[Vec<usize>; 2]; 2]>,
    pub occupied: [bool; 2],
    /// The hum's spacing (m), which the far and near weights scale with.
    pub coarse_m: f64,
}

impl FlightBoxes {
    pub fn new(square: &Square, corners: &[Point], coarse_m: f64) -> Self {
        let mut area = ReceiverArea {
            low: [f64::INFINITY; 2],
            high: [f64::NEG_INFINITY; 2],
            altitudes_m: [f64::INFINITY, f64::NEG_INFINITY],
        };
        for point in corners {
            for axis in 0..2 {
                area.low[axis] = area.low[axis].min(point.position[axis]);
                area.high[axis] = area.high[axis].max(point.position[axis]);
            }
            area.altitudes_m[0] = area.altitudes_m[0].min(point.altitude_m);
            area.altitudes_m[1] = area.altitudes_m[1].max(point.altitude_m);
        }
        let lists: Vec<_> = square
            .aircraft
            .par_iter()
            .map(|tile| {
                let gaps = [(0, 0), (0, 1), (1, 0), (1, 1)].map(|(east, north)| {
                    gap_to_tile(
                        &square.frame,
                        tile.tile,
                        [
                            [area.low[0], area.high[0]][east],
                            [area.low[1], area.high[1]][north],
                        ],
                    )
                });
                let changes_files = tile.fine.is_some()
                    && gaps.iter().any(|&gap| gap <= FINE_BOXES_WITHIN_M)
                    && gaps.iter().any(|&gap| gap > FINE_BOXES_WITHIN_M);
                [tile.fine.as_ref(), tile.far.as_ref()].map(|boxes| {
                    let mut lists: [Vec<usize>; 2] = std::array::from_fn(|_| Vec::new());
                    if let Some(boxes) = boxes {
                        for index in 0..boxes.box_count() {
                            let distance = minimum_box_slant_m(
                                &boxes.aircraft_box(index),
                                tile.tile,
                                &square.frame,
                                area,
                            );
                            let band = usize::from(!changes_files && distance >= 8_000.0);
                            lists[band].push(index);
                        }
                    }
                    lists
                })
            })
            .collect();
        let occupied = std::array::from_fn(|band| {
            lists
                .iter()
                .any(|tile| tile.iter().any(|boxes| !boxes[band].is_empty()))
        });
        Self {
            lists,
            occupied,
            coarse_m,
        }
    }

    /// The flights of `request` on the cores: per band its boxes' energy and energy times lambda.
    pub fn sums(
        &self,
        square: &Square,
        request: &FlightRequest,
    ) -> Result<[FlightSums; 2], String> {
        let mut total = [[[0.0; PERIODS]; 2]; 2];
        let point = request.point;
        let heard = tiles_heard(square, point.position, request.within_m);
        if !self.hears_any(&heard, request.weights) {
            return Ok(total);
        }
        let horizons = Horizons::build(
            &square.ground,
            &square.obstacles,
            point.position,
            point.altitude_m,
            point.own_footprint,
        )?;
        let receiver = AircraftReceiver {
            position: point.position,
            altitude_m: point.altitude_m,
        };
        for tile in heard {
            let lists = &self.lists[tile.number][usize::from(!tile.fine)];
            for ((band_total, weight), list) in total.iter_mut().zip(request.weights).zip(lists) {
                let Some(weight) = weight else {
                    continue;
                };
                let indices = list.iter().copied();
                let weight = |distance_m: f64| weight.at(distance_m, self.coarse_m);
                let sums = tile_flight_sums(
                    &tile.boxes,
                    tile.tile,
                    &square.frame,
                    receiver,
                    &horizons,
                    (indices, &weight, request.within_m),
                );
                for (totals, values) in band_total.iter_mut().zip([sums.0, sums.1]) {
                    for (sum, value) in totals.iter_mut().zip(values) {
                        *sum += value;
                    }
                }
            }
        }
        Ok(total)
    }

    /// Whether any of `heard` holds boxes of a band `weights` sums.
    pub fn hears_any(&self, heard: &[HeardTile], weights: [Option<FlightWeight>; 2]) -> bool {
        heard.iter().any(|tile| {
            (0..2).any(|band| {
                weights[band].is_some()
                    && !self.lists[tile.number][usize::from(!tile.fine)][band].is_empty()
            })
        })
    }
}

/// The flights of every request: on the card when there is one, else on the cores.
pub(crate) fn flight_sums(
    square: &Square,
    boxes: &FlightBoxes,
    requests: &[FlightRequest],
    batch: Option<&dyn Batch>,
) -> Result<Vec<[FlightSums; 2]>, String> {
    match batch {
        Some(batch) => batch.flight_sums(square, boxes, requests),
        None => requests
            .par_iter()
            .map(|request| boxes.sums(square, request))
            .collect(),
    }
}

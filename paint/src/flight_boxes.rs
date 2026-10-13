//! Aircraft boxes assigned once to the finest distance lattice any receiver in the square requires.

use crate::exact::Point;
use crate::flights::{FlightSums, tiles_heard};
use crate::square::{Square, gap_to_tile};
use physics::bands::PERIODS;
use popup::aircraft::FINE_BOXES_WITHIN_M;
use popup::aircraft::boxes::{
    AircraftReceiver, ReceiverArea, minimum_box_slant_m, tile_flight_sums,
};
use popup::aircraft::horizons::Horizons;
use rayon::prelude::*;

pub(crate) struct FlightBoxes {
    lists: Vec<[[Vec<usize>; 2]; 2]>,
    pub occupied: [bool; 2],
}

impl FlightBoxes {
    pub fn new(square: &Square, corners: &[Point]) -> Self {
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
        Self { lists, occupied }
    }

    pub fn at(
        &self,
        square: &Square,
        point: &Point,
        selected: [bool; 2],
        weights: [&dyn Fn(f64) -> f64; 2],
        within_m: f64,
    ) -> Result<[FlightSums; 2], String> {
        let mut total = [[[0.0; PERIODS]; 2]; 2];
        if selected.iter().all(|&selected| !selected) {
            return Ok(total);
        }
        let heard = tiles_heard(square, point.position, within_m);
        if !heard.iter().any(|tile| {
            (0..2).any(|band| {
                selected[band] && !self.lists[tile.number][usize::from(!tile.fine)][band].is_empty()
            })
        }) {
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
            for band in 0..2 {
                if !selected[band] {
                    continue;
                }
                let indices = self.lists[tile.number][usize::from(!tile.fine)][band]
                    .iter()
                    .copied();
                let sums = tile_flight_sums(
                    &tile.boxes,
                    tile.tile,
                    &square.frame,
                    receiver,
                    &horizons,
                    (indices, weights[band], within_m),
                );
                for (totals, values) in total[band].iter_mut().zip([sums.0, sums.1]) {
                    for (sum, value) in totals.iter_mut().zip(values) {
                        *sum += value;
                    }
                }
            }
        }
        Ok(total)
    }
}

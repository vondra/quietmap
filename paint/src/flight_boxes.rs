//! Aircraft boxes assigned once to the finest distance lattice any receiver in the square requires.

use crate::exact::Point;
use crate::flights::{FlightSums, tiles_heard};
use crate::square::{Square, gap_to_tile};
use physics::bands::PERIODS;
use popup::aircraft::FINE_BOXES_WITHIN_M;
use popup::aircraft::boxes::{
    AIRCRAFT_REACH_M, AircraftReceiver, ReceiverArea, decoded_flight_sums, minimum_box_slant_m,
};
use popup::aircraft::horizons::{Horizons, HorizonsCache, LazyHorizons as SectorLazyHorizons};
use rayon::prelude::*;
use std::sync::OnceLock;
use tiles::aircraft::AircraftBox;

#[derive(Default)]
pub(crate) struct HorizonsSlot {
    pub full: OnceLock<Result<Horizons, String>>,
    pub sectors: OnceLock<Box<HorizonsCache>>,
}

impl HorizonsSlot {
    pub fn new() -> Self {
        Self::default()
    }
}

pub(crate) fn lazy_horizons<'a>(
    square: &'a Square<'a>,
    point: &'a Point,
    slot: &'a HorizonsSlot,
) -> SectorLazyHorizons<'a> {
    SectorLazyHorizons {
        ground: &square.ground,
        obstacles: &square.obstacles,
        position: point.position,
        altitude_m: point.altitude_m,
        own_footprint: point.own_footprint,
        cache: &slot.sectors,
    }
}

struct TileFlightBoxes {
    decoded: Vec<([f64; 2], AircraftBox)>,
    lists: [Vec<usize>; 3],
}

pub(crate) type BandWeight<'a> = Option<(usize, &'a dyn Fn(f64) -> f64)>;

pub(crate) struct FlightBoxes {
    tiles: Vec<[TileFlightBoxes; 2]>,
    pub occupied: [bool; 3],
}

impl FlightBoxes {
    pub fn new(square: &Square, corners: &[Point], near_within_m: f64) -> Self {
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
        let tiles: Vec<_> = square
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
                let uses_fine =
                    tile.fine.is_some() && gaps.iter().any(|&gap| gap <= FINE_BOXES_WITHIN_M);
                let uses_far =
                    tile.fine.is_none() || gaps.iter().any(|&gap| gap > FINE_BOXES_WITHIN_M);
                let changes_files = uses_fine && uses_far;
                [
                    uses_fine.then_some(tile.fine.as_ref()).flatten(),
                    uses_far.then_some(tile.far.as_ref()).flatten(),
                ]
                .map(|boxes| {
                    let mut decoded = Vec::new();
                    let mut lists: [Vec<usize>; 3] = std::array::from_fn(|_| Vec::new());
                    if let Some(boxes) = boxes {
                        for index in 0..boxes.box_count() {
                            let global = tile.tile.global(boxes.box_centroid(index));
                            let [cx, cy] = square
                                .frame
                                .metres_of_steps([global.x as f64, global.y as f64]);
                            let dx = (area.low[0] - cx).max(cx - area.high[0]).max(0.0);
                            let dy = (area.low[1] - cy).max(cy - area.high[1]).max(0.0);
                            let area_dist_m = dx.hypot(dy);
                            if area_dist_m > AIRCRAFT_REACH_M + 1e-6 {
                                continue;
                            }
                            let record = boxes.aircraft_box(index);
                            let distance =
                                minimum_box_slant_m(&record, tile.tile, &square.frame, area);
                            let decoded_index = decoded.len();
                            decoded.push(([cx, cy], record));
                            if !changes_files && distance >= 8_000.0 {
                                lists[2].push(decoded_index);
                            } else {
                                if area_dist_m <= near_within_m + 1e-6 {
                                    lists[0].push(decoded_index);
                                }
                                lists[1].push(decoded_index);
                            }
                        }
                    }
                    TileFlightBoxes { decoded, lists }
                })
            })
            .collect();
        let occupied = std::array::from_fn(|band| {
            tiles
                .iter()
                .any(|tile| tile.iter().any(|boxes| !boxes.lists[band].is_empty()))
        });
        Self { tiles, occupied }
    }

    pub fn at(
        &self,
        square: &Square,
        point: &Point,
        cached_horizons: &HorizonsSlot,
        bands: [BandWeight<'_>; 2],
        within_m: f64,
    ) -> Result<[FlightSums; 2], String> {
        let mut total = [[[0.0; PERIODS]; 2]; 2];
        if bands.iter().all(Option::is_none) {
            return Ok(total);
        }
        let heard = tiles_heard(square, point.position, within_m);
        if !heard.iter().any(|tile| {
            bands.iter().flatten().any(|&(band, _)| {
                !self.tiles[tile.number][usize::from(!tile.fine)].lists[band].is_empty()
            })
        }) {
            return Ok(total);
        }
        let horizons = cached_horizons
            .full
            .get_or_init(|| {
                Horizons::build(
                    &square.ground,
                    &square.obstacles,
                    point.position,
                    point.altitude_m,
                    point.own_footprint,
                )
            })
            .as_ref()
            .map_err(Clone::clone)?;
        let receiver = AircraftReceiver {
            position: point.position,
            altitude_m: point.altitude_m,
        };
        for tile in heard {
            let slot_boxes = &self.tiles[tile.number][usize::from(!tile.fine)];
            for (slot, band) in bands.iter().enumerate() {
                let Some((band, weight)) = *band else {
                    continue;
                };
                let indices = slot_boxes.lists[band]
                    .iter()
                    .map(|&idx| &slot_boxes.decoded[idx]);
                let sums = decoded_flight_sums(indices, receiver, horizons, (weight, within_m));
                for (totals, values) in total[slot].iter_mut().zip([sums.0, sums.1]) {
                    for (sum, value) in totals.iter_mut().zip(values) {
                        *sum += value;
                    }
                }
            }
        }
        Ok(total)
    }
}

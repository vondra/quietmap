//! Loudest flight pieces found from box bounds over a square, with horizons only for receivers hearing pieces.
//!
//! The loudest flights at a square's points without bounding every box at every point: each box's
//! bound on its pieces' LAmax over the whole square, sorted once a square, and a point's own bounds
//! taken in that order only until the loudest left over the square could not pass what the point
//! has bounded. The point reads the same boxes in the same order as a click's full sort
//! (`popup::aircraft::loudest_first`): of half a million boxes in reach of a Dobris point, a few
//! thousand.

use crate::exact::Point;
use crate::flight_boxes::{HorizonsSlot, lazy_horizons};
use crate::flights::tiles_heard;
use crate::square::{Square, gap_to_tile};
use physics::doc29::atmosphere::receiver_impedance_db;
use popup::aircraft::FINE_BOXES_WITHIN_M;
use popup::aircraft::boxes::{
    AIRCRAFT_REACH_M, AircraftReceiver, ReceiverArea, box_lamax_bound, box_lamax_bound_over,
};
use popup::aircraft::flights::FlightTotals;
use popup::aircraft::{loudest_first, read_loudest_in_order};
use rayon::prelude::*;
use std::cmp::Ordering;
use std::collections::BinaryHeap;

/// Added to a box's bound over the square: its receivers' impedance is taken at the higher of
/// their lowest and highest elevations, where it is largest as the air thins, and rounding never
/// puts a point's own bound above it.
const OVER_SQUARE_MARGIN_DB: f64 = 0.05;

/// Per aircraft tile of a square, its fine and its far boxes, each box's bound over the square's
/// points, loudest first.
pub(crate) struct LoudestIndex {
    lists: Vec<[Vec<(f64, u32)>; 2]>,
}

impl LoudestIndex {
    /// The index of `square` for receivers at `points`.
    pub fn new(square: &Square, points: &[Point]) -> Self {
        let mut area = ReceiverArea {
            low: [f64::INFINITY; 2],
            high: [f64::NEG_INFINITY; 2],
            altitudes_m: [f64::INFINITY, f64::NEG_INFINITY],
        };
        for point in points {
            for axis in 0..2 {
                area.low[axis] = area.low[axis].min(point.position[axis]);
                area.high[axis] = area.high[axis].max(point.position[axis]);
            }
            area.altitudes_m[0] = area.altitudes_m[0].min(point.altitude_m);
            area.altitudes_m[1] = area.altitudes_m[1].max(point.altitude_m);
        }
        let impedance_db = receiver_impedance_db(area.altitudes_m[0])
            .max(receiver_impedance_db(area.altitudes_m[1]));
        let lists = (square.aircraft.par_iter())
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
                [
                    uses_fine.then_some(tile.fine.as_ref()).flatten(),
                    uses_far.then_some(tile.far.as_ref()).flatten(),
                ]
                .map(|boxes| {
                    let Some(boxes) = boxes else {
                        return Vec::new();
                    };
                    let mut list: Vec<(f64, u32)> = (0..boxes.box_count())
                        .filter_map(|index| {
                            let global = tile.tile.global(boxes.box_centroid(index));
                            let [cx, cy] = square
                                .frame
                                .metres_of_steps([global.x as f64, global.y as f64]);
                            let dx = (area.low[0] - cx).max(cx - area.high[0]).max(0.0);
                            let dy = (area.low[1] - cy).max(cy - area.high[1]).max(0.0);
                            if dx.hypot(dy) > AIRCRAFT_REACH_M + 1e-6 {
                                return None;
                            }
                            let bound = box_lamax_bound_over(
                                boxes,
                                (tile.tile, index),
                                &square.frame,
                                area,
                                impedance_db,
                            )?;
                            Some((bound + OVER_SQUARE_MARGIN_DB, index as u32))
                        })
                        .collect();
                    list.sort_unstable_by(|a, b| b.0.total_cmp(&a.0));
                    list
                })
            })
            .collect();
        LoudestIndex { lists }
    }

    /// The loudest flights a click at `receiver` reads into `flights` (`popup::aircraft`'s
    /// search): the tiles it reads, each box's own bound taken in the order of its bound over the
    /// square, read out loudest first once no box left could pass it.
    pub fn search(
        &self,
        square: &Square,
        point: &Point,
        cached_horizons: Option<&HorizonsSlot>,
        flights: &mut FlightTotals,
    ) -> Result<(), String> {
        let receiver = AircraftReceiver {
            position: point.position,
            altitude_m: point.altitude_m,
        };
        let heard = tiles_heard(square, receiver.position, AIRCRAFT_REACH_M);
        let lists: Vec<&[(f64, u32)]> = (heard.iter())
            .map(|tile| &self.lists[tile.number][usize::from(!tile.fine)][..])
            .collect();
        if lists.iter().all(|list| list.is_empty()) {
            return Ok(());
        }
        let tiles: Vec<_> = heard.iter().map(|tile| (tile.tile, tile.boxes)).collect();
        let impedance_db = receiver_impedance_db(receiver.altitude_m);
        let mut over: BinaryHeap<Over> = (lists.iter().enumerate())
            .filter_map(|(t, list)| list.first().map(|&(bound, _)| Over(bound, t, 0)))
            .collect();
        let mut own: BinaryHeap<Own> = BinaryHeap::new();
        let ordered = std::iter::from_fn(|| {
            loop {
                if let Some(Own(bound, ..)) = own.peek()
                    && over.peek().is_none_or(|next| *bound > next.0)
                {
                    return own.pop().map(|Own(bound, t, index)| (bound, t, index));
                }
                let Over(_, t, at) = over.pop()?;
                if let Some(&(bound, _)) = lists[t].get(at + 1) {
                    over.push(Over(bound, t, at + 1));
                }
                let index = lists[t][at].1 as usize;
                let (tile, boxes) = &tiles[t];
                if let Some(bound) =
                    box_lamax_bound(boxes, (*tile, index), &square.frame, receiver, impedance_db)
                {
                    own.push(Own(bound, t, index));
                }
            }
        });
        let local_lock = HorizonsSlot::new();
        let slot = cached_horizons.unwrap_or(&local_lock);
        if let Some(full) = slot.full.get() {
            let horizons = full.as_ref().map_err(Clone::clone)?;
            read_loudest_in_order(
                ordered,
                &tiles,
                (&square.frame, receiver, horizons),
                flights,
            );
        } else {
            let horizons = lazy_horizons(square, point, slot);
            read_loudest_in_order(
                ordered,
                &tiles,
                (&square.frame, receiver, &horizons),
                flights,
            );
            horizons.check_error()?;
        }
        Ok(())
    }
}

/// A box in the merge by its bound over the square: the bound, its tile among those heard, its
/// place in that tile's list; the greatest bound first.
struct Over(f64, usize, usize);

impl PartialEq for Over {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Over {}
impl PartialOrd for Over {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Over {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

/// A box bounded at the point: its bound, its tile among those heard, its index; first in the
/// search's order first.
struct Own(f64, usize, usize);

impl PartialEq for Own {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Own {}
impl PartialOrd for Own {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Own {
    fn cmp(&self, other: &Self) -> Ordering {
        loudest_first(&(other.0, other.1, other.2), &(self.0, self.1, self.2))
    }
}

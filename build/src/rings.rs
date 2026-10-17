//! Outlines by place, z30: which one holds a position. A site's point inside a building takes the
//! building as its own (no screening by it); an unnamed building inside an industrial site is the
//! site's, not a home.

use crate::dev4::Z30_QUANTUM_M;
use crate::sources::cells::Z30Ring;
use std::collections::HashMap;

/// The index's cell: 2^14 z30 cells, about 600 m of Web Mercator.
const CELL_SHIFT: u32 = 14;

/// Outlines with ids: each of its parts an exterior ring and its holes.
#[derive(Default)]
pub struct RingIndex {
    items: Vec<(u64, Vec<Vec<Z30Ring>>)>,
    cells: HashMap<(i32, i32), Vec<u32>>,
}

impl RingIndex {
    pub fn add(&mut self, id: u64, parts: Vec<Vec<Z30Ring>>) {
        let exteriors = parts.iter().filter_map(|rings| rings.first()).flatten();
        let (mut low, mut high) = ((i32::MAX, i32::MAX), (i32::MIN, i32::MIN));
        for &(x, y) in exteriors {
            (low, high) = ((low.0.min(x), low.1.min(y)), (high.0.max(x), high.1.max(y)));
        }
        if low.0 > high.0 {
            return;
        }
        let at = self.items.len() as u32;
        for cx in low.0 >> CELL_SHIFT..=high.0 >> CELL_SHIFT {
            for cy in low.1 >> CELL_SHIFT..=high.1 >> CELL_SHIFT {
                self.cells.entry((cx, cy)).or_default().push(at);
            }
        }
        self.items.push((id, parts));
    }

    /// The id of an outline holding `position` (inside a part's exterior, outside its holes); of
    /// overlapping ones the first added.
    pub fn containing(&self, position: (i32, i32)) -> Option<u64> {
        let cell = (position.0 >> CELL_SHIFT, position.1 >> CELL_SHIFT);
        self.cells
            .get(&cell)?
            .iter()
            .map(|&at| &self.items[at as usize])
            .find_map(|(id, parts)| {
                parts
                    .iter()
                    .any(|rings| {
                        rings
                            .first()
                            .is_some_and(|exterior| inside(exterior, position))
                            && !rings[1..].iter().any(|hole| inside(hole, position))
                    })
                    .then_some(*id)
            })
    }
}

/// Even-odd: whether `(x, y)` lies inside `ring` (closed or not), exactly in integers.
fn inside(ring: &[(i32, i32)], (x, y): (i32, i32)) -> bool {
    let (x, y) = (i64::from(x), i64::from(y));
    let mut inside = false;
    for (k, &(xi, yi)) in ring.iter().enumerate() {
        let (xj, yj) = ring[(k + ring.len() - 1) % ring.len()];
        let (xi, yi, xj, yj) = (i64::from(xi), i64::from(yi), i64::from(xj), i64::from(yj));
        if (yi > y) != (yj > y) {
            // x left of the edge's crossing at y: (x - xi)(yj - yi) < (y - yi)(xj - xi), the sign
            // turned with the edge's direction.
            let (left, right) = ((x - xi) * (yj - yi), (y - yi) * (xj - xi));
            if (yj > yi && left < right) || (yj < yi && left > right) {
                inside = !inside;
            }
        }
    }
    inside
}

/// z30 cells covering `ground_m` of ground at latitude `latitude_deg` (Web Mercator stretches by
/// the secant).
pub fn z30_cells(ground_m: f64, latitude_deg: f64) -> i32 {
    (ground_m / latitude_deg.to_radians().cos().max(0.05) / Z30_QUANTUM_M).ceil() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A square with a square hole, and a second outline overlapping it: a position in the hole is
    /// only the second's, one outside both is none's.
    #[test]
    fn a_position_is_held_by_the_outline_around_it_and_not_by_a_hole() {
        let mut index = RingIndex::default();
        let square = |low: i32, high: i32| vec![(low, low), (high, low), (high, high), (low, high)];
        index.add(7, vec![vec![square(0, 100), square(40, 60)]]);
        index.add(9, vec![vec![square(50, 200)]]);
        assert_eq!(index.containing((10, 10)), Some(7));
        assert_eq!(index.containing((45, 45)), None);
        assert_eq!(index.containing((55, 55)), Some(9));
        assert_eq!(index.containing((150, 150)), Some(9));
        assert_eq!(index.containing((300, 10)), None);
        assert_eq!(index.containing((-1, 50)), None);
    }
}

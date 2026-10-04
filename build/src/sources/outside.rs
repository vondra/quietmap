//! Points mapped inside a building moved just outside its nearest wall: the people of a bar or a
//! café sit and stand in front of it, where the building screens them from its other side, not
//! inside it, where its own walls would screen them from every receiver. Footprints are dev4's
//! screening outlines (OpenStreetMap's and Overture's), a point in a courtyard is outside.

use crate::dev4::Z30_QUANTUM_M;
use std::collections::HashMap;

/// People stand this far (m) in front of the wall.
const OUT_FROM_WALL_M: f64 = 1.5;
/// Grid cells of 2^12 z30 units (about 150 m at the equator) index the points.
const CELL_SHIFT: u32 = 12;

/// The points of a square and, once footprints are offered, where each goes.
pub struct OutsidePlacer {
    points: Vec<(i32, i32)>,
    moved: Vec<Option<(i32, i32)>>,
    grid: HashMap<(i32, i32), Vec<usize>>,
    /// z30 units of a metre on the ground at the square's latitude.
    units_per_m: f64,
}

fn cell(point: (i32, i32)) -> (i32, i32) {
    (point.0 >> CELL_SHIFT, point.1 >> CELL_SHIFT)
}

/// Even-odd containment of `point` in `ring` (z30, closed or not).
fn inside(point: (f64, f64), ring: &[(i32, i32)]) -> bool {
    let mut within = false;
    let mut previous = ring[ring.len() - 1];
    for &vertex in ring {
        let (a, b) = (
            (f64::from(vertex.0), f64::from(vertex.1)),
            (f64::from(previous.0), f64::from(previous.1)),
        );
        if (a.1 > point.1) != (b.1 > point.1)
            && point.0 < (b.0 - a.0) * (point.1 - a.1) / (b.1 - a.1) + a.0
        {
            within = !within;
        }
        previous = vertex;
    }
    within
}

/// The point of `ring`'s edges nearest `point`.
fn nearest_on_ring(point: (f64, f64), ring: &[(i32, i32)]) -> (f64, f64) {
    let mut best = (f64::INFINITY, point);
    let mut previous = ring[ring.len() - 1];
    for &vertex in ring {
        let (a, b) = (
            (f64::from(previous.0), f64::from(previous.1)),
            (f64::from(vertex.0), f64::from(vertex.1)),
        );
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let length_sq = dx * dx + dy * dy;
        let t = if length_sq > 0.0 {
            (((point.0 - a.0) * dx + (point.1 - a.1) * dy) / length_sq).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let candidate = (a.0 + t * dx, a.1 + t * dy);
        let distance = (candidate.0 - point.0).hypot(candidate.1 - point.1);
        if distance < best.0 {
            best = (distance, candidate);
        }
        previous = vertex;
    }
    best.1
}

impl OutsidePlacer {
    /// `points` (z30) at about `latitude` degrees.
    pub fn new(points: Vec<(i32, i32)>, latitude: f64) -> Self {
        let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (index, &point) in points.iter().enumerate() {
            grid.entry(cell(point)).or_default().push(index);
        }
        OutsidePlacer {
            moved: vec![None; points.len()],
            points,
            grid,
            units_per_m: 1.0 / (Z30_QUANTUM_M * latitude.to_radians().cos()),
        }
    }

    /// Whether any point lies within a cell of `centre` (a footprint's centroid): only then is the
    /// footprint worth decoding.
    pub fn near(&self, centre: (i32, i32)) -> bool {
        let (cx, cy) = cell(centre);
        (-1..=1).any(|dx| (-1..=1).any(|dy| self.grid.contains_key(&(cx + dx, cy + dy))))
    }

    /// A footprint's part (its exterior, then its holes): the points inside it go outside its
    /// nearest wall.
    pub fn offer(&mut self, part: &[Vec<(i32, i32)>]) {
        let Some(exterior) = part.first().filter(|ring| ring.len() >= 3) else {
            return;
        };
        let (min, max) = exterior.iter().fold(
            ((i32::MAX, i32::MAX), (i32::MIN, i32::MIN)),
            |(min, max), &(x, y)| ((min.0.min(x), min.1.min(y)), (max.0.max(x), max.1.max(y))),
        );
        let (low, high) = (cell(min), cell(max));
        for cx in low.0..=high.0 {
            for cy in low.1..=high.1 {
                let Some(indices) = self.grid.get(&(cx, cy)) else {
                    continue;
                };
                for &index in indices {
                    let point = self.points[index];
                    if self.moved[index].is_some()
                        || point.0 < min.0
                        || point.0 > max.0
                        || point.1 < min.1
                        || point.1 > max.1
                    {
                        continue;
                    }
                    let at = (f64::from(point.0) + 0.5, f64::from(point.1) + 0.5);
                    let in_hole = part[1..]
                        .iter()
                        .any(|hole| hole.len() >= 3 && inside(at, hole));
                    if !inside(at, exterior) || in_hole {
                        continue;
                    }
                    let wall = nearest_on_ring(at, exterior);
                    let (dx, dy) = (wall.0 - at.0, wall.1 - at.1);
                    let length = dx.hypot(dy).max(1e-9);
                    let step = OUT_FROM_WALL_M * self.units_per_m / length;
                    self.moved[index] = Some((
                        (wall.0 + dx * step).round() as i32,
                        (wall.1 + dy * step).round() as i32,
                    ));
                }
            }
        }
    }

    /// Each point's position: outside the footprint it fell in, else where it was.
    pub fn positions(&self) -> Vec<(i32, i32)> {
        self.points
            .iter()
            .zip(&self.moved)
            .map(|(&point, moved)| moved.unwrap_or(point))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bar mapped 3 m inside the street wall of a 20 x 10 m building goes 1.5 m in front of that
    /// wall; a café in the courtyard stays; a terrace on the street stays.
    #[test]
    fn points_inside_a_building_go_outside_its_nearest_wall() {
        let m = 1.0 / Z30_QUANTUM_M; // at the equator
        let at = |x: f64, y: f64| ((x * m) as i32, (y * m) as i32);
        let outline: Vec<(i32, i32)> =
            vec![at(0.0, 0.0), at(20.0, 0.0), at(20.0, 10.0), at(0.0, 10.0)];
        let courtyard: Vec<(i32, i32)> =
            vec![at(8.0, 4.0), at(12.0, 4.0), at(12.0, 6.0), at(8.0, 6.0)];
        let mut placer =
            OutsidePlacer::new(vec![at(10.0, 3.0), at(10.0, 5.0), at(10.0, -4.0)], 0.0);
        assert!(placer.near(at(10.0, 5.0)));
        placer.offer(&[outline, courtyard]);
        let positions = placer.positions();
        let metres = |p: (i32, i32)| (f64::from(p.0) / m, f64::from(p.1) / m);
        let bar = metres(positions[0]);
        assert!(
            (bar.0 - 10.0).abs() < 0.1 && (bar.1 + 1.5).abs() < 0.1,
            "{bar:?}"
        );
        assert_eq!(positions[1], at(10.0, 5.0));
        assert_eq!(positions[2], at(10.0, -4.0));
    }
}

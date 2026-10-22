//! Points mapped inside a building moved just outside its nearest free wall, a courtyard's as a
//! street's: the people of a bar or a café sit and stand in front of it, where the building screens
//! them from its other side, not inside it, where its own walls would screen them from every
//! receiver. A wall another building stands against is not free: a row's party wall would put them
//! in the neighbour (beach pavilions: four restaurants 24 dB screened from
//! the promenade before their terraces). Footprints are dev4's screening outlines (OpenStreetMap's
//! and Overture's), a point in a courtyard is outside.

use crate::dev4::Z30_QUANTUM_M;
use std::collections::HashMap;

/// People stand this far (m) in front of the wall.
const OUT_FROM_WALL_M: f64 = 1.5;
/// Grid cells of 2^12 z30 units (about 150 m at the equator) index the points and the footprints.
const CELL_SHIFT: u32 = 12;

type Ring = Vec<(i32, i32)>;

/// A footprint's part: its exterior, then its holes, and its exterior's corners.
struct Part {
    rings: Vec<Ring>,
    min: (i32, i32),
    max: (i32, i32),
}

impl Part {
    /// Whether `point` (z30) lies in the part: in its exterior, not in a hole.
    fn covers(&self, point: (f64, f64)) -> bool {
        let (x, y) = point;
        x >= f64::from(self.min.0)
            && x <= f64::from(self.max.0) + 1.0
            && y >= f64::from(self.min.1)
            && y <= f64::from(self.max.1) + 1.0
            && inside(point, &self.rings[0])
            && !self.rings[1..]
                .iter()
                .any(|hole| hole.len() >= 3 && inside(point, hole))
    }
}

/// The points of a square and, once footprints are offered, where each goes and the source a
/// point belongs to: the building it was mapped in, where that building is offered as one.
pub struct OutsidePlacer {
    points: Vec<(i32, i32)>,
    hosts: Vec<Option<u64>>,
    grid: HashMap<(i32, i32), Vec<usize>>,
    parts: Vec<Part>,
    /// The parts by the cells their corners span.
    part_grid: HashMap<(i32, i32), Vec<usize>>,
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

/// Each edge of `ring` as its point nearest `point` and that distance.
fn walls(point: (f64, f64), ring: &[(i32, i32)]) -> impl Iterator<Item = Wall> + '_ {
    let previous = std::iter::once(ring[ring.len() - 1]).chain(ring.iter().copied());
    previous.zip(ring.iter().copied()).map(move |(a, b)| {
        let (a, b) = (
            (f64::from(a.0), f64::from(a.1)),
            (f64::from(b.0), f64::from(b.1)),
        );
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let length_sq = dx * dx + dy * dy;
        let t = if length_sq > 0.0 {
            (((point.0 - a.0) * dx + (point.1 - a.1) * dy) / length_sq).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let at = (a.0 + t * dx, a.1 + t * dy);
        Wall {
            at,
            distance: (at.0 - point.0).hypot(at.1 - point.1),
        }
    })
}

struct Wall {
    at: (f64, f64),
    distance: f64,
}

impl OutsidePlacer {
    /// `points` (z30) at about `latitude` degrees.
    pub fn new(points: Vec<(i32, i32)>, latitude: f64) -> Self {
        let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (index, &point) in points.iter().enumerate() {
            grid.entry(cell(point)).or_default().push(index);
        }
        OutsidePlacer {
            hosts: vec![None; points.len()],
            points,
            grid,
            parts: Vec::new(),
            part_grid: HashMap::new(),
            units_per_m: 1.0 / (Z30_QUANTUM_M * latitude.to_radians().cos()),
        }
    }

    /// Whether any point lies within three cells of `centre` (a footprint's centroid; 300-450 m,
    /// a mall's or a station's half-length): only then is the footprint worth decoding.
    pub fn near(&self, centre: (i32, i32)) -> bool {
        let (cx, cy) = cell(centre);
        (-3..=3).any(|dx| (-3..=3).any(|dy| self.grid.contains_key(&(cx + dx, cy + dy))))
    }

    /// A footprint's part (its exterior, then its holes): the points mapped inside it belong to
    /// `host`, the group key of the source its building is, if it is one; it is kept for
    /// [`OutsidePlacer::positions`]. A part spanning half the world's longitudes crosses the
    /// antimeridian the short way and is left out, as the industrial sites' index leaves it.
    pub fn offer(&mut self, rings: Vec<Ring>, host: Option<u64>) {
        let Some(exterior) = rings.first().filter(|ring| ring.len() >= 3) else {
            return;
        };
        let (min, max) = exterior.iter().fold(
            ((i32::MAX, i32::MAX), (i32::MIN, i32::MIN)),
            |(min, max), &(x, y)| ((min.0.min(x), min.1.min(y)), (max.0.max(x), max.1.max(y))),
        );
        if i64::from(max.0) - i64::from(min.0) > 1 << 29 {
            return;
        }
        let part = Part { rings, min, max };
        let (low, high) = (cell(min), cell(max));
        for cx in low.0..=high.0 {
            for cy in low.1..=high.1 {
                if let Some(host) = host {
                    for &index in self.grid.get(&(cx, cy)).into_iter().flatten() {
                        let mapped = self.points[index];
                        let mapped_at = (f64::from(mapped.0) + 0.5, f64::from(mapped.1) + 0.5);
                        if self.hosts[index].is_none() && part.covers(mapped_at) {
                            self.hosts[index] = Some(host);
                        }
                    }
                }
                self.part_grid
                    .entry((cx, cy))
                    .or_default()
                    .push(self.parts.len());
            }
        }
        self.parts.push(part);
    }

    /// Whether any offered footprint covers `point` (z30).
    fn covered(&self, point: (f64, f64)) -> bool {
        let key = cell((point.0.floor() as i32, point.1.floor() as i32));
        (self.part_grid.get(&key).into_iter().flatten()).any(|&part| self.parts[part].covers(point))
    }

    /// Where a point covered by footprints goes: through the nearest wall of theirs (an exterior's
    /// or a courtyard's), on from the point, to a front that, as stored (a whole z30 step, tested
    /// at its centre as the points are), no footprint covers; else (a point walled in on every side)
    /// in front of the nearest wall.
    fn outside(&self, at: (f64, f64), covering: &[usize]) -> (i32, i32) {
        let mut candidates: Vec<Wall> = covering
            .iter()
            .flat_map(|&part| self.parts[part].rings.iter())
            .filter(|ring| ring.len() >= 3)
            .flat_map(|ring| walls(at, ring))
            .filter(|wall| wall.distance > 0.0)
            .collect();
        candidates.sort_by(|a, b| a.distance.total_cmp(&b.distance));
        let step = OUT_FROM_WALL_M * self.units_per_m;
        let front = |wall: &Wall| {
            let scale = 1.0 + step / wall.distance;
            (
                (at.0 + (wall.at.0 - at.0) * scale).round() as i32,
                (at.1 + (wall.at.1 - at.1) * scale).round() as i32,
            )
        };
        candidates
            .iter()
            .map(front)
            .find(|front| !self.covered((f64::from(front.0) + 0.5, f64::from(front.1) + 0.5)))
            .unwrap_or_else(|| front(&candidates[0]))
    }

    /// Each point's position: in front of the footprints it fell in, else where it was.
    pub fn positions(&self) -> Vec<(i32, i32)> {
        self.points
            .iter()
            .map(|&point| {
                let at = (f64::from(point.0) + 0.5, f64::from(point.1) + 0.5);
                let covering: Vec<usize> = (self.part_grid.get(&cell(point)).into_iter().flatten())
                    .copied()
                    .filter(|&part| self.parts[part].covers(at))
                    .collect();
                if covering.is_empty() {
                    point
                } else {
                    self.outside(at, &covering)
                }
            })
            .collect()
    }

    /// Each point's host: the group key of the building source it was mapped in, if any.
    pub fn hosts(&self) -> &[Option<u64>] {
        &self.hosts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bar mapped 2 m inside the street wall of a 20 x 12 m building (4 m from its courtyard)
    /// goes 1.5 m in front of that wall; a café in the courtyard stays; a terrace on the street
    /// stays; a pub 1 m from the courtyard's wall and 3 m from the back street's goes into the
    /// courtyard.
    #[test]
    fn points_inside_a_building_go_outside_its_nearest_wall() {
        let m = 1.0 / Z30_QUANTUM_M; // at the equator
        let at = |x: f64, y: f64| ((x * m) as i32, (y * m) as i32);
        let outline: Vec<(i32, i32)> =
            vec![at(0.0, 0.0), at(20.0, 0.0), at(20.0, 12.0), at(0.0, 12.0)];
        let courtyard: Vec<(i32, i32)> =
            vec![at(8.0, 6.0), at(12.0, 6.0), at(12.0, 8.0), at(8.0, 8.0)];
        let mut placer = OutsidePlacer::new(
            vec![at(10.0, 2.0), at(10.0, 7.0), at(10.0, -4.0), at(10.0, 9.0)],
            0.0,
        );
        assert!(placer.near(at(10.0, 7.0)));
        placer.offer(vec![outline, courtyard], Some(7));
        let positions = placer.positions();
        // The bar and the pub belong to their building; the courtyard café and the terrace to none.
        assert_eq!(placer.hosts(), &[Some(7), None, None, Some(7)]);
        let metres = |p: (i32, i32)| (f64::from(p.0) / m, f64::from(p.1) / m);
        let bar = metres(positions[0]);
        assert!(
            (bar.0 - 10.0).abs() < 0.1 && (bar.1 + 1.5).abs() < 0.1,
            "{bar:?}"
        );
        assert_eq!(positions[1], at(10.0, 7.0));
        assert_eq!(positions[2], at(10.0, -4.0));
        let pub_ = metres(positions[3]);
        assert!(
            (pub_.0 - 10.0).abs() < 0.1 && (pub_.1 - 6.5).abs() < 0.1,
            "{pub_:?}"
        );
    }

    /// Three pavilions in a row, wall to wall, their fronts on a promenade at x = 0: a restaurant
    /// mapped 1 m from the middle one's party wall and 4 m from its front goes in front of the
    /// front, not into the neighbour; one in a pavilion walled in by others on every side goes in
    /// front of its nearest wall all the same.
    #[test]
    fn a_party_wall_is_not_a_way_out() {
        let m = 1.0 / Z30_QUANTUM_M;
        let at = |x: f64, y: f64| ((x * m) as i32, (y * m) as i32);
        let pavilion = |y: f64| vec![at(0.0, y), at(15.0, y), at(15.0, y + 8.0), at(0.0, y + 8.0)];
        let mut placer = OutsidePlacer::new(vec![at(4.0, 9.0)], 0.0);
        for y in [0.0, 8.0, 16.0] {
            placer.offer(vec![pavilion(y)], None);
        }
        let metres = |p: (i32, i32)| (f64::from(p.0) / m, f64::from(p.1) / m);
        let restaurant = metres(placer.positions()[0]);
        assert!(
            (restaurant.0 + 1.5).abs() < 0.1 && (restaurant.1 - 9.0).abs() < 0.1,
            "{restaurant:?}"
        );

        let square = |x: f64, y: f64| {
            vec![
                at(x, y),
                at(x + 8.0, y),
                at(x + 8.0, y + 8.0),
                at(x, y + 8.0),
            ]
        };
        let mut enclosed = OutsidePlacer::new(vec![at(4.0, 1.0)], 0.0);
        for (x, y) in [(0.0, 0.0), (-8.0, 0.0), (8.0, 0.0), (0.0, -8.0), (0.0, 8.0)] {
            enclosed.offer(vec![square(x, y)], None);
        }
        let walled_in = metres(enclosed.positions()[0]);
        assert!(
            (walled_in.0 - 4.0).abs() < 0.1 && (walled_in.1 + 1.5).abs() < 0.1,
            "{walled_in:?}"
        );
    }
}

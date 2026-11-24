//! The building around a known indoor point and the courtyards of its exterior ring.

use super::{Footprint, FootprintRing, Scene};
use tiles::obstacles::OutlineKind;

/// With one complete exterior, the containment walk already identifies the building.
pub enum BuildingParts {
    One(Footprint),
    Locate(Vec<Footprint>),
}

impl Scene<'_> {
    /// Separate buildings in one input record, with duplicate cross-tile rings counted once.
    pub fn building_parts(&self, id: u64) -> BuildingParts {
        let mut parts: Vec<Footprint> = Vec::new();
        let mut holes = Vec::new();
        for (_, tile) in self.read_tiles() {
            for index in tile.outlines_of(id) {
                let record = tile.obstacles.outline(index);
                let vertices = record.first_vertex..record.first_vertex + record.vertex_count;
                let ring = FootprintRing {
                    hole: record.kind == OutlineKind::Hole,
                    points: vertices
                        .map(|v| self.lattice.metres(tile.vertex(v)))
                        .collect(),
                };
                if ring.hole {
                    if !holes.contains(&ring) {
                        holes.push(ring);
                    }
                } else if record.kind == OutlineKind::Exterior
                    && !parts.iter().any(|part| part.rings[0] == ring)
                {
                    parts.push(Footprint {
                        id,
                        height_m: record.height_m,
                        envelope: record.envelope,
                        single_part: false,
                        rings: vec![ring],
                    });
                }
            }
        }
        let mut complete = true;
        for hole in holes {
            // A courtyard can contain another building. Its parent encloses the whole hole.
            let parent = parts
                .iter()
                .enumerate()
                .filter(|(_, part)| self.encloses_hole(&part.rings[0], &hole))
                .min_by(|(_, a), (_, b)| {
                    a.rings[0].twice_area().total_cmp(&b.rings[0].twice_area())
                })
                .map(|(index, _)| index);
            if let Some(parent) = parent {
                parts[parent].rings.push(hole);
            } else {
                complete = false;
            }
        }
        if parts.len() == 1 {
            parts[0].single_part = true;
        }
        if complete && parts.len() == 1 {
            BuildingParts::One(parts.pop().unwrap())
        } else {
            BuildingParts::Locate(parts)
        }
    }

    /// Whether `point` (click metres) lies inside `footprint`'s exterior ring and outside its holes.
    pub fn footprint_contains(&self, footprint: &Footprint, point: [f64; 2]) -> bool {
        let Some(outer) = footprint.rings.first() else {
            return false;
        };
        self.ring_contains(outer, point)
            && !footprint.rings[1..]
                .iter()
                .any(|hole| self.ring_contains(hole, point))
    }

    /// The containing exterior of a point already classified indoors under this record's ID.
    /// Within a courtyard an island can have the same ID; its own, smaller shell wins.
    pub fn part_at_indoor_point(&self, parts: &[Footprint], point: [f64; 2]) -> Option<usize> {
        parts
            .iter()
            .enumerate()
            .filter(|(_, part)| self.ring_contains(&part.rings[0], point))
            .min_by(|(_, a), (_, b)| a.rings[0].twice_area().total_cmp(&b.rings[0].twice_area()))
            .map(|(index, _)| index)
    }

    fn ring_contains(&self, ring: &FootprintRing, metres: [f64; 2]) -> bool {
        let point = self.lattice.steps(metres);
        let mut a = self.lattice.steps(ring.points[0]);
        let mut inside = false;
        for &metres in &ring.points[1..] {
            let b = self.lattice.steps(metres);
            if eastward_crossing_x(a, b, point).is_some() {
                inside = !inside;
            }
            a = b;
        }
        inside
    }

    fn encloses_hole(&self, outer: &FootprintRing, hole: &FootprintRing) -> bool {
        // Valid rings do not cross. At a shared vertex the adjacent edge midpoint is inside.
        let [mut west, mut south] = outer.points[0];
        let [mut east, mut north] = outer.points[0];
        for &[x, y] in &outer.points {
            west = west.min(x);
            east = east.max(x);
            south = south.min(y);
            north = north.max(y);
        }
        hole.points
            .iter()
            .all(|&[x, y]| x >= west && x <= east && y >= south && y <= north)
            && hole.points.windows(2).any(|edge| {
                self.ring_contains(
                    outer,
                    [
                        (edge[0][0] + edge[1][0]) * 0.5,
                        (edge[0][1] + edge[1][1]) * 0.5,
                    ],
                )
            })
    }
}

impl FootprintRing {
    fn twice_area(&self) -> f64 {
        let [x, y] = self.points[0];
        self.points
            .windows(2)
            .map(|pair| (pair[0][0] - x) * (pair[1][1] - y) - (pair[1][0] - x) * (pair[0][1] - y))
            .sum::<f64>()
            .abs()
    }
}

/// Intersection with the eastward half-line in south-increasing scene steps, including one
/// end of a vertical interval so a vertex crossing counts once.
pub(super) fn eastward_crossing_x(a: [f64; 2], b: [f64; 2], [x, y]: [f64; 2]) -> Option<f64> {
    if (a[1] > y) == (b[1] > y) {
        return None;
    }
    let crossing = a[0] + (y - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
    (crossing > x).then_some(crossing)
}

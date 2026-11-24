//! The building around a known indoor point and the courtyards of its exterior ring.

use super::{Footprint, FootprintRing, Scene};
use tiles::obstacles::OutlineKind;

/// With one complete exterior, the containment walk already identifies the building.
pub enum BuildingParts {
    One(Footprint),
    Locate(Vec<Footprint>),
}

impl BuildingParts {
    /// All loaded exterior parts of the record in one sliceable list.
    pub fn into_vec(self) -> Vec<Footprint> {
        match self {
            BuildingParts::One(footprint) => vec![footprint],
            BuildingParts::Locate(parts) => parts,
        }
    }
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

    /// Whether a same-id source at `point` (click metres) belongs to `footprint` among the
    /// currently loaded exteriors of `footprint.id`.
    pub fn footprint_owns_point(&self, footprint: &Footprint, point: [f64; 2]) -> bool {
        let parts = self.building_parts(footprint.id).into_vec();
        self.footprint_owns_point_among(&parts, footprint, point)
    }

    /// Same as [`Scene::footprint_owns_point`] with pre-collected `parts` of `footprint.id`:
    /// - A single-part record always owns its id's source (including concave or courtyard centroids).
    /// - Among multiple exteriors: first the smallest exterior whose solid area contains `point`;
    ///   else the smallest exterior whose outer ring encloses `point` (a courtyard hole); else the
    ///   exterior whose outer ring lies nearest to `point` (an emission centroid shifted slightly
    ///   from an adopted multi-part outline).
    pub fn footprint_owns_point_among(
        &self,
        parts: &[Footprint],
        footprint: &Footprint,
        point: [f64; 2],
    ) -> bool {
        let Some(target) = footprint.rings.first() else {
            return false;
        };
        if parts.len() <= 1 {
            return parts
                .first()
                .is_none_or(|only| only.rings.first() == Some(target));
        }
        let owner = parts
            .iter()
            .filter(|part| self.footprint_contains(part, point))
            .min_by(|a, b| cmp_part_canonical(a, b))
            .or_else(|| {
                parts
                    .iter()
                    .filter(|part| self.ring_contains(&part.rings[0], point))
                    .min_by(|a, b| cmp_part_canonical(a, b))
            })
            .or_else(|| {
                parts.iter().min_by(|a, b| {
                    ring_distance_sq(&a.rings[0], point)
                        .total_cmp(&ring_distance_sq(&b.rings[0], point))
                        .then_with(|| cmp_part_canonical(a, b))
                })
            });
        owner.is_some_and(|part| part.rings[0] == *target)
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

/// Deterministic tie-breaker for two parts of the same record: smaller outer area first, then
/// lexicographic outer ring vertices so tile load order never changes part ownership.
fn cmp_part_canonical(a: &Footprint, b: &Footprint) -> std::cmp::Ordering {
    a.rings[0]
        .twice_area()
        .total_cmp(&b.rings[0].twice_area())
        .then_with(|| a.rings[0].points.len().cmp(&b.rings[0].points.len()))
        .then_with(|| {
            for (pa, pb) in a.rings[0].points.iter().zip(&b.rings[0].points) {
                let ord = pa[0]
                    .total_cmp(&pb[0])
                    .then_with(|| pa[1].total_cmp(&pb[1]));
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
            }
            std::cmp::Ordering::Equal
        })
}

/// Minimum squared Euclidean distance (m²) from `point` to any edge of `ring`.
fn ring_distance_sq(ring: &FootprintRing, point: [f64; 2]) -> f64 {
    ring.points
        .windows(2)
        .map(|edge| {
            let (a, b) = (edge[0], edge[1]);
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let len_sq = dx * dx + dy * dy;
            let t = if len_sq > 0.0 {
                (((point[0] - a[0]) * dx + (point[1] - a[1]) * dy) / len_sq).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let (ex, ey) = (a[0] + t * dx - point[0], a[1] + t * dy - point[1]);
            ex * ex + ey * ey
        })
        .fold(f64::INFINITY, f64::min)
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

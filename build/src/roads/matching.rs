//! One spatial/ref/direction matcher with source precedence for every normalized road observation.

use super::{input::Road, model::Model, observation::Observation};
use anyhow::Result;
use rstar::{AABB, RTree, RTreeObject};

struct Candidate {
    index: usize,
    envelope: AABB<[f64; 2]>,
}
impl RTreeObject for Candidate {
    type Envelope = AABB<[f64; 2]>;
    fn envelope(&self) -> Self::Envelope {
        self.envelope
    }
}

pub struct Matcher {
    observations: Vec<Observation>,
    tree: RTree<Candidate>,
}

fn longitude_delta(value: f64) -> f64 {
    if value > 180.0 {
        value - 360.0
    } else if value < -180.0 {
        value + 360.0
    } else {
        value
    }
}

fn flat_distance(point: [f64; 2], other: [f64; 2]) -> f64 {
    let cosine = ((point[1] + other[1]) / 2.0 * std::f64::consts::PI / 180.0).cos();
    (longitude_delta(other[0] - point[0]) * 111_320.0 * cosine)
        .hypot((other[1] - point[1]) * 110_540.0)
}

fn segment_distance(point: [f64; 2], start: [f64; 2], end: [f64; 2]) -> f64 {
    // These local metric coefficients are the original count matcher's degree approximation.
    let longitude_scale = 111_320.0 * (point[1] * std::f64::consts::PI / 180.0).cos();
    let px = longitude_delta(point[0] - start[0]) * longitude_scale;
    let py = (point[1] - start[1]) * 110_540.0;
    let ex = longitude_delta(end[0] - start[0]) * longitude_scale;
    let ey = (end[1] - start[1]) * 110_540.0;
    let squared = ex * ex + ey * ey;
    if squared < 1e-6 {
        return flat_distance(point, start);
    }
    let fraction = ((px * ex + py * ey) / squared).clamp(0.0, 1.0);
    (px - fraction * ex).hypot(py - fraction * ey)
}

fn road_midpoint(road: &Road) -> [f64; 2] {
    let start = crate::osm::geometry::poly::meters_to_lonlat(road.start.0, road.start.1);
    let end = crate::osm::geometry::poly::meters_to_lonlat(road.end.0, road.end.1);
    [
        (start.0 + longitude_delta(end.0 - start.0) / 2.0 + 180.0).rem_euclid(360.0) - 180.0,
        (start.1 + end.1) / 2.0,
    ]
}

pub fn canonical_reference(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_uppercase)
        .collect()
}

impl Matcher {
    pub fn has_measurements(&self) -> bool {
        self.observations.iter().any(|r| r.measured)
    }
    pub fn new(observations: Vec<Observation>) -> Self {
        let mut candidates = Vec::new();
        for (index, row) in observations.iter().enumerate() {
            for part in &row.geometry {
                let origin = part[0][0];
                let west = part
                    .iter()
                    .map(|p| origin + longitude_delta(p[0] - origin))
                    .fold(f64::INFINITY, f64::min);
                let east = part
                    .iter()
                    .map(|p| origin + longitude_delta(p[0] - origin))
                    .fold(f64::NEG_INFINITY, f64::max);
                let south = part.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min)
                    - row.search_radius_m / 110_540.0;
                let north = part.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max)
                    + row.search_radius_m / 110_540.0;
                let latitude = south.abs().max(north.abs()).min(90.0);
                let reach =
                    (row.search_radius_m / (111_320.0 * latitude.to_radians().cos())).min(360.0);
                for shift in [-360.0, 0.0, 360.0] {
                    candidates.push(Candidate {
                        index,
                        envelope: AABB::from_corners(
                            [west - reach + shift, south],
                            [east + reach + shift, north],
                        ),
                    });
                }
            }
        }
        Self {
            observations,
            tree: RTree::bulk_load(candidates),
        }
    }

    pub fn apply(&self, road: &mut Road, model: &Model) -> Result<()> {
        let midpoint = road_midpoint(road);
        let references = road
            .reference
            .split(';')
            .map(canonical_reference)
            .collect::<Vec<_>>();
        let mut candidates = self
            .tree
            .locate_in_envelope_intersecting(&AABB::from_point(midpoint))
            .map(|c| c.index)
            .collect::<Vec<_>>();
        candidates.sort_unstable();
        candidates.dedup();
        let mut best: Option<(usize, f64)> = None;
        for index in candidates {
            let row = &self.observations[index];
            if row.country != 0 && row.country != road.country.0
                || row.road_class_mask & (1 << road.class) == 0
                || row.reference_required
                    && !row
                        .reference
                        .split(';')
                        .any(|reference| references.contains(&canonical_reference(reference)))
            {
                continue;
            }
            let mut distance = row.search_radius_m;
            for part in &row.geometry {
                if part.len() == 1 {
                    distance = distance.min(flat_distance(midpoint, part[0]));
                }
                for pair in part.windows(2) {
                    if row.direction != 0 && road.direction != 0 {
                        let travel = road.travel_vector();
                        let sign = if row.direction == 2 { -1.0 } else { 1.0 };
                        let along = longitude_delta(pair[1][0] - pair[0][0])
                            * midpoint[1].to_radians().cos()
                            * travel.0
                            + (pair[1][1] - pair[0][1]) * travel.1;
                        if sign * along <= 0.0 {
                            continue;
                        }
                    }
                    distance = distance.min(segment_distance(midpoint, pair[0], pair[1]));
                }
            }
            if distance >= row.search_radius_m {
                continue;
            }
            if best.is_none_or(|(previous, previous_distance)| {
                row.source_rank > self.observations[previous].source_rank
                    || row.source_rank == self.observations[previous].source_rank
                        && distance < previous_distance
            }) {
                best = Some((index, distance));
            }
        }
        if let Some((index, _)) = best {
            let row = &self.observations[index];
            let (counts, estimated) = row.daily_classes(model.world[usize::from(road.class)])?;
            road.counts = counts;
            road.estimated = estimated;
            road.source_id = row.source_id;
            road.observation_source_id = row.source_id;
            road.observation = row.identity.clone();
            road.basis = row.count_basis as u8;
            road.measured = row.measured;
        }
        Ok(())
    }
}

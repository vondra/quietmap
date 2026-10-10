//! One source-receiver ray: its sampled ground and obstacle crossings become a CNOSSOS vertical
//! path (the source platform, building roofs raised as hard ground, every crossing a candidate
//! top), and the path's linear transfer 10^(-A/10) per period and band follows from both
//! meteorological states mixed with the period's favourable probability (2.5.9) and air
//! absorption at the slant distance. Divergence and the receiver reflection are the caller's.
//! CNOSSOS-EU has no foliage term: dev4's ISO 9613-2 Annex A (informative, for foliage dense enough
//! to block the view) took 11-13 dB from a motorway 320 m behind a forest the owner hears clearly.

use crate::bands::{BANDS, PERIODS};
use crate::cnossos::{
    PlanePoint, StateBoundary, VerticalPath, VerticalPathScratch, VerticalProfile, state_boundaries,
};
use crate::profile::Profile;

/// Where a ray meets a wall of a building or a barrier.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crossing {
    /// Fraction of the horizontal source-receiver distance, in (0, 1).
    pub t: f64,
    /// Height above the local ground (m).
    pub height_m: f64,
    /// A building wall (its crossings pair into roofs) rather than a barrier.
    pub building: bool,
    /// The global footprint id: the two walls of one building share it.
    pub footprint_id: u64,
}

/// The ends of one ray.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayEnds {
    /// Source height above the ground under it (m).
    pub source_height_m: f64,
    pub receiver_altitude_m: f64,
    /// Gs of (2.5.14); `None` takes the ground factor under the source (point sources).
    pub source_ground_factor: Option<f64>,
    /// Within this distance of the source the terrain may not rise above the source ground (the
    /// platform rule for line sources; 0 for points).
    pub platform_half_width_m: f64,
    /// The source's own building: its walls and roof never screen the source. 0 for none.
    pub own_footprint: u64,
}

/// A ray's transfer: linear 10^(-A/10) per period and band (A without divergence), the
/// periods' mix of the two meteorological states and each state alone (homogeneous,
/// favourable), so that the time a source is heard can follow the weather.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transfer {
    pub slant_m: f64,
    pub periods: [[f64; BANDS]; PERIODS],
    pub states: [[f64; BANDS]; 2],
}

/// Reusable per-thread buffers.
#[derive(Default)]
pub struct RayScratch {
    path: PathBuffers,
    vertical: VerticalPathScratch,
}

/// One ray's vertical path.
#[derive(Default)]
struct PathBuffers {
    distance_m: Vec<f64>,
    altitude_m: Vec<f64>,
    ground: Vec<f64>,
    terrain: Vec<PlanePoint>,
    tops: Vec<PlanePoint>,
    roofs: Vec<Roof>,
    buildings: Vec<(u64, f64, f64)>,
    /// Footprints containing a building's source (its own among them).
    containing: Vec<u64>,
}

/// One roof span along the ray: from the wall at `x0` (top `top0`) to the wall at `x1`.
#[derive(Debug, Clone, Copy)]
struct Roof {
    x0: f64,
    x1: f64,
    top0: f64,
    top1: f64,
}

/// The terms of one ray: the transfer with the boundary of each state (homogeneous, favourable),
/// the air absorption behind it, and what blocks its line of sight in calm air.
pub struct RayTerms {
    pub transfer: Transfer,
    pub favourable_probability: [f64; PERIODS],
    pub boundaries: [StateBoundary; 2],
    pub air_db: [f64; BANDS],
    pub calm_edge: Edge,
}

/// What a ray's sound bends over in calm air: nothing (a free line of sight), the top of a
/// building or wall among the edges, or terrain alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Open,
    Obstacle,
    Terrain,
}

/// The terms of one ray; `favourable_probability` is p of each period for this ray's direction,
/// `alpha_db_per_km` the air absorption of the place per band.
pub fn ray_terms(
    profile: &Profile,
    crossings: &[Crossing],
    ends: &RayEnds,
    (favourable_probability, alpha_db_per_km): ([f64; PERIODS], &[f64; BANDS]),
    scratch: &mut RayScratch,
) -> RayTerms {
    let horizontal = profile.horizontal_m.max(1.0);
    let source_altitude = profile.ground_m[0] + ends.source_height_m;
    let slant = horizontal
        .hypot(ends.receiver_altitude_m - source_altitude)
        .max(1.0);
    scratch.path.fill(profile, crossings, ends);
    let boundaries = state_boundaries(
        &scratch.path.path(profile, ends, source_altitude),
        &mut scratch.vertical,
    );
    let calm = &scratch.vertical.calm_path;
    let calm_edge = if !calm.blocked {
        Edge::Open
    } else if calm
        .points
        .iter()
        .any(|point| scratch.path.tops.contains(point))
    {
        Edge::Obstacle
    } else {
        Edge::Terrain
    };
    let air_db: [f64; BANDS] = std::array::from_fn(|band| alpha_db_per_km[band] * slant / 1000.0);
    let air: [f64; BANDS] = air_db.map(attenuation_energy);
    let homogeneous: [f64; BANDS] =
        std::array::from_fn(|band| attenuation_energy(boundaries[0].attenuation_db[band]));
    let favourable: [f64; BANDS] =
        std::array::from_fn(|band| attenuation_energy(boundaries[1].attenuation_db[band]));
    RayTerms {
        transfer: Transfer {
            slant_m: slant,
            periods: std::array::from_fn(|period| {
                let p = favourable_probability[period];
                std::array::from_fn(|band| {
                    air[band] * (p * favourable[band] + (1.0 - p) * homogeneous[band])
                })
            }),
            states: [homogeneous, favourable]
                .map(|state| std::array::from_fn(|band| air[band] * state[band])),
        },
        favourable_probability,
        boundaries,
        air_db,
        calm_edge,
    }
}

fn attenuation_energy(attenuation_db: f64) -> f64 {
    (-attenuation_db * (std::f64::consts::LN_10 / 10.0)).exp()
}

/// The footprints containing a building's source, sorted, into `out`: its own (`own_footprint`, 0
/// for a source that is no building's: none) and every footprint the ray crosses an odd number of
/// times (the receiver stands outside every enclosed footprint, so the ray leaves it once more than
/// it enters). They never screen the source.
pub fn containing_footprints(crossings: &[Crossing], own_footprint: u64, out: &mut Vec<u64>) {
    out.clear();
    if own_footprint == 0 {
        return;
    }
    out.extend(
        crossings
            .iter()
            .filter(|c| c.building)
            .map(|c| c.footprint_id),
    );
    out.sort_unstable();
    let (mut odd, mut start) = (0, 0);
    while start < out.len() {
        let id = out[start];
        let end = start + out[start..].partition_point(|&other| other == id);
        if (end - start) % 2 == 1 {
            out[odd] = id;
            odd += 1;
        }
        start = end;
    }
    out.truncate(odd);
    out.push(own_footprint);
    out.sort_unstable();
}

impl PathBuffers {
    fn path<'a>(
        &'a self,
        profile: &Profile,
        ends: &RayEnds,
        source_altitude: f64,
    ) -> VerticalPath<'a> {
        let length = self.distance_m[self.distance_m.len() - 1];
        VerticalPath {
            profile: VerticalProfile {
                distance_m: &self.distance_m,
                altitude_m: &self.altitude_m,
                ground_factor: &self.ground,
            },
            source: (0.0, source_altitude),
            receiver: (length, ends.receiver_altitude_m),
            source_ground_factor: ends
                .source_ground_factor
                .unwrap_or(profile.ground_factor[0]),
            terrain_candidates: &self.terrain,
            obstacle_tops: &self.tops,
        }
    }

    /// Fills the vertical path: terrain samples (flattened to the source ground within the
    /// platform), every crossing but the source's own building as a top, building crossings
    /// paired into roofs.
    fn fill(&mut self, profile: &Profile, crossings: &[Crossing], ends: &RayEnds) {
        let length = profile.horizontal_m.max(1.0);
        let terrain = &mut self.terrain;
        terrain.clear();
        let source_ground = profile.ground_m[0];
        for (&t, &z) in profile.t.iter().zip(&profile.ground_m) {
            let x = t * length;
            terrain.push((
                x,
                if x < ends.platform_half_width_m {
                    z.min(source_ground)
                } else {
                    z
                },
            ));
        }
        let terrain_at = |x: f64| interpolate(terrain, x);
        let ground_factor_at = |x: f64| {
            let upper = terrain
                .partition_point(|v| v.0 <= x)
                .clamp(1, terrain.len() - 1);
            let (x0, x1) = (terrain[upper - 1].0, terrain[upper].0);
            let f = if x1 > x0 {
                ((x - x0) / (x1 - x0)).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let (a, b) = (
                profile.ground_factor[upper - 1],
                profile.ground_factor[upper],
            );
            a + f * (b - a)
        };
        self.tops.clear();
        self.roofs.clear();
        self.buildings.clear();
        // A building's source sits inside its footprint, sometimes inside another outline of the
        // same building too (a part mapped twice). The receiver stands outside every enclosed
        // footprint, so a footprint the ray crosses an odd number of times contains the source:
        // it is the source's own and does not screen it.
        let mut containing = std::mem::take(&mut self.containing);
        containing_footprints(crossings, ends.own_footprint, &mut containing);
        for crossing in crossings
            .iter()
            .filter(|c| containing.binary_search(&c.footprint_id).is_err())
        {
            let x = crossing.t * length;
            let top = terrain_at(x) + crossing.height_m;
            self.tops.push((x, top));
            if crossing.building {
                self.buildings.push((crossing.footprint_id, x, top));
            }
        }
        // A footprint's crossings alternate entry and exit along the ray: consecutive pairs are its
        // roofs, an unpaired last crossing (the ray ends inside it) has none.
        self.buildings
            .sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
        for footprint in self.buildings.chunk_by(|a, b| a.0 == b.0) {
            for pair in footprint.chunks_exact(2) {
                self.roofs.push(Roof {
                    x0: pair[0].1,
                    x1: pair[1].1,
                    top0: pair[0].2,
                    top1: pair[1].2,
                });
            }
        }
        self.containing = containing;
        clip_roofs_in_closing_order(&mut self.roofs);
        let (d, z, g) = (&mut self.distance_m, &mut self.altitude_m, &mut self.ground);
        d.clear();
        z.clear();
        g.clear();
        let mut push = |x: f64, altitude: f64, ground: f64| {
            d.push(x);
            z.push(altitude);
            g.push(ground);
        };
        push(0.0, terrain[0].1, profile.ground_factor[0]);
        let mut roofs = self.roofs.iter().peekable();
        let mut covered_to = 0.0;
        for (index, &(x, altitude)) in terrain.iter().enumerate().skip(1) {
            while let Some(roof) = roofs.peek() {
                if roof.x0 >= x {
                    break;
                }
                push(roof.x0, terrain_at(roof.x0), ground_factor_at(roof.x0));
                push(roof.x0, roof.top0, 0.0);
                push(roof.x1, roof.top1, 0.0);
                push(roof.x1, terrain_at(roof.x1), ground_factor_at(roof.x1));
                covered_to = roof.x1;
                roofs.next();
            }
            if x > covered_to {
                push(x, altitude, profile.ground_factor[index]);
            }
        }
    }
}

/// Roofs taken in the order the ray leaves them, each starting no earlier than where the roofs
/// left before it end: overlapping footprints never stack.
fn clip_roofs_in_closing_order(roofs: &mut Vec<Roof>) {
    roofs.sort_by(|a, b| a.x1.total_cmp(&b.x1).then(a.x0.total_cmp(&b.x0)));
    let mut covered_to = f64::NEG_INFINITY;
    roofs.retain_mut(|roof| {
        if roof.x1 <= covered_to {
            return false;
        }
        if roof.x0 < covered_to {
            let span = roof.x1 - roof.x0;
            roof.top0 += (covered_to - roof.x0) / span * (roof.top1 - roof.top0);
            roof.x0 = covered_to;
        }
        covered_to = roof.x1;
        true
    });
}

fn interpolate(points: &[PlanePoint], at: f64) -> f64 {
    let upper = points
        .partition_point(|v| v.0 <= at)
        .clamp(1, points.len() - 1);
    let ((x0, y0), (x1, y1)) = (points[upper - 1], points[upper]);
    let f = if x1 > x0 {
        ((at - x0) / (x1 - x0)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    y0 + f * (y1 - y0)
}

#[cfg(test)]
#[path = "ray_tests.rs"]
mod tests;

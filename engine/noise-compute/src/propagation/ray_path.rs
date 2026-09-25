//! One sampled source→receiver ray as a CNOSSOS vertical path: the bare-earth profile with the
//! source platform, building roofs raised as hard ground and every crossing a candidate top.

use super::cnossos::rubber_band::PlanePoint;
use super::cnossos::{VerticalPath, VerticalProfile};
use super::obstacle_index::{CrossingCandidate, ObstacleKind};
use super::PathProfile;

/// What a ray needs beyond its profile and crossings.
#[derive(Debug, Clone, Copy)]
pub struct RayPathInputs {
    pub source_altitude_m: f64,
    pub receiver_altitude_m: f64,
    /// Gs of (2.5.14).
    pub source_ground_factor: f64,
    /// Within this distance of the source the terrain may not rise above the source ground
    /// (METHOD.md §2.2 platform rule; 0 for point sources).
    pub platform_half_width_m: f64,
    /// Building crossings nearer the source than this are its own footprint.
    pub exclusion_radius_m: f64,
}

/// Reusable buffers holding one ray's vertical path.
#[derive(Default)]
pub struct RayPathBuffers {
    distance_m: Vec<f64>,
    altitude_m: Vec<f64>,
    ground: Vec<f64>,
    terrain: Vec<PlanePoint>,
    tops: Vec<PlanePoint>,
    roofs: Vec<Roof>,
    buildings: Vec<(u16, u32, f64, f64)>,
}

/// One roof span along the ray: from the wall at `x0` (top `top0`) to the wall at `x1`.
#[derive(Debug, Clone, Copy)]
struct Roof {
    x0: f64,
    x1: f64,
    top0: f64,
    top1: f64,
}

impl RayPathBuffers {
    /// Fills the buffers from `profile` and its `crossings` (t-sorted); `with_obstacles` false
    /// leaves out every building and wall (the popup's "no screening" hypothesis).
    pub fn fill(
        &mut self,
        profile: &PathProfile,
        crossings: &[CrossingCandidate],
        inputs: &RayPathInputs,
        with_obstacles: bool,
    ) {
        let length = profile.dist_m;
        let terrain = &mut self.terrain;
        terrain.clear();
        let source_ground = f64::from(profile.elevation_m[0]);
        for (&t, &z) in profile.t.iter().zip(&profile.elevation_m) {
            let x = t * length;
            let z = f64::from(z);
            terrain.push((x, if x < inputs.platform_half_width_m { z.min(source_ground) } else { z }));
        }
        let terrain_at = |x: f64| interpolate(terrain, x);
        let ground_factor_at = |x: f64| {
            let upper = terrain.partition_point(|v| v.0 <= x).clamp(1, terrain.len() - 1);
            let (x0, x1) = (terrain[upper - 1].0, terrain[upper].0);
            let f = if x1 > x0 { ((x - x0) / (x1 - x0)).clamp(0.0, 1.0) } else { 0.0 };
            let (a, b) = (f64::from(profile.imd_u8[upper - 1].min(100)), f64::from(profile.imd_u8[upper].min(100)));
            1.0 - (a + f * (b - a)) / 100.0
        };
        self.tops.clear();
        self.roofs.clear();
        self.buildings.clear();
        if with_obstacles {
            for crossing in crossings {
                let x = crossing.t * length;
                let building = crossing.kind == ObstacleKind::Building;
                if building && x < inputs.exclusion_radius_m {
                    continue;
                }
                let top = terrain_at(x) + f64::from(crossing.height_m);
                self.tops.push((x, top));
                if building {
                    self.buildings.push((crossing.index, crossing.id, x, top));
                }
            }
            // A footprint's crossings alternate entry and exit along the ray: consecutive pairs
            // are its roofs, an unpaired last crossing (the ray ends inside it) has none.
            self.buildings.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)).then(a.2.total_cmp(&b.2)));
            for footprint in self.buildings.chunk_by(|a, b| (a.0, a.1) == (b.0, b.1)) {
                for pair in footprint.chunks_exact(2) {
                    self.roofs.push(Roof { x0: pair[0].2, x1: pair[1].2, top0: pair[0].3, top1: pair[1].3 });
                }
            }
            clip_roofs_in_closing_order(&mut self.roofs);
        }
        let (d, z, g) = (&mut self.distance_m, &mut self.altitude_m, &mut self.ground);
        d.clear();
        z.clear();
        g.clear();
        let mut push = |x: f64, altitude: f64, ground: f64| {
            d.push(x);
            z.push(altitude);
            g.push(ground);
        };
        push(0.0, terrain[0].1, ground_factor_at(0.0));
        let mut roofs = self.roofs.iter().peekable();
        let mut covered_to = 0.0;
        for &(x, altitude) in &terrain[1..] {
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
                push(x, altitude, ground_factor_at(x));
            }
        }
    }

    /// The filled ray as a vertical path; `terrain_diffracts` false leaves the terrain out of the
    /// candidates (the popup's "no terrain" hypothesis).
    pub fn path(&self, inputs: &RayPathInputs, terrain_diffracts: bool) -> VerticalPath<'_> {
        let length = self.distance_m[self.distance_m.len() - 1];
        VerticalPath {
            profile: VerticalProfile {
                distance_m: &self.distance_m,
                altitude_m: &self.altitude_m,
                ground_factor: &self.ground,
            },
            source: (0.0, inputs.source_altitude_m),
            receiver: (length, inputs.receiver_altitude_m),
            source_ground_factor: inputs.source_ground_factor,
            terrain_candidates: if terrain_diffracts { &self.terrain } else { &[] },
            obstacle_tops: &self.tops,
        }
    }
}

/// Roofs taken in the order the ray leaves them (their exit wall), each starting no earlier than
/// where the roofs left before it end: overlapping footprints (0.4 % of roof length on the oracle's
/// real rays, 2026-09-24) never stack, and a streaming scan can apply the rule as it goes.
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
    let upper = points.partition_point(|v| v.0 <= at).clamp(1, points.len() - 1);
    let ((x0, y0), (x1, y1)) = (points[upper - 1], points[upper]);
    let f = if x1 > x0 { ((at - x0) / (x1 - x0)).clamp(0.0, 1.0) } else { 0.0 };
    y0 + f * (y1 - y0)
}

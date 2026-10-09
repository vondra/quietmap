//! The receiver's horizons for aircraft screening (dev4 `horizon.rs`, `screening.rs`): per
//! direction sector and range band the steepest terrain and roof edge seen from the receiver, from
//! which an aircraft behind an edge loses its single-edge path-difference loss
//! (`physics::doc29::screening`). Built once per click, unquantized.

use crate::obstacles::Scene;
use crate::scene::Ground;
use physics::doc29::screening::{ReceiverHorizons, edge_loss_db};
use physics::ray::Crossing;
use std::f64::consts::TAU;

/// Terrain: 32 sectors of 11.25 deg; per sector the steepest edge within each of six range
/// bands, sampled at 48 points from 30 m to 8 km, growing geometrically (dev4).
const TERRAIN_SECTORS: usize = 32;
const TERRAIN_BAND_LIMITS_M: [f64; 6] = [500.0, 1_000.0, 2_000.0, 3_500.0, 5_500.0, 8_000.0];
const TERRAIN_SAMPLES: usize = 48;
const TERRAIN_FIRST_SAMPLE_M: f64 = 30.0;
/// Buildings: 256 sectors of 1.41 deg; bands doubling from 16 m to 512 m (dev4).
const BUILDING_SECTORS: usize = 256;
const BUILDING_BAND_LIMITS_M: [f64; 6] = [16.0, 32.0, 64.0, 128.0, 256.0, 512.0];
/// A crossing at the receiver itself is its footprint's boundary, not an edge (dev4).
const BUILDING_NEAREST_EDGE_M: f64 = 0.01;

/// The steepest edge of one band: range (m) and tangent of its elevation from the receiver.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Edge {
    range_m: f64,
    tangent: f64,
}

/// Per sector, the steepest edge of each band (`None`: the band holds nothing sampled), and the
/// steepest tangent of each sector and of all: a point seen above it is screened by nothing.
#[derive(Debug, Clone, PartialEq)]
struct SectorEdges<const BANDS: usize> {
    sectors: Vec<[Option<Edge>; BANDS]>,
    steepest: Vec<f64>,
    steepest_of_all: f64,
}

impl<const BANDS: usize> SectorEdges<BANDS> {
    fn new(sectors: usize) -> Self {
        SectorEdges {
            sectors: vec![[None; BANDS]; sectors],
            steepest: vec![f64::NEG_INFINITY; sectors],
            steepest_of_all: f64::NEG_INFINITY,
        }
    }

    fn offer(&mut self, sector: usize, band: usize, edge: Edge) {
        let slot = &mut self.sectors[sector][band];
        if slot.is_none_or(|kept| edge.tangent > kept.tangent) {
            *slot = Some(edge);
        }
        self.steepest[sector] = self.steepest[sector].max(edge.tangent);
        self.steepest_of_all = self.steepest_of_all.max(edge.tangent);
    }

    /// The largest edge loss toward `point_m` (east, north, height above the receiver) over the
    /// edges of its sector nearer than it.
    fn loss_db(&self, point_m: [f64; 3]) -> f64 {
        let range_m = point_m[0].hypot(point_m[1]);
        if range_m <= 1.0 || point_m[2] >= self.steepest_of_all * range_m {
            return 0.0;
        }
        let sector = sector_of(point_m, self.sectors.len());
        if point_m[2] >= self.steepest[sector] * range_m {
            return 0.0;
        }
        self.sectors[sector]
            .iter()
            .flatten()
            .map(|edge| {
                edge_loss_db(
                    edge.range_m,
                    edge.tangent * edge.range_m,
                    range_m,
                    point_m[2],
                )
            })
            .fold(0.0, f64::max)
    }
}

/// The sector (anticlockwise from east) holding a direction.
fn sector_of(point_m: [f64; 3], sectors: usize) -> usize {
    let angle = point_m[1].atan2(point_m[0]).rem_euclid(TAU);
    ((angle / TAU * sectors as f64) as usize).min(sectors - 1)
}

/// The unit direction of a sector's centre.
fn sector_direction(sector: usize, sectors: usize) -> [f64; 2] {
    let (sin, cos) = ((sector as f64 + 0.5) * TAU / sectors as f64).sin_cos();
    [cos, sin]
}

fn band_of(range_m: f64, limits: &[f64]) -> usize {
    limits
        .iter()
        .position(|&limit| range_m <= limit + 0.5)
        .unwrap_or(limits.len() - 1)
}

/// Both horizons of one receiver.
pub struct Horizons {
    terrain: SectorEdges<6>,
    buildings: SectorEdges<6>,
}

impl Horizons {
    /// The horizons of the receiver at `position` (click metres) and `altitude_m`, from the terrain
    /// and buildings read so far: samples in tiles not yet read are skipped, so a first answer's
    /// horizon may lack far edges (edges only screen what lies behind them). The walls of
    /// `own_footprint`, the building a painted point stands in (0 for none), are no horizon.
    pub fn build(
        ground: &Ground<'_>,
        obstacles: &Scene<'_>,
        position: [f64; 2],
        altitude_m: f64,
        own_footprint: u64,
    ) -> Result<Self, String> {
        let mut terrain = SectorEdges::new(TERRAIN_SECTORS);
        let growth = (TERRAIN_BAND_LIMITS_M[5] / TERRAIN_FIRST_SAMPLE_M)
            .powf(1.0 / (TERRAIN_SAMPLES - 1) as f64);
        for sector in 0..TERRAIN_SECTORS {
            let [east, north] = sector_direction(sector, TERRAIN_SECTORS);
            let mut range_m = TERRAIN_FIRST_SAMPLE_M;
            for _ in 0..TERRAIN_SAMPLES {
                let point = [position[0] + east * range_m, position[1] + north * range_m];
                if let Some(height_m) = ground.read_height_m(point) {
                    terrain.offer(
                        sector,
                        band_of(range_m, &TERRAIN_BAND_LIMITS_M),
                        Edge {
                            range_m,
                            tangent: (height_m - altitude_m) / range_m,
                        },
                    );
                }
                range_m *= growth;
            }
        }
        let mut buildings = SectorEdges::new(BUILDING_SECTORS);
        let mut crossings: Vec<Crossing> = Vec::new();
        let reach_m = BUILDING_BAND_LIMITS_M[5];
        for sector in 0..BUILDING_SECTORS {
            let [east, north] = sector_direction(sector, BUILDING_SECTORS);
            let end = [position[0] + east * reach_m, position[1] + north * reach_m];
            obstacles.crossings(position, end, &mut crossings)?;
            for crossing in crossings
                .iter()
                .filter(|crossing| crossing.building && crossing.footprint_id != own_footprint)
            {
                let range_m = crossing.t * reach_m;
                if range_m <= BUILDING_NEAREST_EDGE_M {
                    continue;
                }
                let point = [position[0] + east * range_m, position[1] + north * range_m];
                let roof_m = ground.at(point)?.height_m + crossing.height_m;
                buildings.offer(
                    sector,
                    band_of(range_m, &BUILDING_BAND_LIMITS_M),
                    Edge {
                        range_m,
                        tangent: (roof_m - altitude_m) / range_m,
                    },
                );
            }
        }
        Ok(Horizons { terrain, buildings })
    }
}

impl ReceiverHorizons for Horizons {
    fn terrain_loss_db(&self, point_m: [f64; 3]) -> f64 {
        self.terrain.loss_db(point_m)
    }

    fn building_loss_db(&self, point_m: [f64; 3]) -> f64 {
        self.buildings.loss_db(point_m)
    }
}

#[cfg(test)]
#[path = "horizons_tests.rs"]
mod tests;

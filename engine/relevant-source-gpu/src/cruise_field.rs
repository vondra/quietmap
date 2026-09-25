//! Manifest-bound cruise mean powers on one global lattice, with exact near-field replacement.
use crate::{
    input_manifest::InputManifest,
    surface_scene::{scene_bounds, SurfaceScene},
    tile_receivers::TileReceivers,
};
use anyhow::{ensure, Context, Result};
use arrow::{ipc::reader::FileReader, record_batch::RecordBatch};
use grid::{bounds::BoundedSquares, Square};
use noise_compute::{
    compute::aircraft_v6::cruise::cruise_segment,
    constants::DEFAULT_RECEIVER_HEIGHT,
    emission::aircraft::{self, SegmentPrepared, SegmentTerrain},
    types::RasterSampler,
};
use raster_reader::RealRasters;
use rayon::prelude::*;
use source_reader::aircraft_v6::{assert_cruise_contract, CruiseRowAccum};
use std::{collections::BTreeSet, io::Cursor, path::Path};

#[path = "cruise_lattice.rs"]
mod lattice;
#[cfg(feature = "gpu")]
#[path = "cruise_gpu.rs"]
pub(crate) mod gpu;
use lattice::Lattice;

#[derive(Clone)]
struct Bucket {
    prepared: SegmentPrepared,
    lat: f64,
    lon: f64,
    half_length: f64,
    density: f64,
    period: usize,
}
impl Bucket {
    #[cfg(not(feature = "gpu"))]
    fn energy(&self, lat: f64, lon: f64, altitude: f64, npd: &aircraft::NpdLuts) -> f64 {
        let row = aircraft::prepare_row(
            &self.prepared,
            lat,
            aircraft::M_PER_DEG_LAT * lat.to_radians().cos().max(0.2),
        );
        self.energy_at_row(lat, lon, altitude, npd, &row)
    }
    #[cfg(not(feature = "gpu"))]
    fn energy_at_row(
        &self,
        lat: f64,
        lon: f64,
        altitude: f64,
        npd: &aircraft::NpdLuts,
        row: &aircraft::SegmentRowState,
    ) -> f64 {
        let north = (self.lat - lat) * aircraft::M_PER_DEG_LAT;
        let east =
            grid::geo::wrapped_longitude_delta(lon, self.lon) * grid::geo::m_per_deg_lon(lat.to_radians());
        if north * north + east * east
            > (aircraft::AIRCRAFT_MAX_HORIZONTAL_REACH_M + self.half_length).powi(2)
        {
            return 0.0;
        }
        aircraft::segment_sel_at_pixel_energy(&self.prepared, row, lon, altitude, npd, None).map_or(
            0.0,
            |sel| {
                noise_compute::propagation::iso9613::fast_exp_f64(
                    sel * std::f64::consts::LN_10 * 0.1,
                ) * self.density
            },
        )
    }
}

struct Group {
    bounds: [f64; 4],
    half_length: f64,
    buckets: Vec<Bucket>,
}
impl Group {
    #[cfg(not(feature = "gpu"))]
    fn reaches(&self, lat: f64, lon: f64) -> bool {
        let middle = (self.bounds[1] + self.bounds[3]) * 0.5;
        let lon = middle + grid::geo::wrapped_longitude_delta(middle, lon);
        let north =
            (lat - lat.clamp(self.bounds[0], self.bounds[2])) * aircraft::M_PER_DEG_LAT;
        let east =
            (lon - lon.clamp(self.bounds[1], self.bounds[3]))
                * grid::geo::m_per_deg_lon(lat.to_radians());
        north * north + east * east
            <= (aircraft::AIRCRAFT_MAX_HORIZONTAL_REACH_M + self.half_length).powi(2)
    }
}

pub struct CruiseField {
    owner: Square,
    lattice: Lattice,
    groups: Vec<Group>,
    altitudes: Vec<f64>,
    energies: Vec<[f64; 3]>,
    days: u16,
}
impl CruiseField {
    pub fn load(
        owner: Square,
        prepared: &Path,
        manifest: &InputManifest,
        rasters: &RealRasters,
    ) -> Result<Self> {
        ensure!(
            owner.x < grid::Z9_TILES_PER_AXIS && owner.y < grid::Z9_TILES_PER_AXIS,
            "invalid cruise field owner"
        );
        let lattice = Lattice::new(scene_bounds(owner));
        let [north, west] = lattice.point(0, 0);
        let [south, _] = lattice.point(0, lattice.height - 1);
        let east = west + (lattice.width - 1) as f64 / lattice.axis as f64 * 360.0;
        let (dy, dx) = grid::geo::reach_box_half_extents_deg(
            south.abs().max(north.abs()),
            aircraft::CRUISE_QUERY_RADIUS_M,
        );
        let squares = BoundedSquares::from_degrees(
            (south - dy).next_down(),
            (west - dx).next_down(),
            (north + dy).next_up(),
            (east + dx).next_up(),
        )
        .context("invalid cruise support")?;
        let mut groups = Vec::new();
        let mut window: Option<aircraft::SamplingWindow> = None;
        let mut owners: Vec<_> = squares.iter().collect();
        owners.sort_by_key(|square| (square.y, square.x));
        for square in owners {
            let relative = format!("z9/{}/{}/cruise.arrow", square.x, square.y);
            let Some((bytes, _)) = manifest.read_arrow(prepared, &relative)? else {
                continue;
            };
            let reader = FileReader::try_new(Cursor::new(bytes), None)?;
            assert_cruise_contract(&relative, &[RecordBatch::new_empty(reader.schema())])
                .map_err(anyhow::Error::msg)?;
            let file_window = aircraft::SamplingWindow::from_metadata(reader.schema().metadata())
                .map_err(anyhow::Error::msg)?;
            ensure!(
                window.as_ref().is_none_or(|seen| *seen == file_window),
                "mixed cruise sampling windows"
            );
            let weights = file_window.provenance_weights();
            window = Some(file_window);
            for batch in reader {
                let rows = CruiseRowAccum::new(&[batch?]).map_err(anyhow::Error::msg)?;
                let slices = rows.views();
                let views = slices.as_row_views();
                let mut group = Group {
                    bounds: [
                        f64::INFINITY,
                        f64::INFINITY,
                        f64::NEG_INFINITY,
                        f64::NEG_INFINITY,
                    ],
                    half_length: 0.0,
                    buckets: Vec::new(),
                };
                for (index, row) in views.iter().enumerate() {
                    let Some((segment, density)) = cruise_segment(row, index, &weights) else {
                        continue;
                    };
                    let terrain = SegmentTerrain::sample(&segment, rasters);
                    ensure!(
                        [
                            terrain.start_elev,
                            terrain.q1_elev,
                            terrain.mid_elev,
                            terrain.q3_elev,
                            terrain.end_elev
                        ]
                        .iter()
                        .all(|v| v.is_finite()),
                        "cruise source terrain is unavailable"
                    );
                    if !aircraft::is_valid_airborne_with_terrain(&segment, &terrain) {
                        continue;
                    }
                    let lon = west + grid::geo::wrapped_longitude_delta(west, row.lon);
                    group.bounds[0] = group.bounds[0].min(row.lat);
                    group.bounds[2] = group.bounds[2].max(row.lat);
                    group.bounds[1] = group.bounds[1].min(lon);
                    group.bounds[3] = group.bounds[3].max(lon);
                    group.half_length = group
                        .half_length
                        .max(f64::from(segment.segment_length_m) * 0.5);
                    group.buckets.push(Bucket {
                        prepared: aircraft::prepare_segment(
                            &segment,
                            terrain.start_elev - 30.0,
                            terrain.end_elev - 30.0,
                        ),
                        lat: row.lat,
                        lon: row.lon,
                        half_length: f64::from(segment.segment_length_m) * 0.5,
                        density,
                        period: usize::from(row.period.min(2)),
                    });
                }
                if !group.buckets.is_empty() {
                    groups.push(group)
                }
            }
        }
        let days = window.map_or(0, |window| window.baseline_days);
        Self::build(owner, lattice, groups, days, rasters)
    }
    fn build(
        owner: Square,
        lattice: Lattice,
        groups: Vec<Group>,
        days: u16,
        rasters: &dyn RasterSampler,
    ) -> Result<Self> {
        let count = lattice.width * lattice.height;
        let mut field = Self {
            owner,
            lattice,
            groups,
            altitudes: Vec::new(),
            energies: vec![[0.0; 3]; count],
            days,
        };
        if field.groups.is_empty() {
            return Ok(field);
        }
        ensure!(days > 0, "cruise normalization window missing");
        field.altitudes = (0..count)
            .into_par_iter()
            .map(|i| {
                let [lat, lon] = field
                    .lattice
                    .point(i % field.lattice.width, i / field.lattice.width);
                rasters.elevation(lat, lon) + DEFAULT_RECEIVER_HEIGHT
            })
            .collect();
        ensure!(
            field.altitudes.iter().all(|v| v.is_finite()),
            "cruise receiver terrain is unavailable"
        );
        #[cfg(feature = "gpu")]
        {
            let buckets: Vec<_> = field.groups.iter().flat_map(|g| &g.buckets).collect();
            let nodes: Vec<_> = (0..count)
                .map(|i| {
                    field
                        .lattice
                        .point(i % field.lattice.width, i / field.lattice.width)
                })
                .collect();
            field.energies = gpu::evaluate(&buckets, &nodes, &field.altitudes)?;
        }
        #[cfg(not(feature = "gpu"))]
        {
            let npd = aircraft::NpdLuts::shared();
            field
                .energies
                .par_chunks_mut(field.lattice.width)
                .enumerate()
                .for_each(|(y, values)| {
                    let [lat, _] = field.lattice.point(0, y);
                    let m_lon = aircraft::M_PER_DEG_LAT * lat.to_radians().cos().max(0.2);
                    for group in &field.groups {
                        let columns: Vec<_> = (0..field.lattice.width)
                            .filter_map(|x| {
                                let [_, lon] = field.lattice.point(x, y);
                                group.reaches(lat, lon).then_some((x, lon))
                            })
                            .collect();
                        if columns.is_empty() {
                            continue;
                        }
                        for bucket in &group.buckets {
                            let row = aircraft::prepare_row(&bucket.prepared, lat, m_lon);
                            for &(x, lon) in &columns {
                                values[x][bucket.period] += bucket.energy_at_row(
                                    lat,
                                    lon,
                                    field.altitudes[y * field.lattice.width + x],
                                    npd,
                                    &row,
                                );
                            }
                        }
                    }
                });
        }
        Ok(field)
    }
    pub fn period_powers(
        &self,
        scene: &SurfaceScene,
        receivers: &TileReceivers,
    ) -> Result<Vec<f32>> {
        ensure!(
            scene.owner == self.owner,
            "cruise field belongs to another owner"
        );
        let count = receivers.x.len();
        ensure!(
            receivers.y.len() == count && receivers.altitude.len() == count,
            "incomplete cruise receivers"
        );
        if self.groups.is_empty() {
            return Ok(vec![0.0; count * 3]);
        }
        let points: Vec<_> = receivers
            .x
            .iter()
            .zip(&receivers.y)
            .map(|(&x, &y)| scene.frame.decode(x, y))
            .collect();
        self.powers_at(&points, &receivers.altitude)
    }
    fn powers_at(&self, points: &[[f64; 2]], altitudes: &[f32]) -> Result<Vec<f32>> {
        ensure!(
            points.len() == altitudes.len() && altitudes.iter().all(|x| x.is_finite()),
            "invalid cruise receiver heights"
        );
        let count = points.len();
        if self.groups.is_empty() {
            return Ok(vec![0.0; count * 3]);
        }
        let brackets = points
            .iter()
            .map(|&[lat, lon]| self.lattice.bracket(lat, lon))
            .collect::<Result<Vec<_>>>()?;
        let max_alt = altitudes.iter().copied().fold(f32::NEG_INFINITY, f32::max) as f64;
        let near: Vec<_> = self
            .groups
            .iter()
            .flat_map(|g| &g.buckets)
            .filter(|b| {
                b.prepared
                    .start_alt_m
                    .min(b.prepared.start_alt_m + b.prepared.sdz)
                    <= max_alt + 7_620.0
            })
            .collect();
        let mut nodes = self.energies.clone();
        if !near.is_empty() {
            let mut required = BTreeSet::new();
            for &(x, y, _, _) in &brackets {
                for i in [
                    y * self.lattice.width + x,
                    y * self.lattice.width + x + 1,
                    (y + 1) * self.lattice.width + x,
                    (y + 1) * self.lattice.width + x + 1,
                ] {
                    required.insert(i);
                }
            }
            let node_points: Vec<_> = required
                .iter()
                .map(|&i| {
                    self.lattice
                        .point(i % self.lattice.width, i / self.lattice.width)
                })
                .collect();
            let node_altitudes: Vec<_> = required.iter().map(|&i| self.altitudes[i]).collect();
            let subtracted = self.near_energies(&near, &node_points, &node_altitudes)?;
            for (i, energy) in required.iter().zip(&subtracted) {
                for period in 0..3 {
                    nodes[*i][period] -= energy[period];
                }
                for power in &mut nodes[*i] {
                    *power = power.max(0.0);
                }
            }
        }
        let receiver_altitudes: Vec<_> = altitudes.iter().map(|&a| f64::from(a)).collect();
        let added = self.near_energies(&near, points, &receiver_altitudes)?;
        let mut out = vec![0.0; count * 3];
        out.par_chunks_mut(3).enumerate().for_each(|(i, power)| {
            let mut energy = self.lattice.blend(&nodes, brackets[i]);
            for period in 0..3 {
                energy[period] += added[i][period];
                power[period] = (energy[period]
                    / (f64::from(self.days) * aircraft::PERIOD_SECONDS[period]))
                    as f32;
            }
        });
        ensure!(
            out.iter().all(|v| v.is_finite() && *v >= 0.0),
            "invalid cruise mean power"
        );
        Ok(out)
    }
    /// Near-field bucket energies per receiver, either from the cruise CUDA unit
    /// or, without the GPU feature, from the canonical CPU kernel.
    fn near_energies(
        &self,
        near: &[&Bucket],
        points: &[[f64; 2]],
        altitudes: &[f64],
    ) -> Result<Vec<[f64; 3]>> {
        if near.is_empty() {
            return Ok(vec![[0.0; 3]; points.len()]);
        }
        #[cfg(feature = "gpu")]
        {
            gpu::evaluate(near, points, altitudes)
        }
        #[cfg(not(feature = "gpu"))]
        {
            let npd = aircraft::NpdLuts::shared();
            Ok(points
                .par_iter()
                .zip(altitudes)
                .map(|(&[lat, lon], &altitude)| {
                    let mut energy = [0.0f64; 3];
                    for bucket in near {
                        energy[bucket.period] += bucket.energy(lat, lon, altitude, npd);
                    }
                    energy
                })
                .collect())
        }
    }
}

#[cfg(test)]
#[path = "cruise_field_tests.rs"]
mod tests;

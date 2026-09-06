//! Receiver selection and horizons shared by the CPU and GPU airborne painters.

use noise_compute::emission::aircraft::{
    build_receiver_horizon_row, BuildingHorizon, ReceiverHorizon,
};
use noise_compute::envelope::EnvelopeClass;
use noise_compute::propagation::obstacle_index::{CrossingScratch, ObstacleSet};
use noise_compute::types::RasterSampler;
use raster_reader::fused_tile_z13::FusedTileZ13;
use rayon::prelude::*;

use crate::accumulator::{CoarseLattice, COARSE_LEVELS_N};
use crate::grid::TILE_PX;
use crate::source_loader_structure::InteriorEstimate;

/// Buffers for one row of [`ReceiverScreeningGrid::build`], one per Rayon job and
/// reused across the rows that job takes, so the row batching adds no allocator
/// traffic to the DEM march it replaces.
#[derive(Default)]
struct RowScratch {
    crossings: CrossingScratch,
    columns: Vec<usize>,
    marched: Vec<(f64, f64)>,
    horizons: Vec<ReceiverHorizon>,
}

struct ReceiverScreening {
    terrain: ReceiverHorizon,
    buildings: Option<Box<BuildingHorizon>>,
}

/// Pixel-indexed horizons. Pixels unused by every coarse lattice stay `None`
/// unless at least one segment takes the exact receiver path.
pub struct ReceiverScreeningGrid(Vec<Option<ReceiverScreening>>);

impl ReceiverScreeningGrid {
    pub fn build(
        tile: &FusedTileZ13,
        obstacles: &ObstacleSet,
        interior: &InteriorEstimate,
        exact_receiver_path: bool,
    ) -> Self {
        let coarse_axis = coarse_receiver_axis();
        // Row-batched: every selected receiver in a pixel row shares its
        // latitude, so one `build_receiver_horizon_row` call marches the DEM
        // once per (sector, sample) across the whole row instead of once per
        // receiver. The horizons are bit-identical; only the DEM access order
        // changes. Filled in place — a per-row `Vec<Option<ReceiverScreening>>`
        // would allocate and then move ~0.8 KB per receiver on top of the
        // march it is meant to make cheaper.
        // Parallel init: the grid is ~0.2 GB of `Option<ReceiverScreening>`
        // per tile, and first-touching it on one thread costs more than the
        // march this batching saves.
        let mut receivers: Vec<Option<ReceiverScreening>> = (0..TILE_PX * TILE_PX)
            .into_par_iter()
            .map(|_| None)
            .collect();
        receivers.par_chunks_mut(TILE_PX).enumerate().for_each_init(
            RowScratch::default,
            |scratch, (py, row)| {
                scratch.columns.clear();
                scratch.columns.extend((0..TILE_PX).filter(|&px| {
                    pixel_is_selected(py * TILE_PX + px, exact_receiver_path, &coarse_axis)
                }));
                if scratch.columns.is_empty() {
                    return;
                }
                let rx_lat = tile.rx_lat[py];
                scratch.marched.clear();
                scratch.marched.extend(
                    scratch
                        .columns
                        .iter()
                        .map(|&px| (tile.rx_lon[px], tile.rx_alt_m[py * TILE_PX + px] as f64)),
                );
                scratch.horizons.clear();
                scratch
                    .horizons
                    .resize(scratch.columns.len(), ReceiverHorizon::EMPTY);
                build_receiver_horizon_row(
                    |lat, lon| tile.elevation(lat, lon),
                    rx_lat,
                    &scratch.marched,
                    &mut scratch.horizons,
                );
                for ((&px, &(rx_lon, rx_alt)), &terrain) in scratch
                    .columns
                    .iter()
                    .zip(&scratch.marched)
                    .zip(&scratch.horizons)
                {
                    let building_enabled =
                        EnvelopeClass::from_u8(interior.classes()[py * TILE_PX + px])
                            == EnvelopeClass::Outdoor;
                    let buildings = building_enabled
                        .then(|| {
                            BuildingHorizon::build(
                                obstacles,
                                tile,
                                rx_lat,
                                rx_lon,
                                rx_alt,
                                &mut scratch.crossings,
                            )
                        })
                        .filter(|horizon| !horizon.is_empty())
                        .map(Box::new);
                    row[px] = Some(ReceiverScreening { terrain, buildings });
                }
            },
        );
        Self(receivers)
    }

    #[inline]
    pub fn at(&self, pixel: usize) -> (&ReceiverHorizon, Option<&BuildingHorizon>) {
        let receiver = self.0[pixel]
            .as_ref()
            .expect("airborne scatter requested an unbuilt receiver horizon");
        (&receiver.terrain, receiver.buildings.as_deref())
    }
}

/// Receiver selection and index maps shared by the device terrain and building
/// builders. Exact-path tiles select every pixel; coarse-only tiles select the
/// union of the established interpolation lattices.
pub struct PackedReceiverScreening {
    pub record_of_pixel: Vec<u32>,
    pub pixel_of_record: Vec<u32>,
    pub building_enabled: Vec<u8>,
    pub records: usize,
}

impl PackedReceiverScreening {
    pub fn select(interior: &InteriorEstimate, exact_receiver_path: bool) -> Self {
        let coarse_axis = coarse_receiver_axis();
        let records = (0..TILE_PX * TILE_PX)
            .filter(|&pixel| pixel_is_selected(pixel, exact_receiver_path, &coarse_axis))
            .count();
        let mut packed = PackedReceiverScreening {
            record_of_pixel: vec![u32::MAX; TILE_PX * TILE_PX],
            pixel_of_record: Vec::with_capacity(records),
            building_enabled: Vec::with_capacity(records),
            records,
        };
        let mut record = 0usize;
        for pixel in 0..TILE_PX * TILE_PX {
            if !pixel_is_selected(pixel, exact_receiver_path, &coarse_axis) {
                continue;
            }
            packed.record_of_pixel[pixel] = record as u32;
            packed.pixel_of_record.push(pixel as u32);
            let outdoor =
                EnvelopeClass::from_u8(interior.classes()[pixel]) == EnvelopeClass::Outdoor;
            packed.building_enabled.push(u8::from(outdoor));
            record += 1;
        }
        packed
    }
}

#[inline]
fn pixel_is_selected(
    pixel: usize,
    exact_receiver_path: bool,
    coarse_axis: &[bool; TILE_PX],
) -> bool {
    exact_receiver_path || (coarse_axis[pixel / TILE_PX] && coarse_axis[pixel % TILE_PX])
}

/// Edge, in pixels, of the receiver blocks the device bounds the lowest source tangent over
/// (`airborne_lowest_source_tangent`): about 100 m at z13, small against the hundreds of
/// metres to a sub-segment whose direction the bound has to keep, while 1 024 blocks per tile
/// keep that pass far below the roof scan it prunes.
pub const LOWEST_SOURCE_TANGENT_BLOCK_PX: usize = 16;
const _: () = assert!(TILE_PX.is_multiple_of(LOWEST_SOURCE_TANGENT_BLOCK_PX));

/// Union of the three coarse axes. The current ladders are nested, but taking
/// their union makes horizon coverage stay correct if their sizes later change.
/// Pixels on this lattice are also queried by far sub-segments, which is why the
/// device gives them no lowest-source-tangent floor.
pub fn coarse_receiver_axis() -> [bool; TILE_PX] {
    let mut selected = [false; TILE_PX];
    for n in COARSE_LEVELS_N {
        let lattice = CoarseLattice::new(n);
        for i in 0..n {
            selected[lattice.coarse_pixel(i)] = true;
        }
    }
    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_coarse_lattice_node_is_selected() {
        let selected = coarse_receiver_axis();
        for n in COARSE_LEVELS_N {
            let lattice = CoarseLattice::new(n);
            for i in 0..n {
                let pixel = lattice.coarse_pixel(i);
                assert!(selected[pixel], "n={n} i={i} pixel={pixel}");
            }
        }
    }
}

//! Pure choices of the façade-exposure stage: the noisiest receiver, launch layout, row order.
use crate::source_frame::*;
use grid::poly::GridPolygons;
use tile_painter::hm3::SOURCE_LAYERS;

/// One launch of the painter's kernel evaluates this many receivers: a z13 tile's pixels.
pub const RECEIVERS_PER_LAUNCH: usize = TILE_PIXEL_SIDE * TILE_PIXEL_SIDE;
/// The kernel's thread block: the receivers that share one source list.
pub const RECEIVERS_PER_BLOCK: usize = BLOCK_PIXEL_SIDE * BLOCK_PIXEL_SIDE;

/// All-source Lden of one receiver from its layer powers.
pub fn all_source_lden(layers: &[&[f32]; SOURCE_LAYERS.len()], receiver: usize) -> f64 {
    let period = |p: usize| {
        let power: f64 = layers
            .iter()
            .map(|plane| f64::from(plane[receiver * PERIOD_COUNT + p]))
            .sum();
        10.0 * power.log10()
    };
    noise_compute::periods::compute_lden(period(0), period(1), period(2))
}

/// The loudest receiver of a building (first in canonical order on ties) and
/// the runner-up's Lden.
pub fn noisiest_receiver(
    layers: &[&[f32]; SOURCE_LAYERS.len()],
    receivers: std::ops::Range<usize>,
) -> Option<(usize, Option<f64>)> {
    let mut best: Option<(usize, f64)> = None;
    let mut runner_up: Option<f64> = None;
    for receiver in receivers {
        let lden = all_source_lden(layers, receiver);
        match best {
            Some((_, loudest)) if lden <= loudest => {
                runner_up = Some(runner_up.map_or(lden, |second: f64| second.max(lden)));
            }
            _ => {
                runner_up = best.map(|(_, loudest)| loudest);
                best = Some((receiver, lden));
            }
        }
    }
    best.map(|(receiver, _)| (receiver, runner_up))
}

pub fn morton_key((gx, gy): (i32, i32)) -> u64 {
    let spread = |value: u32| {
        let mut v = u64::from(value);
        v = (v | (v << 16)) & 0x0000_ffff_0000_ffff;
        v = (v | (v << 8)) & 0x00ff_00ff_00ff_00ff;
        v = (v | (v << 4)) & 0x0f0f_0f0f_0f0f_0f0f;
        v = (v | (v << 2)) & 0x3333_3333_3333_3333;
        (v | (v << 1)) & 0x5555_5555_5555_5555
    };
    spread(gx as u32) | (spread(gy as u32) << 1)
}

pub fn footprint_bbox(polygons: &GridPolygons) -> [f64; 4] {
    let mut bbox = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for &(gx, gy) in polygons.iter().flatten().flatten() {
        let (lon, lat) = square_store::grid_cols::grid_cell_lonlat(gx, gy);
        bbox = [bbox[0].min(lat), bbox[1].min(lon), bbox[2].max(lat), bbox[3].max(lon)];
    }
    bbox
}

/// The kernel's pixel for receiver slot `slot` of a launch: 256 consecutive
/// slots fill one 16 × 16 block, so each block's source list serves receivers
/// that lie together.
pub fn pixel_of_slot(slot: usize) -> usize {
    let block = slot / RECEIVERS_PER_BLOCK;
    let within = slot % RECEIVERS_PER_BLOCK;
    let block_row = block / BLOCKS_PER_TILE_SIDE;
    let block_column = block % BLOCKS_PER_TILE_SIDE;
    (block_row * BLOCK_PIXEL_SIDE + within / BLOCK_PIXEL_SIDE) * TILE_PIXEL_SIDE
        + block_column * BLOCK_PIXEL_SIDE
        + within % BLOCK_PIXEL_SIDE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layers_with_road(road: &[f32]) -> [Vec<f32>; SOURCE_LAYERS.len()] {
        std::array::from_fn(|layer| {
            if layer == 0 {
                road.to_vec()
            } else {
                vec![0.0; road.len()]
            }
        })
    }

    /// The loudest receiver wins, the first in canonical order on a tie, and a
    /// rerun over the same powers makes the same choice.
    #[test]
    fn the_noisiest_receiver_wins_and_ties_keep_the_lowest_canonical_index() {
        let road = [10.0, 10.0, 10.0, 1.0e6, 1.0e6, 1.0e6, 1.0e3, 1.0e3, 1.0e3, 1.0e6, 1.0e6, 1.0e6];
        let planes = layers_with_road(&road);
        let layers = planes.each_ref().map(|plane| plane.as_slice());
        let (best, runner_up) = noisiest_receiver(&layers, 0..4).unwrap();
        assert_eq!(best, 1, "receivers 1 and 3 tie; the lower index wins");
        assert!((runner_up.unwrap() - all_source_lden(&layers, 3)).abs() < 1e-12);
        assert_eq!(noisiest_receiver(&layers, 2..4).unwrap().0, 3);
        assert_eq!(noisiest_receiver(&layers, 2..3).unwrap(), (2, None));
        assert!(noisiest_receiver(&layers, 2..2).is_none(), "no receiver, no exposure");
    }

    #[test]
    fn launch_slots_fill_whole_blocks_and_every_pixel_once() {
        let mut seen = vec![false; RECEIVERS_PER_LAUNCH];
        for slot in 0..RECEIVERS_PER_LAUNCH {
            let pixel = pixel_of_slot(slot);
            assert!(!seen[pixel]);
            seen[pixel] = true;
            let block = (pixel / TILE_PIXEL_SIDE / BLOCK_PIXEL_SIDE) * BLOCKS_PER_TILE_SIDE
                + pixel % TILE_PIXEL_SIDE / BLOCK_PIXEL_SIDE;
            assert_eq!(block, slot / RECEIVERS_PER_BLOCK);
        }
    }
}

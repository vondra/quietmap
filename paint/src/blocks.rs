//! A block of the painter's square: its corners' loud sources and which of them the block's pixels
//! evaluate exactly, by dev1's rule (`crate::paint`); the energy the others leave at each corner.

use crate::exact::LAYERS;
use std::collections::HashSet;

/// A block's corners admit their loudest sources, layer by layer, until the energy each corner has
/// not admitted is at most this share of the block's quietest corner's: dev1's speed dial
/// (`relevance_partition.rs`, chosen by the owner 2026-09-02; on dev1's etalon window 0.15 broke
/// only the railways' rung over 6 dB, 194 cells against 138, where 0.10 passed every rung at 58 %
/// more card time).
pub const DROP_BUDGET_FRACTION: f64 = 0.15;

/// A block corner: per layer every source loud there with its Lden energy, loudest first, and the
/// layer's total.
#[derive(Default)]
pub(crate) struct Corner {
    pub ranked: [Vec<(u32, f64)>; LAYERS],
    pub totals: [f64; LAYERS],
}

/// A block: the sources its pixels evaluate exactly (sorted), and per corner (north-west,
/// north-east, south-west, south-east) and layer the energy they leave out there.
pub(crate) struct Block {
    pub exact: Vec<u32>,
    pub left_out: [[f64; LAYERS]; 4],
}

/// Whether the piece `a`-`b` meets the rectangle `low`-`high` (Liang-Barsky).
pub(crate) fn meets(a: [f64; 2], b: [f64; 2], low: [f64; 2], high: [f64; 2]) -> bool {
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    let d = [b[0] - a[0], b[1] - a[1]];
    for axis in 0..2 {
        for (p, q) in [
            (-d[axis], a[axis] - low[axis]),
            (d[axis], high[axis] - a[axis]),
        ] {
            if p == 0.0 {
                if q < 0.0 {
                    return false;
                }
            } else if p < 0.0 {
                t0 = t0.max(q / p);
            } else {
                t1 = t1.min(q / p);
            }
        }
    }
    t0 <= t1
}

/// dev1's admission of a block: its own sources `local`, then at each corner in turn, layer by
/// layer, the corner's loudest until the energy it has not admitted is within the budget; and the
/// energy the admitted leave out at each corner.
pub(crate) fn admit(local: Vec<u32>, corners: [&Corner; 4]) -> Block {
    let mut admitted: HashSet<u32> = local.iter().copied().collect();
    let mut exact = local;
    for layer in 0..LAYERS {
        let quietest = (corners.iter())
            .map(|corner| corner.totals[layer])
            .fold(f64::INFINITY, f64::min);
        let budget = DROP_BUDGET_FRACTION * quietest;
        for corner in corners {
            let ranked = &corner.ranked[layer];
            let mut unadmitted: f64 = (ranked.iter())
                .filter(|(source, _)| !admitted.contains(source))
                .map(|(_, energy)| energy)
                .sum();
            for &(source, energy) in ranked {
                if unadmitted <= budget {
                    break;
                }
                if admitted.insert(source) {
                    exact.push(source);
                    unadmitted -= energy;
                }
            }
        }
    }
    let left_out = corners.map(|corner| {
        std::array::from_fn(|layer| {
            let kept: f64 = (corner.ranked[layer].iter())
                .filter(|(source, _)| admitted.contains(source))
                .map(|(_, energy)| energy)
                .sum();
            (corner.totals[layer] - kept).max(0.0)
        })
    });
    exact.sort_unstable();
    Block { exact, left_out }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiles::sources::Layer;

    fn corner(road: Vec<(u32, f64)>) -> Corner {
        let mut corner = Corner::default();
        corner.totals[Layer::Road as usize] = road.iter().map(|&(_, energy)| energy).sum();
        corner.ranked[Layer::Road as usize] = road;
        corner
    }

    /// dev1's rule: a block keeps its own sources (9), then each corner admits its loudest until
    /// what it has not admitted is within 15 % of the quietest corner's layer. Corners of 93 and
    /// 23 give a budget of 3.45: the loud corners admit both their sources and nothing is left
    /// out. Four corners of 100 give 15: source 2's 12 is left out, blended from the corners.
    #[test]
    fn a_block_admits_the_loudest_until_the_rest_is_within_the_quietest_corners_budget() {
        let road = Layer::Road as usize;
        let loud = corner(vec![(1, 80.0), (2, 12.0), (9, 1.0)]);
        let quiet = corner(vec![(2, 12.0), (1, 10.0), (9, 1.0)]);
        let block = admit(vec![9], [&loud, &loud, &quiet, &quiet]);
        assert_eq!(block.exact, vec![1, 2, 9]);
        assert!(block.left_out.iter().all(|corner| corner[road] == 0.0));
        let even = corner(vec![(1, 88.0), (2, 12.0)]);
        let block = admit(Vec::new(), [&even; 4]);
        assert_eq!(block.exact, vec![1]);
        assert!(block.left_out.iter().all(|corner| corner[road] == 12.0));
    }
}

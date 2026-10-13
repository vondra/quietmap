//! A block of the painter's square: its corners' loud sources and which of them the block's pixels
//! evaluate exactly, by dev1's rule (`crate::paint`); the energy the others leave at each corner.

use crate::exact::{LAYERS, PairEnergy};
use physics::bands::{PERIODS, lden_energy};
use std::collections::HashSet;

/// A block's corners admit their loudest sources, layer by layer, until the energy each corner has
/// not admitted is at most this share of the block's quietest corner's: dev1's speed dial
/// (`relevance_partition.rs`; 0.15 chosen by the owner 2026-09-02, on dev1's etalon window
/// breaking only the railways' rung over 6 dB). 0.20 since 2026-10-13 (the owner: "souhlasím"):
/// with the left-out's reach on Dobris's four squares 13 % fewer pairs, wbench-2's Dobris 154.2 ->
/// 143.6 s, the total's drift within every rung (over 3 phon 0 -> 2 cells), the railway over 1
/// phon 1.4 -> 2.0 %, the buildings 0.8 -> 1.1 %; 0.25 no faster.
pub const DROP_BUDGET_FRACTION: f64 = 0.20;

/// A source loud at a corner: its index, its group, its Lden energy, its energy per period (mixed
/// and in either weather state) and its distance from the corner.
pub(crate) struct Ranked {
    pub index: u32,
    pub group: u64,
    pub lden: f64,
    pub energy: PairEnergy,
    pub distance_m: f64,
}

/// A block corner: per layer every source loud there, loudest first, and the layer's total Lden
/// energy.
#[derive(Default)]
pub(crate) struct Corner {
    pub ranked: [Vec<Ranked>; LAYERS],
    pub totals: [f64; LAYERS],
}

impl Corner {
    /// Adds source `index` of `group` and `layer` heard there with `energy` per period,
    /// `distance_m` away.
    pub fn add(
        &mut self,
        layer: usize,
        (index, group): (u32, u64),
        (energy, distance_m): (PairEnergy, f64),
    ) {
        let lden = lden_energy(&energy.mixed);
        self.ranked[layer].push(Ranked {
            index,
            group,
            lden,
            energy,
            distance_m,
        });
        self.totals[layer] += lden;
    }
}

/// A source group a block leaves out: its energy per period at each corner (north-west,
/// north-east, south-west, south-east), mixed and in either weather state, the far pieces' calm and
/// downwind apart by up to 17 dB at 700 m; its loudest source left out, whose display record its
/// line takes; and at each corner its nearest source left out (`u32::MAX` where it has none there),
/// whose distance its line takes where a pixel hears none of its other pieces, as a click takes a
/// group's nearest piece.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LeftGroup {
    pub key: u64,
    pub at_corners: [PairEnergy; 4],
    pub loudest: u32,
    pub nearest: [u32; 4],
}

/// A block: the sources its pixels evaluate exactly (sorted), of them those its corners admit
/// beyond its own (sorted) and their Lden energy at each corner, what they leave out at each corner
/// per layer and period, and of it each source group's (sorted by group): it sounds with its passes
/// at the pixels, not steady (a railway's farther ways carry the same trains).
pub(crate) struct Block {
    pub exact: Vec<u32>,
    pub admitted: Vec<u32>,
    pub admitted_at_corners: [f64; 4],
    pub left_out: [[[f64; PERIODS]; LAYERS]; 4],
    pub left_groups: Vec<LeftGroup>,
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
    let mut beyond = Vec::new();
    for layer in 0..LAYERS {
        let quietest = (corners.iter())
            .map(|corner| corner.totals[layer])
            .fold(f64::INFINITY, f64::min);
        let budget = DROP_BUDGET_FRACTION * quietest;
        for corner in corners {
            let ranked = &corner.ranked[layer];
            let mut unadmitted: f64 = (ranked.iter())
                .filter(|source| !admitted.contains(&source.index))
                .map(|source| source.lden)
                .sum();
            for source in ranked {
                if unadmitted <= budget {
                    break;
                }
                if admitted.insert(source.index) {
                    exact.push(source.index);
                    beyond.push(source.index);
                    unadmitted -= source.lden;
                }
            }
        }
    }
    let mut left_out = [[[0.0; PERIODS]; LAYERS]; 4];
    // Each source left out at a corner, with its Lden energy and distance there.
    let mut left: Vec<(LeftGroup, f64, [f64; 4])> = Vec::new();
    for (c, corner) in corners.iter().enumerate() {
        for (layer, ranked) in corner.ranked.iter().enumerate() {
            for source in ranked
                .iter()
                .filter(|source| !admitted.contains(&source.index))
            {
                for (total, value) in left_out[c][layer].iter_mut().zip(source.energy.mixed) {
                    *total += value;
                }
                let mut group = LeftGroup {
                    key: source.group,
                    at_corners: [PairEnergy::default(); 4],
                    loudest: source.index,
                    nearest: [u32::MAX; 4],
                };
                group.at_corners[c] = source.energy;
                group.nearest[c] = source.index;
                let mut distances = [f64::INFINITY; 4];
                distances[c] = source.distance_m;
                left.push((group, source.lden, distances));
            }
        }
    }
    // One entry a group: its loudest source (ties to the lowest index) the representative, at each
    // corner its nearest.
    left.sort_unstable_by(|a, b| {
        (a.0.key.cmp(&b.0.key))
            .then(b.1.total_cmp(&a.1))
            .then(a.0.loudest.cmp(&b.0.loudest))
    });
    left.dedup_by(|later, kept| {
        if later.0.key != kept.0.key {
            return false;
        }
        for c in 0..4 {
            let (into, from) = (&mut kept.0.at_corners[c], later.0.at_corners[c]);
            for p in 0..PERIODS {
                into.mixed[p] += from.mixed[p];
                into.states[0][p] += from.states[0][p];
                into.states[1][p] += from.states[1][p];
            }
            if later.2[c] < kept.2[c] {
                (kept.2[c], kept.0.nearest[c]) = (later.2[c], later.0.nearest[c]);
            }
        }
        true
    });
    let left_groups = left.into_iter().map(|(group, ..)| group).collect();
    exact.sort_unstable();
    beyond.sort_unstable();
    let admitted_at_corners = corners.map(|corner| {
        (corner.ranked.iter().flatten())
            .filter(|source| beyond.binary_search(&source.index).is_ok())
            .map(|source| source.lden)
            .sum()
    });
    Block {
        exact,
        admitted: beyond,
        admitted_at_corners,
        left_out,
        left_groups,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiles::sources::Layer;

    /// A corner of road sources, each its own group, the same energy in every period, its index in
    /// metres away.
    fn corner(road: Vec<(u32, f64)>) -> Corner {
        let mut corner = Corner::default();
        for (index, energy) in road {
            let group = u64::from(index);
            let energy = PairEnergy {
                mixed: [energy; PERIODS],
                states: [[energy; PERIODS]; 2],
            };
            corner.add(
                Layer::Road as usize,
                (index, group),
                (energy, f64::from(index)),
            );
        }
        corner
    }

    /// dev1's rule: a block keeps its own sources (9), then each corner admits its loudest until
    /// what it has not admitted is within 20 % of the quietest corner's layer. Corners of 93 and
    /// 23 give a budget of 4.6: the loud corners admit both their sources and nothing is left out.
    /// Four corners of 100 give 20: source 2's 12 is left out, blended from the corners.
    #[test]
    fn a_block_admits_the_loudest_until_the_rest_is_within_the_quietest_corners_budget() {
        let road = Layer::Road as usize;
        let loud = corner(vec![(1, 80.0), (2, 12.0), (9, 1.0)]);
        let quiet = corner(vec![(2, 12.0), (1, 10.0), (9, 1.0)]);
        let block = admit(vec![9], [&loud, &loud, &quiet, &quiet]);
        assert_eq!(block.exact, vec![1, 2, 9]);
        assert!(
            block
                .left_out
                .iter()
                .all(|corner| corner[road] == [0.0; PERIODS])
        );
        assert!(block.left_groups.is_empty());
        let even = corner(vec![(1, 88.0), (2, 12.0)]);
        let block = admit(Vec::new(), [&even; 4]);
        assert_eq!(block.exact, vec![1]);
        assert!(
            block
                .left_out
                .iter()
                .all(|corner| corner[road] == [12.0; PERIODS])
        );
        let twelve = PairEnergy {
            mixed: [12.0; PERIODS],
            states: [[12.0; PERIODS]; 2],
        };
        let left = LeftGroup {
            key: 2,
            at_corners: [twelve; 4],
            loudest: 2,
            nearest: [2; 4],
        };
        assert_eq!(block.left_groups, vec![left]);
    }
}

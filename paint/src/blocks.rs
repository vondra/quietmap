//! A block of the painter's square: the sources its pixels evaluate exactly and what they leave out at its corners by layer and direction.

use crate::corner_sources::{Corner, Ranked, SECTORS};
use crate::exact::LAYERS;
use physics::bands::PERIODS;
use popup::percentiles::Weather;
use std::collections::HashSet;
use std::hash::{BuildHasherDefault, Hasher};

/// Source indices hashed by one multiplication (Fibonacci hashing): the standard hasher's SipHash
/// showed in a dense city square's profile (2026-10-17).
#[derive(Default)]
struct IndexHasher(u64);

impl Hasher for IndexHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, _: &[u8]) {
        unreachable!("an index set hashes u32 indices only")
    }

    fn write_u32(&mut self, index: u32) {
        self.0 = u64::from(index).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

type IndexSet = HashSet<u32, BuildHasherDefault<IndexHasher>>;

/// A pixel tracing deeper (`crate::transfers::Reaching::asks_deeper`) traces each corner's loudest
/// sources, layer by layer in `Layer::ALL`'s order (road, railway, industry, building, ship,
/// aircraft), until what the corner has not traced is at most this share of the block's quietest
/// corner's layer: dev1's rule, and every pixel's until 2026-11-12. The railways' 0.05 since
/// 2026-11-09: a railway's few large pieces, left out, are blended from the corners as lines of
/// passes, which read its loudness wrong where its energy is right.
const DEEPER_UNTRACED_SHARES: [f64; LAYERS] = [0.20, 0.05, 0.20, 0.20, 0.20, 0.20];

/// Per layer and direction.
pub(crate) type Strata<T> = [[T; SECTORS]; LAYERS];

/// What a group left out sounds at one corner from one direction in one period and weather bin
/// (`popup::percentiles::Weather::binned`): homogeneous, favourable and mixed. Each direction
/// reaches a pixel by its own transfer, so the groups' lines take exactly what the layer's left-out
/// energy holds of them, and the bins keep the pieces' weather apart as a click's contributor does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LeftBin {
    pub corner: u8,
    pub sector: u8,
    pub period: u8,
    pub bin: u8,
    pub values: [f64; 3],
}

/// A source group a block leaves out: its loudest source left out, whose display record its line
/// takes; at each corner (north-west, north-east, south-west, south-east) its nearest source left
/// out (`u32::MAX` where it has none there), whose distance its line takes where a pixel hears
/// none of its other pieces, as a click takes a group's nearest piece; and where its energy at the
/// corners lies among the block's [`LeftBin`]s.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LeftGroup {
    pub key: u64,
    pub loudest: u32,
    pub nearest: [u32; 4],
    pub bins: std::ops::Range<u32>,
}

/// A source a block's pixels trace that does not cross it or the eight around: its layer, its Lden
/// energy at each corner (0 where it is not loud there) and the direction each corner hears it
/// from.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Far {
    pub index: u32,
    pub layer: u8,
    pub at_corners: [f64; 4],
    pub sector: [u8; 4],
}

/// A block: the sources its pixels evaluate exactly (sorted: its own and its corners'), of them
/// its own (crossing it or the eight around, sorted) and the far ones (sorted); the most sources a
/// pixel of it may trace (its deeper list's count), which bounds a pass's pairs; what the exact
/// sources leave out at each corner per layer, direction and period, and of it each source
/// group's (sorted by group) with its weather bins: it sounds with its passes at the pixels, not
/// steady (a railway's farther ways carry the same trains).
#[derive(Clone)]
pub(crate) struct Block {
    pub exact: Vec<u32>,
    pub own: Vec<u32>,
    pub far: Vec<Far>,
    pub most: usize,
    pub left_out: Box<[Strata<[f64; PERIODS]>; 4]>,
    pub left_groups: Vec<LeftGroup>,
    pub left_bins: Vec<LeftBin>,
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

/// The sources a block's pixels evaluate exactly: its own `local` ones (crossing it or the eight
/// around) and its four corners'.
fn exact_sources(local: Vec<u32>, corners: [&Corner; 4]) -> (Vec<u32>, IndexSet) {
    let mut set: IndexSet = local.iter().copied().collect();
    let mut exact = local;
    for corner in corners {
        for &own in &corner.traced {
            if set.insert(own) {
                exact.push(own);
            }
        }
    }
    (exact, set)
}

/// What a pixel tracing deeper adds to the sources of `set`: layer by layer each corner's loudest
/// until what the corner has not traced is within [`DEEPER_UNTRACED_SHARES`] of the quietest
/// corner's layer.
fn deeper_additions(set: &IndexSet, corners: [&Corner; 4]) -> Vec<u32> {
    let mut more = Vec::new();
    let mut added = IndexSet::default();
    for (layer, share) in DEEPER_UNTRACED_SHARES.into_iter().enumerate() {
        let quietest = (corners.iter())
            .map(|corner| corner.totals[layer])
            .fold(f64::INFINITY, f64::min);
        let budget = share * quietest;
        for corner in corners {
            let ranked = &corner.ranked[layer];
            let mut untraced: f64 = (ranked.iter())
                .filter(|source| !set.contains(&source.index) && !added.contains(&source.index))
                .map(|source| source.lden)
                .sum();
            for source in ranked {
                if untraced <= budget {
                    break;
                }
                if !set.contains(&source.index) && added.insert(source.index) {
                    more.push(source.index);
                    untraced -= source.lden;
                }
            }
        }
    }
    more
}

/// The most sources a pixel of a block may trace (its deeper list's count) and whether its corners
/// leave any out of its exact sources: what a quiet block's test and a pass's bound ask of every
/// block, without the groups left out (building and sorting those was most of a dense city
/// square's work on the cores, 2026-10-17).
pub(crate) fn admission_counts(local: Vec<u32>, corners: [&Corner; 4]) -> (usize, bool) {
    let (exact, set) = exact_sources(local, corners);
    let leaves_out = (corners.iter())
        .flat_map(|corner| corner.ranked.iter().flatten())
        .any(|source| !set.contains(&source.index));
    let most = exact.len() + deeper_additions(&set, corners).len();
    (most, leaves_out)
}

/// A block's exact sources ([`exact_sources`]) and the energy they leave out at each corner by
/// layer and direction, of it each source group's.
pub(crate) fn admit(local: Vec<u32>, corners: [&Corner; 4]) -> Block {
    let own_count = local.len();
    let (exact, set) = exact_sources(local, corners);
    let most = exact.len() + deeper_additions(&set, corners).len();
    leave_out((own_count, most), exact, &set, corners)
}

/// `block` tracing deeper: its exact sources and what [`deeper_additions`] adds; what is then left
/// out.
pub(crate) fn admit_deeper(block: &Block, corners: [&Corner; 4]) -> Block {
    let mut set: IndexSet = block.exact.iter().copied().collect();
    let mut exact = block.own.clone();
    let own: IndexSet = exact.iter().copied().collect();
    exact.extend(block.exact.iter().filter(|index| !own.contains(index)));
    let more = deeper_additions(&set, corners);
    set.extend(more.iter().copied());
    exact.extend(more);
    leave_out((block.own.len(), exact.len()), exact, &set, corners)
}

/// The block whose pixels trace `exact` (its own sources its first `own_count`; `set` the same
/// sources) and at most `most` sources: what they leave out at each corner by layer and direction,
/// of it each source group's.
fn leave_out(
    (own_count, most): (usize, usize),
    mut exact: Vec<u32>,
    set: &IndexSet,
    corners: [&Corner; 4],
) -> Block {
    let mut own = exact[..own_count].to_vec();
    own.sort_unstable();
    // The corners' sources beyond the block's own, each corner's energy and direction to come.
    let mut far: Vec<(Far, [bool; 4])> = (exact[own_count..].iter())
        .map(|&index| {
            let far = Far {
                index,
                layer: 0,
                at_corners: [0.0; 4],
                sector: [0; 4],
            };
            (far, [false; 4])
        })
        .collect();
    far.sort_unstable_by_key(|(far, _)| far.index);
    let mut left_out = Box::new([[[[0.0; PERIODS]; SECTORS]; LAYERS]; 4]);
    // Each source left out at a corner as a small key (its group, Lden energy and index there) and
    // the corner: sorting the whole 360-byte groups moved more memory than anything else in a
    // dense city square (2026-10-17).
    let mut left: Vec<(u64, f64, u32, usize, &Ranked)> = Vec::new();
    for (c, corner) in corners.iter().enumerate() {
        for (layer, ranked) in corner.ranked.iter().enumerate() {
            for source in ranked {
                if !set.contains(&source.index) {
                    let stratum = &mut left_out[c][layer][source.sector as usize];
                    for (total, value) in stratum.iter_mut().zip(source.energy.mixed) {
                        *total += value;
                    }
                    left.push((source.group, source.lden, source.index, c, source));
                } else if let Ok(at) = far.binary_search_by_key(&source.index, |(far, _)| far.index)
                {
                    let (far, heard) = &mut far[at];
                    far.layer = layer as u8;
                    far.at_corners[c] = source.lden;
                    (far.sector[c], heard[c]) = (source.sector, true);
                }
            }
        }
    }
    // One entry a group: its loudest source (ties to the lowest index) the representative, at each
    // corner its sources' energy by direction, period and weather bin, and its nearest.
    left.sort_unstable_by(|a, b| {
        (a.0.cmp(&b.0))
            .then(b.1.total_cmp(&a.1))
            .then(a.2.cmp(&b.2))
    });
    let mut left_groups: Vec<LeftGroup> = Vec::new();
    let mut left_bins: Vec<LeftBin> = Vec::new();
    let mut nearest_m = [f64::INFINITY; 4];
    for &(key, _, index, c, source) in &left {
        if left_groups.last().is_none_or(|group| group.key != key) {
            close_group(&mut left_groups, &mut left_bins);
            let start = left_bins.len() as u32;
            left_groups.push(LeftGroup {
                key,
                loudest: index,
                nearest: [u32::MAX; 4],
                bins: start..start,
            });
            nearest_m = [f64::INFINITY; 4];
        }
        let group = left_groups.last_mut().unwrap();
        let energy = &source.energy;
        for period in 0..PERIODS {
            let states = [energy.states[0][period], energy.states[1][period]];
            let (bin, values) = Weather::binned(energy.mixed[period], states);
            if values.iter().any(|&value| value > 0.0) {
                left_bins.push(LeftBin {
                    corner: c as u8,
                    sector: source.sector,
                    period: period as u8,
                    bin: bin as u8,
                    values,
                });
            }
        }
        if source.slant_distance_m < nearest_m[c] {
            (nearest_m[c], group.nearest[c]) = (source.slant_distance_m, index);
        }
    }
    close_group(&mut left_groups, &mut left_bins);
    exact.sort_unstable();
    // A corner that does not hear a far source loud takes its direction from one that does.
    let far = (far.into_iter())
        .map(|(mut far, heard)| {
            let known = heard.iter().position(|&heard| heard).unwrap_or(0);
            let from_known = far.sector[known];
            for (sector, heard) in far.sector.iter_mut().zip(heard) {
                if !heard {
                    *sector = from_known;
                }
            }
            far
        })
        .collect();
    Block {
        exact,
        own,
        far,
        most,
        left_out,
        left_groups,
        left_bins,
    }
}

/// Ends the last group's bins: one entry per corner, direction, period and bin, its sources' sum.
fn close_group(groups: &mut [LeftGroup], bins: &mut Vec<LeftBin>) {
    let Some(group) = groups.last_mut() else {
        return;
    };
    let start = group.bins.start as usize;
    let key = |entry: &LeftBin| (entry.corner, entry.sector, entry.period, entry.bin);
    bins[start..].sort_unstable_by_key(key);
    let mut kept = start;
    for at in start..bins.len() {
        if kept > start && key(&bins[kept - 1]) == key(&bins[at]) {
            let values = bins[at].values;
            for (sum, value) in bins[kept - 1].values.iter_mut().zip(values) {
                *sum += value;
            }
        } else {
            bins[kept] = bins[at];
            kept += 1;
        }
    }
    bins.truncate(kept);
    group.bins.end = kept as u32;
}

#[cfg(test)]
#[path = "blocks_tests.rs"]
mod tests;

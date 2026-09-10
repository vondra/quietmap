//! Energy-budget corner-union selection with a per-block linear-energy background.

use std::collections::HashSet;

use anyhow::{bail, ensure, Result};
use rayon::prelude::*;

use crate::source_frame::{
    BLOCKS_PER_TILE_SIDE, BLOCK_COUNT, CORNERS_PER_TILE_SIDE, CORNER_COUNT, PERIOD_COUNT,
};
use crate::tile_source_incidence::TileSourceIncidence;

/// Fraction of a block's quietest-corner Lden energy that may stay un-admitted
/// (interpolated as background). This is the repaint speed dial; corner
/// interpolation is an approximation, not an interior error bound. dev1 etalon
/// v3 (2 450 z13 tiles, five surface layers, RTX 5070, exact-CPU reference):
///
///   fraction  GPU s   rail >3 dB (limit 1377)  rail >6 dB (138)  road >6 dB (341)
///   0.15      136.6                     1119         194 OVER               36
///   0.10      216.3                      596          52                     7
///   0.05      266.8                      171           6                     3
///
/// The rail tail is receivers 0.1–6 m behind a building that screens the line
/// while no corner of their block does. The owner chose 0.15 (2026-09-02, kept
/// 2026-09-10 for dev4 so the world repaint costs the same as dev1's).
pub const DROP_BUDGET_FRACTION: f64 = 0.15;

/// The complete reusable source partition for one fixed geographic tile.
#[derive(Clone, Debug, PartialEq)]
pub struct RelevantSourcePartition {
    pub block_offsets: Vec<u32>,
    pub relevant_source_indices: Vec<u32>,
    /// Residual after subtracting the final admitted union at every corner.
    pub background_corner_energy: Vec<[[f32; PERIOD_COUNT]; 4]>,
}

impl RelevantSourcePartition {
    pub fn source_indices_for_block(&self, block: usize) -> &[u32] {
        let start = self.block_offsets[block] as usize;
        let end = self.block_offsets[block + 1] as usize;
        &self.relevant_source_indices[start..end]
    }
}

/// Build a partition from one full-physics result per corner/source candidate pair.
pub fn build_relevant_source_partition(
    incidence: &TileSourceIncidence,
    corner_pair_energy: &[[f32; PERIOD_COUNT]],
    lden_weights: [f64; PERIOD_COUNT],
) -> Result<RelevantSourcePartition> {
    validate_incidence(incidence, corner_pair_energy.len())?;
    ensure!(
        corner_pair_energy
            .iter()
            .flatten()
            .all(|energy| energy.is_finite() && *energy >= 0.0),
        "invalid corner energy"
    );
    ensure!(
        lden_weights
            .iter()
            .all(|weight| weight.is_finite() && *weight > 0.0),
        "invalid period weight"
    );
    let ranked_sources = rank_sources_at_corners(incidence, corner_pair_energy, lden_weights);
    let corner_total_energy = sum_corner_energy(incidence, corner_pair_energy);
    // The same Lden scores the ranking is ordered by, so budget and ranking are
    // in one unit by construction.
    let corner_lden_total: Vec<f64> = ranked_sources
        .iter()
        .map(|ranked| ranked.iter().map(|&(_, score)| score).sum())
        .collect();
    let blocks: Vec<(Vec<u32>, [[f32; PERIOD_COUNT]; 4])> = (0..BLOCK_COUNT)
        .into_par_iter()
        .map(|block| {
            let local_sources = &incidence.local_source_indices_by_block[block];
            let mut relevant_sources = local_sources.clone();
            let mut admitted: HashSet<u32> = local_sources.iter().copied().collect();
            let corners = block_corner_indices(block);
            let block_budget = DROP_BUDGET_FRACTION
                * corners
                    .iter()
                    .map(|&corner| corner_lden_total[corner])
                    .fold(f64::INFINITY, f64::min);
            for corner in corners {
                let ranked = &ranked_sources[corner];
                let mut unadmitted = ranked
                    .iter()
                    .filter(|(source_index, _)| !admitted.contains(source_index))
                    .map(|&(_, score)| score)
                    .sum::<f64>();
                for &(source_index, score) in ranked {
                    if unadmitted <= block_budget {
                        break;
                    }
                    if admitted.insert(source_index) {
                        relevant_sources.push(source_index);
                        unadmitted -= score;
                    }
                }
            }
            relevant_sources.sort_unstable();
            debug_assert!(
                relevant_sources.windows(2).all(|pair| pair[0] < pair[1]),
                "the merge walk below needs the admitted list strictly ascending"
            );

            let mut block_background = [[0.0_f32; PERIOD_COUNT]; 4];
            for (block_corner, corner) in corners.into_iter().enumerate() {
                // Both lists ascend, so one merge walk finds every admitted
                // source's pair for all three periods at once. A source the
                // corner does not carry subtracts nothing, and subtracting
                // nothing is what skipping it does.
                let range = corner_pair_range(incidence, corner);
                let candidates = &incidence.corner_source_indices[range.clone()];
                debug_assert!(
                    candidates.windows(2).all(|pair| pair[0] < pair[1]),
                    "the merge walk needs the corner's candidates strictly ascending, \
                     which validate_incidence refused this incidence without"
                );
                let mut dropped = corner_total_energy[corner];
                let mut candidate = 0;
                for &source_index in &relevant_sources {
                    while candidate < candidates.len() && candidates[candidate] < source_index {
                        candidate += 1;
                    }
                    if candidate == candidates.len() {
                        break;
                    }
                    if candidates[candidate] == source_index {
                        let pair = &corner_pair_energy[range.start + candidate];
                        for period in 0..PERIOD_COUNT {
                            dropped[period] -= f64::from(pair[period]);
                        }
                    }
                }
                for period in 0..PERIOD_COUNT {
                    block_background[block_corner][period] = dropped[period].max(0.0) as f32;
                }
            }
            (relevant_sources, block_background)
        })
        .collect();
    let mut block_offsets = Vec::with_capacity(BLOCK_COUNT + 1);
    let mut relevant_source_indices = Vec::new();
    let mut background_corner_energy = Vec::with_capacity(BLOCK_COUNT);
    block_offsets.push(0);
    for (relevant_sources, block_background) in blocks {
        relevant_source_indices.extend(relevant_sources);
        block_offsets.push(relevant_source_indices.len() as u32);
        background_corner_energy.push(block_background);
    }
    Ok(RelevantSourcePartition {
        block_offsets,
        relevant_source_indices,
        background_corner_energy,
    })
}

/// Every corner's candidates as `(source, Lden-weighted energy)`, loudest first.
fn rank_sources_at_corners(
    incidence: &TileSourceIncidence,
    pair_energy: &[[f32; PERIOD_COUNT]],
    lden_weights: [f64; PERIOD_COUNT],
) -> Vec<Vec<(u32, f64)>> {
    (0..CORNER_COUNT)
        .into_par_iter()
        .map(|corner| {
            let range = corner_pair_range(incidence, corner);
            let mut ranked: Vec<(u32, f64)> = range
                .clone()
                .map(|pair| {
                    let score = pair_energy[pair]
                        .iter()
                        .zip(lden_weights)
                        .map(|(&energy, weight)| f64::from(energy) * weight)
                        .sum();
                    (incidence.corner_source_indices[pair], score)
                })
                .collect();
            ranked.sort_unstable_by(|left, right| {
                right
                    .1
                    .total_cmp(&left.1)
                    .then_with(|| left.0.cmp(&right.0))
            });
            ranked
        })
        .collect()
}

fn sum_corner_energy(
    incidence: &TileSourceIncidence,
    pair_energy: &[[f32; PERIOD_COUNT]],
) -> Vec<[f64; PERIOD_COUNT]> {
    (0..CORNER_COUNT)
        .map(|corner| {
            let mut totals = [0.0; PERIOD_COUNT];
            for pair in corner_pair_range(incidence, corner) {
                for period in 0..PERIOD_COUNT {
                    totals[period] += f64::from(pair_energy[pair][period]);
                }
            }
            totals
        })
        .collect()
}

fn block_corner_indices(block: usize) -> [usize; 4] {
    let row = block / BLOCKS_PER_TILE_SIDE;
    let column = block % BLOCKS_PER_TILE_SIDE;
    let top_left = row * CORNERS_PER_TILE_SIDE + column;
    [
        top_left,
        top_left + 1,
        top_left + CORNERS_PER_TILE_SIDE,
        top_left + CORNERS_PER_TILE_SIDE + 1,
    ]
}

fn corner_pair_range(incidence: &TileSourceIncidence, corner: usize) -> std::ops::Range<usize> {
    incidence.corner_offsets[corner] as usize..incidence.corner_offsets[corner + 1] as usize
}

fn validate_incidence(incidence: &TileSourceIncidence, pair_count: usize) -> Result<()> {
    if incidence.corner_offsets.len() != CORNER_COUNT + 1
        || incidence.local_source_indices_by_block.len() != BLOCK_COUNT
        || incidence.corner_offsets.first() != Some(&0)
        || incidence.corner_offsets.last().copied().unwrap_or_default() as usize != pair_count
        || incidence.corner_source_indices.len() != pair_count
    {
        bail!("corner/source incidence has inconsistent dimensions");
    }
    if incidence
        .corner_offsets
        .windows(2)
        .any(|window| window[0] > window[1])
    {
        bail!("corner/source offsets are not monotonic");
    }
    if incidence
        .local_source_indices_by_block
        .iter()
        .any(|sources| sources.windows(2).any(|window| window[0] >= window[1]))
        || (0..CORNER_COUNT).any(|corner| {
            incidence.corner_source_indices[corner_pair_range(incidence, corner)]
                .windows(2)
                .any(|window| window[0] >= window[1])
        })
    {
        bail!("source incidence lists must be strictly ascending");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SOURCE_COUNT: u32 = 34;

    fn compact_incidence() -> TileSourceIncidence {
        let source_count = TEST_SOURCE_COUNT;
        let mut corner_offsets = Vec::with_capacity(CORNER_COUNT + 1);
        let mut corner_source_indices = Vec::with_capacity(CORNER_COUNT * source_count as usize);
        corner_offsets.push(0);
        for _ in 0..CORNER_COUNT {
            corner_source_indices.extend(0..source_count);
            corner_offsets.push(corner_source_indices.len() as u32);
        }
        let mut local_source_indices_by_block = vec![Vec::new(); BLOCK_COUNT];
        local_source_indices_by_block[0] = vec![source_count - 1];
        TileSourceIncidence {
            corner_offsets,
            corner_source_indices,
            local_source_indices_by_block,
        }
    }

    #[test]
    fn corner_union_admits_until_the_dropped_energy_is_within_budget() {
        let incidence = compact_incidence();
        let energies: Vec<[f32; PERIOD_COUNT]> = incidence
            .corner_source_indices
            .iter()
            .map(|&source| [source as f32 + 1.0; PERIOD_COUNT])
            .collect();
        let partition = build_relevant_source_partition(
            &incidence,
            &energies,
            [
                12.0 / 24.0,
                4.0 / 24.0 * 10.0_f64.powf(0.5),
                8.0 / 24.0 * 10.0,
            ],
        )
        .unwrap();
        // Energies 1..=34 per corner, total 595: admission from the loudest down
        // stops once the rest is within 0.15 × 595 = 89.25, leaving 1+…+12 = 78.
        assert_eq!(
            partition.source_indices_for_block(0),
            &(12..TEST_SOURCE_COUNT).collect::<Vec<_>>()
        );
        assert_eq!(
            partition.background_corner_energy[0],
            [[78.0; PERIOD_COUNT]; 4]
        );
        assert_eq!(
            partition.source_indices_for_block(1),
            &(12..TEST_SOURCE_COUNT).collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_budget_is_spent_against_the_blocks_quietest_corner() {
        let incidence = compact_incidence();
        let energies: Vec<[f32; PERIOD_COUNT]> = incidence
            .corner_source_indices
            .iter()
            .enumerate()
            .map(|(pair, &source)| {
                let corner_scale = if pair < TEST_SOURCE_COUNT as usize {
                    0.01
                } else {
                    1.0
                };
                [(source as f32 + 1.0) * corner_scale; PERIOD_COUNT]
            })
            .collect();
        let partition = build_relevant_source_partition(
            &incidence,
            &energies,
            [
                12.0 / 24.0,
                4.0 / 24.0 * 10.0_f64.powf(0.5),
                8.0 / 24.0 * 10.0,
            ],
        )
        .unwrap();
        assert_eq!(
            partition.source_indices_for_block(0),
            &(0..TEST_SOURCE_COUNT).collect::<Vec<_>>()
        );
        assert_eq!(
            partition.source_indices_for_block(1),
            &(12..TEST_SOURCE_COUNT).collect::<Vec<_>>()
        );
    }
    #[test]
    fn later_corner_admission_is_subtracted_from_every_corner() {
        let mut incidence = compact_incidence();
        incidence
            .local_source_indices_by_block
            .iter_mut()
            .for_each(Vec::clear);
        let energies: Vec<_> = incidence
            .corner_source_indices
            .iter()
            .enumerate()
            .map(|(pair, &source)| {
                let corner = pair / TEST_SOURCE_COUNT as usize;
                let value = match source {
                    0 => {
                        if corner == 1 {
                            1.0
                        } else {
                            100.0
                        }
                    }
                    1 => {
                        if corner == 1 {
                            100.0
                        } else {
                            1.0
                        }
                    }
                    _ => 0.0,
                };
                [value; PERIOD_COUNT]
            })
            .collect();
        let partition =
            build_relevant_source_partition(&incidence, &energies, [1.0; PERIOD_COUNT]).unwrap();
        assert_eq!(partition.source_indices_for_block(0), &[0, 1]);
        assert_eq!(
            partition.background_corner_energy[0],
            [[0.0; PERIOD_COUNT]; 4]
        );
        let mut invalid = energies;
        invalid[0][0] = f32::NAN;
        assert!(
            build_relevant_source_partition(&incidence, &invalid, [1.0; PERIOD_COUNT]).is_err()
        );
    }
}

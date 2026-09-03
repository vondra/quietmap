//! Energy-budget corner-union selection with a persisted per-block linear-energy background.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::{bail, Context, Result};
use rayon::prelude::*;

use crate::source_frame::{
    BLOCKS_PER_TILE_SIDE, BLOCK_COUNT, CORNERS_PER_TILE_SIDE, CORNER_COUNT, PERIOD_COUNT,
};
use crate::tile_source_incidence::TileSourceIncidence;

const FILE_MAGIC: [u8; 8] = *b"QMRSP001";
const FILE_VERSION: u32 = 2;

/// A corner admits its ranked sources until the Lden energy it has NOT admitted
/// is at most this fraction of the SMALLEST corner total in its block; the rest
/// becomes that block's background constant. Energy, not a count: a motorway
/// block stops after a few sources, a block where twenty comparable streets meet
/// keeps all twenty.
///
/// Against the block's QUIETEST corner, not each corner's own total, because the
/// background is four corner samples blended bilinearly over the block's 256
/// pixels, and the damage it does is measured at the quietest pixel, not at the
/// loudest corner. Source-weighted over the four W2 benchmark cells, a rail
/// block's four corner totals stand a median 1.5 dB apart but 10.0 dB at the
/// 95th percentile, and 15 % of the loud corner's energy is then more than the
/// whole answer at the quiet one.
///
/// THIS IS THE SPEED DIAL. It trades painted accuracy for GPU seconds and nothing
/// else does so as directly; the owner chose 0.15 on 2026-09-02 because it is the
/// only measured value that meets the whole accuracy contract. Measured on r9950
/// (RTX 5070), the four wbench-orig cells, five surface layers in one process,
/// seconds and drift from the same run:
///
///   fraction   W2 GPU s   W2 wall   rail >3 dB cells (limit 1379)   industrial (921)
///   0.15       293.8      331.0     1101  passes                       48  passes
///   0.20       257.1      294.8     1738  1.3x over                    73  passes
///   0.30       209.7      247.1     3150  2.3x over                   187  passes
///
/// Every other rung of every layer passes at all three. The rule's own effect at
/// 0.15 was rail 4994 -> 1101 and industrial 1984 -> 48 for
/// +23.7 % of the wave's GPU seconds against the per-corner budget it replaced.
pub const DROP_BUDGET_FRACTION: f64 = 0.15;

/// The complete reusable source partition for one fixed geographic tile.
#[derive(Clone, Debug, PartialEq)]
pub struct RelevantSourcePartition {
    pub source_fingerprint: u64,
    pub block_offsets: Vec<u32>,
    pub relevant_source_indices: Vec<u32>,
    /// The dropped energy at each block's four corners, which the paint kernel
    /// blends bilinearly into every pixel of the block.
    ///
    /// A screening edge inside a block is a step in this field, and four samples
    /// cannot hold a step: measured against the exact CPU reference, refining the
    /// lattice to 8 pixels still leaves 2605 of rail's 4994 W2 cells over 3 dB
    /// and costs +50 GPU s of a 237 s wave, and giving every pixel its block's
    /// quietest corner leaves 1867 against a limit of 1379. So the blend is left
    /// alone and [`DROP_BUDGET_FRACTION`] keeps what it carries below the
    /// quietest answer in the block instead.
    pub background_corner_energy: Vec<[[f32; PERIOD_COUNT]; 4]>,
}

impl RelevantSourcePartition {
    pub fn source_indices_for_block(&self, block: usize) -> &[u32] {
        let start = self.block_offsets[block] as usize;
        let end = self.block_offsets[block + 1] as usize;
        &self.relevant_source_indices[start..end]
    }

    pub fn write_to(&self, path: &Path) -> Result<()> {
        validate_partition_shape(self)?;
        let temporary_path = path.with_extension("relevant-source-partition.tmp");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create partition directory {}", parent.display()))?;
        }
        let mut writer = BufWriter::new(
            File::create(&temporary_path)
                .with_context(|| format!("create {}", temporary_path.display()))?,
        );
        writer.write_all(&FILE_MAGIC)?;
        write_u32(&mut writer, FILE_VERSION)?;
        write_u32(&mut writer, crate::source_frame::BLOCK_PIXEL_SIDE as u32)?;
        write_u32(&mut writer, PERIOD_COUNT as u32)?;
        write_u32(&mut writer, BLOCK_COUNT as u32)?;
        write_u64(&mut writer, self.source_fingerprint)?;
        write_u32(&mut writer, self.relevant_source_indices.len() as u32)?;
        write_u32(&mut writer, 0)?;
        for &offset in &self.block_offsets {
            write_u32(&mut writer, offset)?;
        }
        for corners in &self.background_corner_energy {
            for periods in corners {
                for &energy in periods {
                    writer.write_all(&energy.to_le_bytes())?;
                }
            }
        }
        for &source_index in &self.relevant_source_indices {
            write_u32(&mut writer, source_index)?;
        }
        writer.flush()?;
        drop(writer);
        fs::rename(&temporary_path, path).with_context(|| {
            format!(
                "publish partition {} as {}",
                temporary_path.display(),
                path.display()
            )
        })?;
        Ok(())
    }
}

/// Build a partition from one full-physics result per corner/source candidate pair.
pub fn build_relevant_source_partition(
    incidence: &TileSourceIncidence,
    corner_pair_energy: &[[f32; PERIOD_COUNT]],
    lden_weights: [f64; PERIOD_COUNT],
    source_fingerprint: u64,
) -> Result<RelevantSourcePartition> {
    validate_incidence(incidence, corner_pair_energy.len())?;
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

            let mut block_background = [[0.0_f32; PERIOD_COUNT]; 4];
            for (block_corner, corner) in corners.into_iter().enumerate() {
                for period in 0..PERIOD_COUNT {
                    let mut dropped_energy = corner_total_energy[corner][period];
                    for &source_index in &relevant_sources {
                        dropped_energy -= lookup_pair_energy(
                            incidence,
                            corner_pair_energy,
                            corner,
                            source_index,
                            period,
                        ) as f64;
                    }
                    block_background[block_corner][period] = dropped_energy.max(0.0) as f32;
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
        source_fingerprint,
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

fn lookup_pair_energy(
    incidence: &TileSourceIncidence,
    pair_energy: &[[f32; PERIOD_COUNT]],
    corner: usize,
    source_index: u32,
    period: usize,
) -> f32 {
    let range = corner_pair_range(incidence, corner);
    match incidence.corner_source_indices[range.clone()].binary_search(&source_index) {
        Ok(relative_pair) => pair_energy[range.start + relative_pair][period],
        Err(_) => 0.0,
    }
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

fn validate_partition_shape(partition: &RelevantSourcePartition) -> Result<()> {
    if partition.block_offsets.len() != BLOCK_COUNT + 1
        || partition.background_corner_energy.len() != BLOCK_COUNT
        || partition.block_offsets.first() != Some(&0)
        || partition.block_offsets.last().copied().unwrap_or_default() as usize
            != partition.relevant_source_indices.len()
        || partition
            .block_offsets
            .windows(2)
            .any(|window| window[0] > window[1])
    {
        bail!("relevant-source partition has inconsistent dimensions");
    }
    Ok(())
}

fn write_u32(writer: &mut impl Write, value: u32) -> Result<()> {
    writer.write_all(&value.to_le_bytes())?;
    Ok(())
}

fn write_u64(writer: &mut impl Write, value: u64) -> Result<()> {
    writer.write_all(&value.to_le_bytes())?;
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

    /// Energies 1..=34 (source k carries k + 1) at every corner: the total is
    /// 595, so a corner keeps admitting from the loudest down until the
    /// un-admitted rest is at most 89.25 — sources 34..=13 leave
    /// 1+2+...+12 = 78 behind. Block 0 also holds source 33 (energy 34) locally.
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
            tile_painter::scatter_band::LDEN_WEIGHTS,
            7,
        )
        .unwrap();
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

    /// One quiet corner rules its whole block. Corner 0 hears a hundredth of what
    /// the others hear, so block 0 (the only block that owns corner 0) may leave
    /// at most 0.15 * 5.95 = 0.8925 behind at EVERY one of its corners — less
    /// than the weakest source carries, so it admits all 34. Block 1 does not
    /// touch corner 0 and keeps the 22 sources the budget allowed before. The
    /// numbers are in unweighted energy: the test's period energies are equal,
    /// so the Lden weight sum multiplies both sides and cancels.
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
            tile_painter::scatter_band::LDEN_WEIGHTS,
            7,
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
}

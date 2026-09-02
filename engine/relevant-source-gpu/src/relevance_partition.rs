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

/// A corner admits its ranked sources until the Lden energy it has NOT admitted is
/// at most this fraction of the corner's total; the rest becomes the background
/// constant. Energy, not a count: a motorway block stops after a few sources, a
/// block where twenty comparable streets meet keeps all twenty.
pub const DROP_BUDGET_FRACTION: f64 = 0.15;

/// The complete reusable source partition for one fixed geographic z12 tile.
#[derive(Clone, Debug, PartialEq)]
pub struct RelevantSourcePartition {
    pub source_fingerprint: u64,
    pub block_offsets: Vec<u32>,
    pub relevant_source_indices: Vec<u32>,
    /// The dropped energy at each block's four corners, which the paint kernel
    /// blends bilinearly into every pixel of the block.
    ///
    /// That blend, not the per-pair kernel, is where the W2 >3 dB tail lives.
    /// Measured on the four benchmark cells against the exact CPU reference: of
    /// the 4953 rail cells over 3 dB, 4857 have this background ALONE more than
    /// 1 dB above the exact answer, and subtracting it leaves 13 — Lden energy
    /// is linear in the period energies, so that subtraction is exact.
    /// Industrial goes 1929 -> 66 the same way. The four corners of a failing
    /// block already disagree by a median 8.0 dB (rail) / 18.3 dB (industrial)
    /// across the block's 16 pixels, so no blend of them can be right: the
    /// dropped field has to be resolved, not interpolated. Giving each pixel
    /// its block's quietest corner instead bounds the tail at 1465 (rail, limit
    /// 1379) and 154 (industrial, limit 921) cells.
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
    let blocks: Vec<(Vec<u32>, [[f32; PERIOD_COUNT]; 4])> = (0..BLOCK_COUNT)
        .into_par_iter()
        .map(|block| {
            let local_sources = &incidence.local_source_indices_by_block[block];
            let mut relevant_sources = local_sources.clone();
            let mut admitted: HashSet<u32> = local_sources.iter().copied().collect();
            for corner in block_corner_indices(block) {
                let ranked = &ranked_sources[corner];
                let corner_total: f64 = ranked.iter().map(|&(_, score)| score).sum();
                let mut unadmitted = ranked
                    .iter()
                    .filter(|(source_index, _)| !admitted.contains(source_index))
                    .map(|&(_, score)| score)
                    .sum::<f64>();
                for &(source_index, score) in ranked {
                    if unadmitted <= DROP_BUDGET_FRACTION * corner_total {
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
            for (block_corner, corner) in block_corner_indices(block).into_iter().enumerate() {
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
}

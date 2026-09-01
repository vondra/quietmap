//! Boolean corner-union selection with a persisted per-block linear-energy background.

use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::source_frame::{
    BLOCKS_PER_TILE_SIDE, BLOCK_COUNT, CORNERS_PER_TILE_SIDE, CORNER_COUNT, PERIOD_COUNT,
    RANKED_SOURCES_PER_CORNER,
};
use crate::tile_source_incidence::TileSourceIncidence;

const FILE_MAGIC: [u8; 8] = *b"QMRSP001";
const FILE_VERSION: u32 = 2;
const LDEN_PERIOD_WEIGHTS: [f64; PERIOD_COUNT] = [12.0, 12.649_110_640_7, 80.0];

/// The complete reusable source partition for one fixed geographic z12 tile.
#[derive(Clone, Debug, PartialEq)]
pub struct RelevantSourcePartition {
    pub source_fingerprint: u64,
    pub block_offsets: Vec<u32>,
    pub relevant_source_indices: Vec<u32>,
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

    pub fn read_from(path: &Path, expected_source_fingerprint: u64) -> Result<Self> {
        let mut reader = BufReader::new(
            File::open(path).with_context(|| format!("open partition {}", path.display()))?,
        );
        let mut magic = [0_u8; 8];
        reader.read_exact(&mut magic)?;
        if magic != FILE_MAGIC {
            bail!("{} is not a relevant-source partition", path.display());
        }
        let version = read_u32(&mut reader)?;
        let block_side = read_u32(&mut reader)? as usize;
        let period_count = read_u32(&mut reader)? as usize;
        let block_count = read_u32(&mut reader)? as usize;
        let source_fingerprint = read_u64(&mut reader)?;
        let relevant_count = read_u32(&mut reader)? as usize;
        let reserved = read_u32(&mut reader)?;
        if version != FILE_VERSION
            || block_side != crate::source_frame::BLOCK_PIXEL_SIDE
            || period_count != PERIOD_COUNT
            || block_count != BLOCK_COUNT
            || reserved != 0
        {
            bail!("{} has an incompatible partition header", path.display());
        }
        if source_fingerprint != expected_source_fingerprint {
            bail!("{} belongs to a different source ordering", path.display());
        }

        let block_offsets = read_u32_vector(&mut reader, BLOCK_COUNT + 1)?;
        let mut background_corner_energy = vec![[[0.0; PERIOD_COUNT]; 4]; BLOCK_COUNT];
        for corners in &mut background_corner_energy {
            for periods in corners {
                for energy in periods {
                    *energy = f32::from_le_bytes(read_array::<4>(&mut reader)?);
                }
            }
        }
        let relevant_source_indices = read_u32_vector(&mut reader, relevant_count)?;
        let mut trailing_byte = [0_u8; 1];
        if reader.read(&mut trailing_byte)? != 0 {
            bail!("{} has trailing bytes", path.display());
        }
        let partition = Self {
            source_fingerprint,
            block_offsets,
            relevant_source_indices,
            background_corner_energy,
        };
        validate_partition_shape(&partition)?;
        Ok(partition)
    }
}

/// Build a partition from one full-physics result per corner/source candidate pair.
pub fn build_relevant_source_partition(
    incidence: &TileSourceIncidence,
    corner_pair_energy: &[[f32; PERIOD_COUNT]],
    source_fingerprint: u64,
) -> Result<RelevantSourcePartition> {
    validate_incidence(incidence, corner_pair_energy.len())?;
    let ranked_sources = rank_sources_at_corners(incidence, corner_pair_energy);
    let corner_total_energy = sum_corner_energy(incidence, corner_pair_energy);
    let mut block_offsets = Vec::with_capacity(BLOCK_COUNT + 1);
    let mut relevant_source_indices = Vec::new();
    let mut background_corner_energy = Vec::with_capacity(BLOCK_COUNT);
    block_offsets.push(0);

    for block in 0..BLOCK_COUNT {
        let local_sources = &incidence.local_source_indices_by_block[block];
        let mut relevant_sources = local_sources.clone();
        for corner in block_corner_indices(block) {
            let mut nonlocal_kept = 0;
            for &source_index in &ranked_sources[corner] {
                if local_sources.binary_search(&source_index).is_ok() {
                    continue;
                }
                relevant_sources.push(source_index);
                nonlocal_kept += 1;
                if nonlocal_kept == RANKED_SOURCES_PER_CORNER {
                    break;
                }
            }
        }
        relevant_sources.sort_unstable();
        relevant_sources.dedup();

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

fn rank_sources_at_corners(
    incidence: &TileSourceIncidence,
    pair_energy: &[[f32; PERIOD_COUNT]],
) -> Vec<Vec<u32>> {
    (0..CORNER_COUNT)
        .map(|corner| {
            let range = corner_pair_range(incidence, corner);
            let mut ranked: Vec<(u32, f64)> = range
                .clone()
                .map(|pair| {
                    let score = pair_energy[pair]
                        .iter()
                        .zip(LDEN_PERIOD_WEIGHTS)
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
            ranked.into_iter().map(|(source, _)| source).collect()
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

fn read_u32(reader: &mut impl Read) -> Result<u32> {
    Ok(u32::from_le_bytes(read_array::<4>(reader)?))
}

fn read_u64(reader: &mut impl Read) -> Result<u64> {
    Ok(u64::from_le_bytes(read_array::<8>(reader)?))
}

fn read_u32_vector(reader: &mut impl Read, length: usize) -> Result<Vec<u32>> {
    (0..length).map(|_| read_u32(reader)).collect()
}

fn read_array<const LENGTH: usize>(reader: &mut impl Read) -> Result<[u8; LENGTH]> {
    let mut bytes = [0_u8; LENGTH];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn compact_incidence() -> TileSourceIncidence {
        let source_count = RANKED_SOURCES_PER_CORNER as u32 + 2;
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
    fn corner_union_keeps_local_source_and_each_corner_background_energy() {
        let incidence = compact_incidence();
        let energies: Vec<[f32; PERIOD_COUNT]> = incidence
            .corner_source_indices
            .iter()
            .map(|&source| [source as f32 + 1.0; PERIOD_COUNT])
            .collect();
        let partition = build_relevant_source_partition(&incidence, &energies, 7).unwrap();
        let first = partition.source_indices_for_block(0);
        assert_eq!(
            first,
            &(1..RANKED_SOURCES_PER_CORNER as u32 + 2).collect::<Vec<_>>()
        );
        assert_eq!(
            partition.background_corner_energy[0],
            [[1.0; PERIOD_COUNT]; 4]
        );
    }

    #[test]
    fn persisted_partition_round_trips_and_rejects_another_source_order() {
        let incidence = compact_incidence();
        let energies = vec![[1.0; PERIOD_COUNT]; incidence.corner_source_indices.len()];
        let partition = build_relevant_source_partition(&incidence, &energies, 91).unwrap();
        let directory = tempdir().unwrap();
        let path = directory.path().join("tile.rsp");
        partition.write_to(&path).unwrap();
        assert_eq!(
            RelevantSourcePartition::read_from(&path, 91).unwrap(),
            partition
        );
        assert!(RelevantSourcePartition::read_from(&path, 92).is_err());
    }
}

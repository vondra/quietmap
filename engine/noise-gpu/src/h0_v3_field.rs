//! Versioned binary field shared by the CPU H0 V3 renderer and analyser.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use noise_compute::propagation::h0_v3::{H0V3Observation, H0V3Theta};
use noise_compute::types::NUM_BANDS;
use raster_reader::fused_tile_z13::TILE_PX;
use sha2::{Digest, Sha256};
use tile_painter::accumulator::NUM_PERIODS;
use tile_painter::h0_pair_reference::H0V3PairArm;
use tile_painter::h0_v3_tile_reference::H0V3TileField;

pub const H0_V3_FIELD_MAGIC: [u8; 8] = *b"QMV3H0F2";
pub const H0_V3_FIELD_VERSION: u32 = 2;
const HEADER_U32_FIELDS: usize = 6;
pub const H0_V3_FIELD_HEADER_BYTES: usize = 8 + HEADER_U32_FIELDS * 4;
pub const H0_V3_FIELD_DATA_BYTES: usize = TILE_PX
    * TILE_PX
    * (NUM_PERIODS * size_of::<f32>() + NUM_PERIODS * NUM_BANDS * size_of::<f64>());
pub const H0_V3_FIELD_BYTES: usize = H0_V3_FIELD_HEADER_BYTES + H0_V3_FIELD_DATA_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct H0V3FieldIdentity {
    pub case_index: u32,
    pub arm: H0V3FieldArm,
}

/// On-disk arm identity. `Stock` carries period powers only and exists solely
/// to report the complete stock-model delta; it is never a judge and does not
/// isolate the P2b predicate from the other staged H0 changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H0V3FieldArm {
    Stock,
    H0(H0V3Theta),
    JudgeCoarse,
    JudgeFine,
}

impl From<H0V3PairArm> for H0V3FieldArm {
    fn from(arm: H0V3PairArm) -> Self {
        match arm {
            H0V3PairArm::Production(theta) => Self::H0(theta),
            H0V3PairArm::JudgeCoarse => Self::JudgeCoarse,
            H0V3PairArm::JudgeFine => Self::JudgeFine,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum H0V3FieldError {
    Io,
    InvalidHeader,
    InvalidLength,
    InvalidValue,
}

/// Hash one field or executable with the same lowercase SHA-256 form used by
/// the external evidence seals.
pub fn sha256_file_hex(path: &Path) -> Result<String, H0V3FieldError> {
    let mut input = BufReader::new(File::open(path).map_err(|_| H0V3FieldError::Io)?);
    let mut digest = Sha256::new();
    let mut chunk = [0_u8; 64 * 1024];
    loop {
        let count = input.read(&mut chunk).map_err(|_| H0V3FieldError::Io)?;
        if count == 0 {
            break;
        }
        digest.update(&chunk[..count]);
    }
    let digest = digest.finalize();
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut encoded, "{byte:02x}").map_err(|_| H0V3FieldError::Io)?;
    }
    Ok(encoded)
}

#[must_use]
pub const fn arm_code(arm: H0V3FieldArm) -> u32 {
    match arm {
        H0V3FieldArm::H0(H0V3Theta::Degrees5) => 0,
        H0V3FieldArm::H0(H0V3Theta::Degrees4) => 1,
        H0V3FieldArm::H0(H0V3Theta::Degrees3) => 2,
        H0V3FieldArm::H0(H0V3Theta::Degrees2) => 3,
        H0V3FieldArm::JudgeCoarse => 4,
        H0V3FieldArm::JudgeFine => 5,
        H0V3FieldArm::Stock => 6,
    }
}

pub fn arm_from_code(code: u32) -> Result<H0V3FieldArm, H0V3FieldError> {
    match code {
        0 => Ok(H0V3FieldArm::H0(H0V3Theta::Degrees5)),
        1 => Ok(H0V3FieldArm::H0(H0V3Theta::Degrees4)),
        2 => Ok(H0V3FieldArm::H0(H0V3Theta::Degrees3)),
        3 => Ok(H0V3FieldArm::H0(H0V3Theta::Degrees2)),
        4 => Ok(H0V3FieldArm::JudgeCoarse),
        5 => Ok(H0V3FieldArm::JudgeFine),
        6 => Ok(H0V3FieldArm::Stock),
        _ => Err(H0V3FieldError::InvalidHeader),
    }
}

/// Write one complete field with a self-describing, fixed-size header.
pub fn write_h0_v3_field(
    path: &Path,
    identity: H0V3FieldIdentity,
    field: &H0V3TileField,
) -> Result<(), H0V3FieldError> {
    if field.period_power_f32.len() != TILE_PX * TILE_PX
        || field.period_band_power.len() != TILE_PX * TILE_PX
    {
        return Err(H0V3FieldError::InvalidLength);
    }
    let mut output = BufWriter::new(File::create(path).map_err(|_| H0V3FieldError::Io)?);
    output
        .write_all(&H0_V3_FIELD_MAGIC)
        .map_err(|_| H0V3FieldError::Io)?;
    for value in [
        H0_V3_FIELD_VERSION,
        TILE_PX as u32,
        NUM_PERIODS as u32,
        NUM_BANDS as u32,
        identity.case_index,
        arm_code(identity.arm),
    ] {
        output
            .write_all(&value.to_le_bytes())
            .map_err(|_| H0V3FieldError::Io)?;
    }
    for (period_power, period_band_power) in
        field.period_power_f32.iter().zip(&field.period_band_power)
    {
        for power in period_power {
            if !power.is_finite() || *power < 0.0 {
                return Err(H0V3FieldError::InvalidValue);
            }
            output
                .write_all(&power.to_le_bytes())
                .map_err(|_| H0V3FieldError::Io)?;
        }
        for period in period_band_power {
            for power in period {
                if !power.is_finite() || *power < 0.0 {
                    return Err(H0V3FieldError::InvalidValue);
                }
                output
                    .write_all(&power.to_le_bytes())
                    .map_err(|_| H0V3FieldError::Io)?;
            }
        }
    }
    output.flush().map_err(|_| H0V3FieldError::Io)
}

/// Read and validate one complete field, then convert every cell through the
/// sole frozen V3 observation constructor.
pub fn read_h0_v3_observations(
    path: &Path,
    expected: H0V3FieldIdentity,
) -> Result<Vec<H0V3Observation>, H0V3FieldError> {
    let metadata = std::fs::metadata(path).map_err(|_| H0V3FieldError::Io)?;
    if metadata.len() != H0_V3_FIELD_BYTES as u64 {
        return Err(H0V3FieldError::InvalidLength);
    }
    let mut input = BufReader::new(File::open(path).map_err(|_| H0V3FieldError::Io)?);
    let mut magic = [0_u8; 8];
    input
        .read_exact(&mut magic)
        .map_err(|_| H0V3FieldError::Io)?;
    let mut header = [0_u32; HEADER_U32_FIELDS];
    for value in &mut header {
        let mut bytes = [0_u8; 4];
        input
            .read_exact(&mut bytes)
            .map_err(|_| H0V3FieldError::Io)?;
        *value = u32::from_le_bytes(bytes);
    }
    if magic != H0_V3_FIELD_MAGIC
        || header[0] != H0_V3_FIELD_VERSION
        || header[1] != TILE_PX as u32
        || header[2] != NUM_PERIODS as u32
        || header[3] != NUM_BANDS as u32
        || header[4] != expected.case_index
        || arm_from_code(header[5])? != expected.arm
    {
        return Err(H0V3FieldError::InvalidHeader);
    }
    let mut observations = Vec::with_capacity(TILE_PX * TILE_PX);
    for pixel_index in 0..TILE_PX * TILE_PX {
        let mut period_power = [0.0_f32; NUM_PERIODS];
        for power in &mut period_power {
            let mut bytes = [0_u8; 4];
            input
                .read_exact(&mut bytes)
                .map_err(|_| H0V3FieldError::Io)?;
            *power = f32::from_le_bytes(bytes);
        }
        let mut powers = [[0.0_f64; NUM_BANDS]; NUM_PERIODS];
        for period in &mut powers {
            for power in period {
                let mut bytes = [0_u8; 8];
                input
                    .read_exact(&mut bytes)
                    .map_err(|_| H0V3FieldError::Io)?;
                *power = f64::from_le_bytes(bytes);
            }
        }
        observations.push(
            H0V3Observation::from_accumulated_power(
                (u64::from(expected.case_index) << 32) | pixel_index as u64,
                period_power,
                powers,
            )
            .map_err(|_| H0V3FieldError::InvalidValue)?,
        );
    }
    Ok(observations)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arm_codes_are_complete_and_unique() {
        let arms = [
            H0V3FieldArm::Stock,
            H0V3FieldArm::H0(H0V3Theta::Degrees5),
            H0V3FieldArm::H0(H0V3Theta::Degrees4),
            H0V3FieldArm::H0(H0V3Theta::Degrees3),
            H0V3FieldArm::H0(H0V3Theta::Degrees2),
            H0V3FieldArm::JudgeCoarse,
            H0V3FieldArm::JudgeFine,
        ];
        for arm in arms {
            assert_eq!(arm_from_code(arm_code(arm)), Ok(arm));
        }
    }
}

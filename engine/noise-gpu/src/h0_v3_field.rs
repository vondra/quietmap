//! Sampled binary field shared by the CPU H0 V3 renderer and analyser.
//!
//! Format 3 (`QMV3H0F3`) carries exactly 1,024 receiver keys in ascending
//! order. Both writer and reader bind those keys to the frozen uniform S7
//! sampler, so a truncated, full-resolution, or differently sampled arm fails
//! before scoring. This is the sampled protocol sealed by `ESCALATIONS.md`
//! SHA-256 `6d6b8239f1107cd62d2fc81f063d971c3753e75f8f524ad7c76e42f7acdf1fa3`.

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
use tile_painter::h0_v3_sampler::{h0_v3_sampled_receivers, H0_V3_SAMPLED_RECEIVERS};
use tile_painter::h0_v3_tile_reference::H0V3TileField;

/// Format 3 carries an explicit sampled receiver key list. Format 2 was the
/// implicit full-resolution `TILE_PX * TILE_PX` layout; it is not read here, so
/// a full-resolution field can never be mixed into a sampled arm matrix.
pub const H0_V3_FIELD_MAGIC: [u8; 8] = *b"QMV3H0F3";
pub const H0_V3_FIELD_VERSION: u32 = 3;
const HEADER_U32_FIELDS: usize = 7;
pub const H0_V3_FIELD_HEADER_BYTES: usize = 8 + HEADER_U32_FIELDS * 4;
/// Bytes per emitted receiver: its `u32` key, then period and band powers.
pub const H0_V3_FIELD_BYTES_PER_RECEIVER: usize =
    size_of::<u32>() + NUM_PERIODS * size_of::<f32>() + NUM_PERIODS * NUM_BANDS * size_of::<f64>();

/// Exact on-disk size of a field carrying `receiver_count` receivers.
#[must_use]
pub const fn h0_v3_field_bytes(receiver_count: usize) -> usize {
    H0_V3_FIELD_HEADER_BYTES + receiver_count * H0_V3_FIELD_BYTES_PER_RECEIVER
}

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

/// Write one complete field: a self-describing header, the ascending sampled
/// receiver key list, then one record per sampled receiver in that same order.
/// The key list is the sole authority for which receivers the arm carries, so
/// two arms of a case are comparable only if their key lists are identical.
pub fn write_h0_v3_field(
    path: &Path,
    identity: H0V3FieldIdentity,
    field: &H0V3TileField,
) -> Result<(), H0V3FieldError> {
    let receiver_count = field.receiver_indices.len();
    if receiver_count != H0_V3_SAMPLED_RECEIVERS
        || field.period_power_f32.len() != receiver_count
        || field.period_band_power.len() != receiver_count
    {
        return Err(H0V3FieldError::InvalidLength);
    }
    if field.receiver_indices != h0_v3_sampled_receivers(identity.case_index) {
        return Err(H0V3FieldError::InvalidValue);
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
        receiver_count as u32,
    ] {
        output
            .write_all(&value.to_le_bytes())
            .map_err(|_| H0V3FieldError::Io)?;
    }
    for &index in &field.receiver_indices {
        output
            .write_all(&index.to_le_bytes())
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

/// Read and validate one complete field, then convert every sampled cell
/// through the sole frozen V3 observation constructor. Observation keys are
/// `(case_index << 32) | receiver_index`, so the scorer's strictly-ascending
/// key requirement is inherited directly from the validated key list.
pub fn read_h0_v3_observations(
    path: &Path,
    expected: H0V3FieldIdentity,
) -> Result<Vec<H0V3Observation>, H0V3FieldError> {
    let metadata = std::fs::metadata(path).map_err(|_| H0V3FieldError::Io)?;
    if metadata.len() < H0_V3_FIELD_HEADER_BYTES as u64 {
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
    let receiver_count = header[6] as usize;
    if receiver_count != H0_V3_SAMPLED_RECEIVERS
        || metadata.len() != h0_v3_field_bytes(receiver_count) as u64
    {
        return Err(H0V3FieldError::InvalidLength);
    }
    // The key list is read and validated before any power, so a field whose
    // sampled population is malformed never reaches the scorer.
    let mut receiver_indices = Vec::with_capacity(receiver_count);
    for _ in 0..receiver_count {
        let mut bytes = [0_u8; 4];
        input
            .read_exact(&mut bytes)
            .map_err(|_| H0V3FieldError::Io)?;
        receiver_indices.push(u32::from_le_bytes(bytes));
    }
    if receiver_indices != h0_v3_sampled_receivers(expected.case_index) {
        return Err(H0V3FieldError::InvalidValue);
    }
    let mut observations = Vec::with_capacity(receiver_count);
    for &pixel_index in &receiver_indices {
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
    use std::fs::{self, OpenOptions};
    use std::io::{Seek, SeekFrom};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn test_field_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "quietmap-h0-v3-field-{}-{label}-{}",
            std::process::id(),
            TEST_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn write_zero_field(path: &Path, magic: [u8; 8], case_index: u32, receiver_indices: &[u32]) {
        let mut output = BufWriter::new(File::create(path).unwrap());
        output.write_all(&magic).unwrap();
        for value in [
            H0_V3_FIELD_VERSION,
            TILE_PX as u32,
            NUM_PERIODS as u32,
            NUM_BANDS as u32,
            case_index,
            arm_code(H0V3FieldArm::JudgeFine),
            receiver_indices.len() as u32,
        ] {
            output.write_all(&value.to_le_bytes()).unwrap();
        }
        for receiver_index in receiver_indices {
            output.write_all(&receiver_index.to_le_bytes()).unwrap();
        }
        let zero_observation = vec![0_u8; H0_V3_FIELD_BYTES_PER_RECEIVER - size_of::<u32>()];
        for _ in receiver_indices {
            output.write_all(&zero_observation).unwrap();
        }
        output.flush().unwrap();
    }

    fn zero_tile_field(receiver_indices: Vec<u32>) -> H0V3TileField {
        let receiver_count = receiver_indices.len();
        H0V3TileField {
            receiver_indices,
            period_power_f32: vec![[0.0; NUM_PERIODS]; receiver_count],
            period_band_power: vec![[[0.0; NUM_BANDS]; NUM_PERIODS]; receiver_count],
            evaluated_pair_count: 0,
            evaluated_node_count: 0,
            admitted_node_count: 0,
            maximum_distinct_hint_records: 0,
            maximum_unique_u_hints: 0,
            maximum_logical_hint_storage_bytes: 0,
        }
    }

    fn overwrite_u32(path: &Path, offset: u64, value: u32) {
        let mut output = OpenOptions::new().write(true).open(path).unwrap();
        output.seek(SeekFrom::Start(offset)).unwrap();
        output.write_all(&value.to_le_bytes()).unwrap();
        output.flush().unwrap();
    }

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

    #[test]
    fn sampled_format_three_parses_the_frozen_ascending_key_list() {
        let path = test_field_path("valid");
        let receiver_indices = h0_v3_sampled_receivers(0);
        write_zero_field(&path, H0_V3_FIELD_MAGIC, 0, &receiver_indices);
        let observations = read_h0_v3_observations(
            &path,
            H0V3FieldIdentity {
                case_index: 0,
                arm: H0V3FieldArm::JudgeFine,
            },
        )
        .unwrap();
        fs::remove_file(path).unwrap();

        assert_eq!(observations.len(), H0_V3_SAMPLED_RECEIVERS);
        assert_eq!(observations[0].key, u64::from(receiver_indices[0]));
        assert_eq!(
            observations.last().unwrap().key,
            u64::from(*receiver_indices.last().unwrap())
        );
    }

    #[test]
    fn truncated_mixed_and_differently_sampled_fields_fail_closed() {
        let identity = H0V3FieldIdentity {
            case_index: 0,
            arm: H0V3FieldArm::JudgeFine,
        };
        let receiver_indices = h0_v3_sampled_receivers(0);

        let truncated = test_field_path("truncated");
        write_zero_field(&truncated, H0_V3_FIELD_MAGIC, 0, &receiver_indices);
        let length = fs::metadata(&truncated).unwrap().len();
        OpenOptions::new()
            .write(true)
            .open(&truncated)
            .unwrap()
            .set_len(length - 1)
            .unwrap();
        assert_eq!(
            read_h0_v3_observations(&truncated, identity),
            Err(H0V3FieldError::InvalidLength)
        );
        fs::remove_file(truncated).unwrap();

        let mixed_format = test_field_path("format-two");
        write_zero_field(&mixed_format, *b"QMV3H0F2", 0, &receiver_indices);
        assert_eq!(
            read_h0_v3_observations(&mixed_format, identity),
            Err(H0V3FieldError::InvalidHeader)
        );
        fs::remove_file(mixed_format).unwrap();

        let wrong_sample = test_field_path("wrong-sample");
        write_zero_field(
            &wrong_sample,
            H0_V3_FIELD_MAGIC,
            0,
            &h0_v3_sampled_receivers(1),
        );
        assert_eq!(
            read_h0_v3_observations(&wrong_sample, identity),
            Err(H0V3FieldError::InvalidValue)
        );
        fs::remove_file(wrong_sample).unwrap();
    }

    #[test]
    fn writer_rejects_wrong_receiver_count_and_key_list() {
        let identity = H0V3FieldIdentity {
            case_index: 0,
            arm: H0V3FieldArm::JudgeFine,
        };
        let frozen = h0_v3_sampled_receivers(0);
        for count in [H0_V3_SAMPLED_RECEIVERS - 1, H0_V3_SAMPLED_RECEIVERS + 1] {
            let path = test_field_path("writer-count");
            let mut keys = frozen.clone();
            keys.truncate(count);
            if count > keys.len() {
                keys.push(u32::MAX);
            }
            let field = zero_tile_field(keys);
            assert_eq!(
                write_h0_v3_field(&path, identity, &field),
                Err(H0V3FieldError::InvalidLength)
            );
            assert!(!path.exists());
        }

        let path = test_field_path("writer-keys");
        let mut wrong_keys = frozen;
        wrong_keys.swap(0, 1);
        let field = zero_tile_field(wrong_keys);
        assert_eq!(
            write_h0_v3_field(&path, identity, &field),
            Err(H0V3FieldError::InvalidValue)
        );
        assert!(!path.exists());
    }

    #[test]
    fn reader_rejects_header_tamper_and_overlength() {
        let identity = H0V3FieldIdentity {
            case_index: 0,
            arm: H0V3FieldArm::JudgeFine,
        };
        let frozen = h0_v3_sampled_receivers(0);

        let header_tamper = test_field_path("header-tamper");
        write_zero_field(&header_tamper, H0_V3_FIELD_MAGIC, 0, &frozen);
        overwrite_u32(&header_tamper, 8, H0_V3_FIELD_VERSION + 1);
        assert_eq!(
            read_h0_v3_observations(&header_tamper, identity),
            Err(H0V3FieldError::InvalidHeader)
        );
        fs::remove_file(header_tamper).unwrap();

        let count_tamper = test_field_path("count-tamper");
        write_zero_field(&count_tamper, H0_V3_FIELD_MAGIC, 0, &frozen);
        overwrite_u32(
            &count_tamper,
            (H0_V3_FIELD_HEADER_BYTES - size_of::<u32>()) as u64,
            (H0_V3_SAMPLED_RECEIVERS - 1) as u32,
        );
        assert_eq!(
            read_h0_v3_observations(&count_tamper, identity),
            Err(H0V3FieldError::InvalidLength)
        );
        fs::remove_file(count_tamper).unwrap();

        let overlength = test_field_path("overlength");
        write_zero_field(&overlength, H0_V3_FIELD_MAGIC, 0, &frozen);
        OpenOptions::new()
            .append(true)
            .open(&overlength)
            .unwrap()
            .write_all(&[0])
            .unwrap();
        assert_eq!(
            read_h0_v3_observations(&overlength, identity),
            Err(H0V3FieldError::InvalidLength)
        );
        fs::remove_file(overlength).unwrap();
    }

    #[test]
    fn reader_rejects_nonascending_and_duplicate_keys() {
        let identity = H0V3FieldIdentity {
            case_index: 0,
            arm: H0V3FieldArm::JudgeFine,
        };
        let frozen = h0_v3_sampled_receivers(0);
        for (label, mut keys) in [("nonascending", frozen.clone()), ("duplicate", frozen)] {
            if label == "nonascending" {
                keys.swap(0, 1);
            } else {
                keys[1] = keys[0];
            }
            let path = test_field_path(label);
            write_zero_field(&path, H0_V3_FIELD_MAGIC, 0, &keys);
            assert_eq!(
                read_h0_v3_observations(&path, identity),
                Err(H0V3FieldError::InvalidValue)
            );
            fs::remove_file(path).unwrap();
        }
    }
}

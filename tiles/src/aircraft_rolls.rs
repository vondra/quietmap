//! `aircraft-rolls` tiles: the take-off and landing ground rolls that start in the tile, one per
//! runway direction, operation and noise class, with their movements on an average day by period
//! (written by the airport traffic pass, heard through `physics::doc29::roll`).
//!
//! ```text
//! magic "qmroll1\n", u32 rolls
//! per roll, 26 B: start 2 x i16 (tile-local steps), heading u16 (2^-16 of a turn anticlockwise
//!   from east), length u16 (m), speed u16 (0.01 m/s), field elevation i16 (m), class u8,
//!   landing u8, movements a day per period 3 x f32
//! ```

use crate::FormatError;
use crate::aircraft::PERIODS;

const MAGIC: &[u8; 8] = b"qmroll1\n";
const HEADER_BYTES: usize = 12;
const ROLL_BYTES: usize = 26;
const HEADING_STEPS_PER_TURN: f64 = 65_536.0;
const SPEED_STEPS_PER_MS: f64 = 100.0;

/// One ground roll: a take-off from its start of roll to lift-off, or a landing from touchdown to
/// where it leaves the runway, of one noise class on one runway direction.
#[derive(Clone, Debug, PartialEq)]
pub struct Roll {
    /// Tile-local steps of its start: the start of roll or the touchdown point.
    pub start: [i16; 2],
    /// Direction of travel, radians anticlockwise from east (2^-16 of a turn).
    pub heading_rad: f64,
    /// Length (1 m steps), and the speed at lift-off or at touchdown (0.01 m/s steps).
    pub length_m: f64,
    pub speed_ms: f64,
    /// The runway's elevation (m above sea level, 1 m steps).
    pub field_m: f64,
    /// The Doc 29 noise class.
    pub class: u8,
    pub landing: bool,
    /// Movements of an average day per period (day, evening, night).
    pub movements_per_day: [f64; PERIODS],
}

/// A tile's rolls in the format above.
pub fn encode(rolls: &[Roll]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HEADER_BYTES + ROLL_BYTES * rolls.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&(rolls.len() as u32).to_le_bytes());
    for roll in rolls {
        let turn = roll.heading_rad.rem_euclid(std::f64::consts::TAU) / std::f64::consts::TAU;
        let heading = (turn * HEADING_STEPS_PER_TURN).round() as u32 % 65_536;
        let in_range = |value: f64, low: f64, high: f64| {
            assert!(
                (low..=high).contains(&value),
                "a roll out of range: {roll:?}"
            );
            value
        };
        bytes.extend_from_slice(&roll.start[0].to_le_bytes());
        bytes.extend_from_slice(&roll.start[1].to_le_bytes());
        bytes.extend_from_slice(&(heading as u16).to_le_bytes());
        let length = in_range(roll.length_m.round(), 0.0, 65_535.0) as u16;
        bytes.extend_from_slice(&length.to_le_bytes());
        let speed = in_range((roll.speed_ms * SPEED_STEPS_PER_MS).round(), 0.0, 65_535.0) as u16;
        bytes.extend_from_slice(&speed.to_le_bytes());
        let field = in_range(roll.field_m.round(), -32_768.0, 32_767.0) as i16;
        bytes.extend_from_slice(&field.to_le_bytes());
        bytes.push(roll.class);
        bytes.push(u8::from(roll.landing));
        for movements in roll.movements_per_day {
            bytes.extend_from_slice(&(movements as f32).to_le_bytes());
        }
    }
    bytes
}

/// The rolls of a tile's bytes.
pub fn parse(bytes: &[u8]) -> Result<Vec<Roll>, FormatError> {
    if bytes.len() < HEADER_BYTES || &bytes[..8] != MAGIC {
        return Err(FormatError("aircraft-rolls: bad magic"));
    }
    let count = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    if bytes.len() != HEADER_BYTES + ROLL_BYTES * count {
        return Err(FormatError(
            "aircraft-rolls: the length does not match the count",
        ));
    }
    Ok(bytes[HEADER_BYTES..]
        .chunks_exact(ROLL_BYTES)
        .map(|record| {
            let u16_at = |at: usize| u16::from_le_bytes([record[at], record[at + 1]]);
            let i16_at = |at: usize| i16::from_le_bytes([record[at], record[at + 1]]);
            let f32_at = |at: usize| f32::from_le_bytes(record[at..at + 4].try_into().unwrap());
            Roll {
                start: [i16_at(0), i16_at(2)],
                heading_rad: f64::from(u16_at(4)) / HEADING_STEPS_PER_TURN * std::f64::consts::TAU,
                length_m: f64::from(u16_at(6)),
                speed_ms: f64::from(u16_at(8)) / SPEED_STEPS_PER_MS,
                field_m: f64::from(i16_at(10)),
                class: record[12],
                landing: record[13] != 0,
                movements_per_day: std::array::from_fn(|period| f64::from(f32_at(14 + 4 * period))),
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A take-off and a landing read back within their steps; a truncated tile fails.
    #[test]
    fn rolls_read_back_as_written() {
        let rolls = vec![
            Roll {
                start: [-12_000, 30_000],
                heading_rad: 4.1,
                length_m: 1_800.0,
                speed_ms: 77.17,
                field_m: 380.0,
                class: 7,
                landing: false,
                movements_per_day: [101.25, 12.5, 0.75],
            },
            Roll {
                start: [5, -5],
                heading_rad: 0.96,
                length_m: 1_400.0,
                speed_ms: 66.88,
                field_m: -2.0,
                class: 1,
                landing: true,
                movements_per_day: [0.0, 0.0, 3.0],
            },
        ];
        let bytes = encode(&rolls);
        assert_eq!(bytes.len(), HEADER_BYTES + 2 * ROLL_BYTES);
        let read = parse(&bytes).unwrap();
        for (read, written) in read.iter().zip(&rolls) {
            assert!((read.heading_rad - written.heading_rad).abs() < 1e-4);
            assert_eq!(
                Roll {
                    heading_rad: written.heading_rad,
                    ..read.clone()
                },
                *written
            );
        }
        assert!(parse(&bytes[..bytes.len() - 1]).is_err());
    }
}

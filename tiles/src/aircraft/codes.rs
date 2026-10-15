//! The numbers an aircraft file stores, each code beside its inverse: levels and LAmax, the NPD
//! curves as drops, flight counts, a centroid's place in its cell and a piece's ends from it.
//! Where a code is coarser than the value, its step is far below what the value moves: a level
//! within 0.025 dB, a scaled distance within 0.5 %, a piece's end within 1/128 of its cell.

use super::NPD_DISTANCES;

/// Stored level = code / 100 - 100 dB; code 0 is silence.
const LEVEL_OFFSET_DB: f64 = 100.0;
/// A box's loudest LAmax = code / 2 + 30 dB, rounded up; code 0 is none.
const LAMAX_OFFSET_DB: f64 = 30.0;
/// A curve's next level is stored as its drop from the one before, in steps of 5 level codes
/// (0.05 dB): the drops between NPD distances lie within 1.3 and 9.6 dB (all of r064's boxes).
const DROP_CODES: i64 = 5;
/// The tail anchor's level below the curve's last, in 0.1 dB (6.4 to 21.6 dB in r064).
const TAIL_DROP_CODES: i64 = 10;
/// lg of a scaled distance in 1e-4 steps; the next distance's change in steps of 40 (0.004:
/// 0.9 %; the changes lie within -0.21 and +0.45 in r064).
const LG_SCALE: f64 = 10_000.0;
const LG_STEP_CODES: i64 = 40;
/// A flight count below this is stored as it is; above, with 12 bits of mantissa.
const EXACT_FLIGHTS: u32 = 4_096;

pub(super) fn level_code(level_db: f64) -> u16 {
    if level_db == f64::NEG_INFINITY {
        return 0;
    }
    let code = ((level_db + LEVEL_OFFSET_DB) * 100.0).round();
    assert!(
        (1.0..=f64::from(u16::MAX)).contains(&code),
        "aircraft energy {level_db} dB outside the u16 range"
    );
    code as u16
}

pub(super) fn level_db(code: i64) -> f64 {
    if code == 0 {
        f64::NEG_INFINITY
    } else {
        code as f64 / 100.0 - LEVEL_OFFSET_DB
    }
}

pub(super) fn lamax_code(lamax_db: f64) -> u8 {
    if lamax_db == f64::NEG_INFINITY {
        return 0;
    }
    (2.0 * (lamax_db - LAMAX_OFFSET_DB))
        .ceil()
        .clamp(1.0, 255.0) as u8
}

pub(super) fn lamax_db(code: u8) -> f64 {
    if code == 0 {
        f64::NEG_INFINITY
    } else {
        f64::from(code) / 2.0 + LAMAX_OFFSET_DB
    }
}

/// An NPD curve's ten levels (falling with distance) as the first level's code and nine drops;
/// each drop is taken from the level the stored ones reach, so no level strays more than half a
/// step. Also the code the stored curve ends at.
pub(super) fn curve_codes(levels: &[f64; NPD_DISTANCES]) -> (u16, [u8; NPD_DISTANCES - 1], i64) {
    assert!(
        levels.iter().all(|level| level.is_finite()),
        "a heard period's NPD levels are all finite: {levels:?}"
    );
    let first = level_code(levels[0]);
    let mut reached = i64::from(first);
    let drops = std::array::from_fn(|k| {
        let drop = (reached - i64::from(level_code(levels[k + 1])) + DROP_CODES / 2)
            .div_euclid(DROP_CODES);
        assert!(
            (0..=255).contains(&drop) && reached - drop * DROP_CODES >= 1,
            "an NPD curve dropping {} dB to {} dB",
            levels[k] - levels[k + 1],
            levels[k + 1]
        );
        reached -= drop * DROP_CODES;
        drop as u8
    });
    (first, drops, reached)
}

/// The levels of a stored curve and the code it ends at.
pub(super) fn curve_levels(first: u16, drops: &[u8]) -> ([f64; NPD_DISTANCES], i64) {
    let mut reached = i64::from(first);
    let levels = std::array::from_fn(|k| {
        if k > 0 {
            reached -= i64::from(drops[k - 1]) * DROP_CODES;
        }
        level_db(reached)
    });
    (levels, reached)
}

/// The tail anchor's level as its drop below the curve's last stored code.
pub(super) fn tail_code(curve_end: i64, tail_db: f64) -> u8 {
    let drop = (curve_end - i64::from(level_code(tail_db)) + TAIL_DROP_CODES / 2)
        .div_euclid(TAIL_DROP_CODES);
    assert!(
        tail_db.is_finite() && (0..=255).contains(&drop) && curve_end - drop * TAIL_DROP_CODES >= 1,
        "a tail anchor {tail_db} dB below a curve ending at code {curve_end}"
    );
    drop as u8
}

pub(super) fn tail_db(curve_end: i64, code: u8) -> f64 {
    level_db(curve_end - i64::from(code) * TAIL_DROP_CODES)
}

/// lg of the scaled distances as the first value and nine changes, each taken from the value the
/// stored ones reach.
pub(super) fn lg_distance_codes(lg: &[f64; NPD_DISTANCES]) -> (u16, [i8; NPD_DISTANCES - 1]) {
    let code = |value: f64| {
        let code = (value.max(0.0) * LG_SCALE).round();
        assert!(code <= f64::from(u16::MAX), "scaled distance 10^{value} m");
        code as i64
    };
    let first = code(lg[0]);
    let mut reached = first;
    let steps = std::array::from_fn(|k| {
        let change = code(lg[k + 1]) - reached;
        let step = (change + LG_STEP_CODES / 2).div_euclid(LG_STEP_CODES);
        assert!(
            (-128..=127).contains(&step) && reached + step * LG_STEP_CODES >= 0,
            "a scaled distance changing by {change} codes"
        );
        reached += step * LG_STEP_CODES;
        step as i8
    });
    (first as u16, steps)
}

pub(super) fn lg_distances(first: u16, steps: &[u8]) -> [f64; NPD_DISTANCES] {
    let mut reached = i64::from(first);
    std::array::from_fn(|k| {
        if k > 0 {
            reached += i64::from(steps[k - 1] as i8) * LG_STEP_CODES;
        }
        reached as f64 / LG_SCALE
    })
}

/// A flight count in 16 bits: below 4,096 as it is, above as a 4-bit exponent and a 12-bit
/// mantissa (within 0.013 %).
pub(super) fn flights_code(count: u32) -> u16 {
    if count < EXACT_FLIGHTS {
        return count as u16;
    }
    let mut exponent = 32 - count.leading_zeros() - 12;
    let mut mantissa = (f64::from(count) / f64::from(1u32 << (exponent - 1))).round() as u32;
    if mantissa == 2 * EXACT_FLIGHTS {
        (exponent, mantissa) = (exponent + 1, EXACT_FLIGHTS);
    }
    assert!(exponent < 16, "{count} flights");
    (exponent << 12 | (mantissa - EXACT_FLIGHTS)) as u16
}

pub(super) fn flights(code: u16) -> u32 {
    let (exponent, mantissa) = (u32::from(code >> 12), u32::from(code & 0x0fff));
    if exponent == 0 {
        mantissa
    } else {
        (EXACT_FLIGHTS + mantissa) << (exponent - 1)
    }
}

/// A cell's edge in tile steps (32,768 a tile) at `zoom`.
pub(super) fn cell_steps(zoom: u8) -> i32 {
    32_768 >> (zoom - 12)
}

/// The tile step of a cell's west (or north) edge.
fn cell_start(cell: u8, edge: i32) -> i32 {
    -16_384 + i32::from(cell) * edge
}

/// A centroid coordinate as its place in its cell, in 1/256 of the edge (exact where the edge is
/// 256 steps or less: zoom 19 and 20). The centroid of a box's pieces lies in its cell, its far
/// edge included.
pub(super) fn in_cell_code(coordinate: i16, cell: u8, edge: i32) -> u8 {
    let place = i32::from(coordinate) - cell_start(cell, edge);
    assert!(
        (0..=edge).contains(&place),
        "a box centroid {place} steps into a cell of {edge}"
    );
    (place * 256 / edge).min(255) as u8
}

/// The coordinate a place in a cell stands for: the centre of its step.
pub(super) fn in_cell(code: u8, cell: u8, edge: i32) -> i16 {
    (cell_start(cell, edge) + (2 * i32::from(code) + 1) * edge / 512) as i16
}

/// A piece's end coordinate as its offset from the box's centroid in 1/64 of the cell edge (a
/// piece ends within 1.2 edges of its box's centroid).
pub(super) fn offset_code(coordinate: i16, centroid: i16, edge: i32) -> i8 {
    let offset = i32::from(coordinate) - i32::from(centroid);
    let code = (2 * offset * 64 + edge).div_euclid(2 * edge);
    assert!(
        (-128..=127).contains(&code),
        "a piece end {offset} steps from its box's centroid in a cell of {edge}"
    );
    code as i8
}

pub(super) fn offset(code: u8, centroid: i16, edge: i32) -> i16 {
    (i32::from(centroid) + i32::from(code as i8) * edge / 64) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_curve_reads_back_within_half_a_step() {
        let levels: [f64; NPD_DISTANCES] =
            std::array::from_fn(|k| 101.237 - 4.913 * k as f64 - 0.0371 * (k * k) as f64);
        let (first, drops, end) = curve_codes(&levels);
        let (read, read_end) = curve_levels(first, &drops);
        assert_eq!(end, read_end);
        for (r, l) in read.iter().zip(levels) {
            assert!((r - l).abs() <= 0.0251, "{r} vs {l}");
        }
        let tail = tail_db(end, tail_code(end, levels[9] - 12.34));
        assert!((tail - (levels[9] - 12.34)).abs() <= 0.051, "{tail}");
        let lg: [f64; NPD_DISTANCES] =
            std::array::from_fn(|k| 1.8 + 0.27 * k as f64 - 0.01 * (k * k) as f64);
        let (first, steps) = lg_distance_codes(&lg);
        let steps: Vec<u8> = steps.iter().map(|&s| s as u8).collect();
        for (r, l) in lg_distances(first, &steps).iter().zip(lg) {
            assert!((r - l).abs() <= 0.00201, "{r} vs {l}");
        }
    }

    #[test]
    fn flight_counts_are_exact_below_4096_and_within_0_013_percent_above() {
        for count in [
            0, 1, 4_095, 4_096, 8_191, 8_192, 65_535, 200_001, 16_777_215,
        ] {
            let read = flights(flights_code(count));
            assert!(
                (f64::from(read) - f64::from(count)).abs() <= f64::from(count) * 1.3e-4,
                "{count} reads {read}"
            );
            if count < 4_096 {
                assert_eq!(read, count);
            }
        }
    }

    #[test]
    fn a_centroid_and_a_piece_end_read_back_within_their_steps() {
        for zoom in [12, 15, 19, 20] {
            let edge = cell_steps(zoom);
            let cell = (32_768 / edge - 1).min(3) as u8;
            for place in [0, 1, edge / 3, edge - 1] {
                let coordinate = (-16_384 + i32::from(cell) * edge + place) as i16;
                let read = in_cell(in_cell_code(coordinate, cell, edge), cell, edge);
                let step = (edge / 256).max(1);
                assert!((i32::from(read) - i32::from(coordinate)).abs() <= step / 2 + 1);
                if edge <= 256 {
                    assert_eq!(read, coordinate, "exact at zoom {zoom}");
                }
                let end = (i32::from(coordinate) + edge * 6 / 5).min(16_383) as i16;
                let back = offset(offset_code(end, coordinate, edge) as u8, coordinate, edge);
                assert!((i32::from(back) - i32::from(end)).abs() <= edge / 128 + 1);
            }
        }
    }
}

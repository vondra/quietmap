//! The numbers an aircraft file stores, each code beside its inverse: levels and LAmax, the NPD
//! curves as drops, the scaled distances as steps, a centroid's place in its cell and a piece's
//! ends from it (flights a day are `crate::rate`'s). Where a code is coarser than the value, its step is far below what
//! the value moves: a level within 0.025 dB (the tail anchor 0.05 dB), a scaled distance within
//! 0.002 of its lg, a piece's end within a step (0.3 m) for nearly every piece and exact for the
//! farthest.

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
/// lg of a box's first scaled distance in 1e-4 steps.
const LG_SCALE: f64 = 10_000.0;
/// The next scaled distances as their lg's steps from the one before, in 0.004 (40 codes of the
/// first's), from -0.3 (code 0) to +0.72 (code 255): r064's steps lie within -0.21 and +0.56
/// (325 million of the stratified sample, `lg-steps-r064.txt`).
const LG_STEP_CODES: i64 = 40;
const LG_STEP_BIAS: i64 = 75;

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
/// each drop is taken from the level the stored ones reach to the next level itself (one
/// rounding), so no level strays more than half a step. Also the code the stored curve ends at.
pub(super) fn curve_codes(levels: &[f64; NPD_DISTANCES]) -> (u16, [u8; NPD_DISTANCES - 1], i64) {
    assert!(
        levels.iter().all(|level| level.is_finite()),
        "a heard period's NPD levels are all finite: {levels:?}"
    );
    let first = level_code(levels[0]);
    let mut reached = i64::from(first);
    let drops = std::array::from_fn(|k| {
        let drop = ((level_db(reached) - levels[k + 1]) * 100.0 / DROP_CODES as f64).round() as i64;
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

/// The tail anchor's level as its drop below the curve's last stored code, rounded once.
pub(super) fn tail_code(curve_end: i64, tail_db: f64) -> u8 {
    let drop = ((level_db(curve_end) - tail_db) * 100.0 / TAIL_DROP_CODES as f64).round() as i64;
    assert!(
        tail_db.is_finite() && (0..=255).contains(&drop) && curve_end - drop * TAIL_DROP_CODES >= 1,
        "a tail anchor {tail_db} dB below a curve ending at code {curve_end}"
    );
    drop as u8
}

pub(super) fn tail_db(curve_end: i64, code: u8) -> f64 {
    level_db(curve_end - i64::from(code) * TAIL_DROP_CODES)
}

/// The lg of a box's ten scaled distances as the first's code and nine steps, each taken from the
/// value the stored ones reach, so none strays more than half a step.
pub(super) fn lg_distance_codes(lg: &[f64; NPD_DISTANCES]) -> (u16, [u8; NPD_DISTANCES - 1]) {
    let code = |lg: f64| (lg.max(0.0) * LG_SCALE).round() as i64;
    let first = code(lg[0]);
    assert!(
        first <= i64::from(u16::MAX),
        "scaled distance 10^{} m",
        lg[0]
    );
    let mut reached = first;
    let steps = std::array::from_fn(|k| {
        // From the value the stored ones reach to the next value itself: one rounding.
        let step = ((lg[k + 1].max(0.0) * LG_SCALE - reached as f64) / LG_STEP_CODES as f64).round()
            as i64;
        let stored = step + LG_STEP_BIAS;
        assert!(
            (0..=255).contains(&stored),
            "a scaled distance's lg stepping {} between NPD distances",
            lg[k + 1] - lg[k]
        );
        reached += step * LG_STEP_CODES;
        stored as u8
    });
    (first as u16, steps)
}

pub(super) fn lg_distances(first: u16, steps: &[u8]) -> [f64; NPD_DISTANCES] {
    let mut reached = i64::from(first);
    std::array::from_fn(|k| {
        if k > 0 {
            reached += (i64::from(steps[k - 1]) - LG_STEP_BIAS) * LG_STEP_CODES;
        }
        reached as f64 / LG_SCALE
    })
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
/// 256 steps or less, zoom 19 and 20, but on the cell's far edge, a step short there). The
/// centroid of a box's pieces lies in its cell, its far edge included.
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

/// The tile steps of a piece end's code at each scale: a byte holds +-254, +-1,016 and +-4,064
/// steps from the box's centroid (within 0.3, 1.2 and 4.8 m, under ADS-B's positions' 10 m).
/// r064's ends lie within 2,083 steps of their centroid but for one in a thousand, within 3,261 but
/// for one in ten thousand (`box-ranges-r064.txt`); a piece farther keeps its ends exact.
const OFFSET_STEPS: [i32; 3] = [2, 8, 32];
/// The scale of a piece whose ends are kept exact in the file's table of wide pieces.
pub(super) const WIDE_SCALE: u8 = OFFSET_STEPS.len() as u8;

/// The finest scale whose codes hold a piece's ends from its box's centroid, [`WIDE_SCALE`] when
/// none does.
pub(super) fn offset_scale(ends: &[[i16; 2]; 2], centroid: [i16; 2]) -> u8 {
    (0..WIDE_SCALE)
        .find(|&scale| {
            ends.iter()
                .flatten()
                .zip(centroid.iter().cycle())
                .all(|(&end, &centre)| {
                    let offset = i32::from(end) - i32::from(centre);
                    let step = OFFSET_STEPS[usize::from(scale)];
                    (-128..=127).contains(&(2 * offset + step).div_euclid(2 * step))
                })
        })
        .unwrap_or(WIDE_SCALE)
}

/// A piece's end coordinate as its offset from the box's centroid in steps of `scale`.
pub(super) fn offset_code(coordinate: i16, centroid: i16, scale: u8) -> i8 {
    let (offset, step) = (
        i32::from(coordinate) - i32::from(centroid),
        OFFSET_STEPS[usize::from(scale)],
    );
    let code = (2 * offset + step).div_euclid(2 * step);
    assert!(
        (-128..=127).contains(&code),
        "a piece end {offset} steps from its box's centroid at scale {scale}"
    );
    code as i8
}

pub(super) fn offset(code: u8, centroid: i16, scale: u8) -> i16 {
    (i32::from(centroid) + i32::from(code as i8) * OFFSET_STEPS[usize::from(scale)])
        .clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A piece far from its box's centroid takes the coarser scale it needs and reads back within
    /// half its step; one beyond the coarsest is wide.
    #[test]
    fn a_long_piece_takes_a_coarser_scale() {
        let centroid = [-16_384i16, 16_384];
        for (ends, scale) in [
            ([[-16_300, 16_200], [-16_184, 16_300]], 0),
            ([[-15_900, 16_384], [-16_384, 16_000]], 1),
            ([[-13_000, 16_384], [-16_384, 16_000]], 2),
        ] {
            assert_eq!(offset_scale(&ends, centroid), scale);
            for &[x, y] in &ends {
                for (value, centre) in [(x, centroid[0]), (y, centroid[1])] {
                    let read = offset(offset_code(value, centre, scale) as u8, centre, scale);
                    let step = OFFSET_STEPS[usize::from(scale)];
                    assert!((i32::from(read) - i32::from(value)).abs() <= step / 2);
                }
            }
        }
        assert_eq!(
            offset_scale(&[[16_384, -16_384], [0, 0]], centroid),
            WIDE_SCALE
        );
    }

    /// A box's scaled distances read back within half a step of 0.004, a step down included.
    #[test]
    fn scaled_distances_read_back_within_half_a_step() {
        let lg: [f64; NPD_DISTANCES] = [
            1.78533, 2.17609, 2.45939, 2.69897, 3.04139, 3.40926, 3.62941, 3.83136, 3.6213, 4.18,
        ];
        let (first, steps) = lg_distance_codes(&lg);
        for (read, written) in lg_distances(first, &steps).iter().zip(lg) {
            assert!(
                (read - written).abs() <= 0.002 + 1e-9,
                "{read} vs {written}"
            );
        }
        let (first, steps) = lg_distance_codes(&std::array::from_fn(|k| 2.0 + 0.40196 * k as f64));
        assert!((lg_distances(first, &steps)[1] - 2.40196).abs() <= 0.002 + 1e-9);
    }

    #[test]
    fn a_curve_reads_back_within_half_a_step() {
        let levels: [f64; NPD_DISTANCES] =
            std::array::from_fn(|k| 101.237 - 4.913 * k as f64 - 0.0371 * (k * k) as f64);
        let (first, drops, end) = curve_codes(&levels);
        let (read, read_end) = curve_levels(first, &drops);
        assert_eq!(end, read_end);
        for (r, l) in read.iter().zip(levels) {
            assert!((r - l).abs() <= 0.025 + 1e-9, "{r} vs {l}");
        }
        let tail = tail_db(end, tail_code(end, levels[9] - 12.34));
        assert!((tail - (levels[9] - 12.34)).abs() <= 0.05 + 1e-9, "{tail}");
        // A tail of 93.554 dB below a curve ending at 100 dB: once rounded, 93.55 is 93.6 not 93.5.
        assert!((tail_db(20_000, tail_code(20_000, 93.554)) - 93.554).abs() <= 0.05);
    }

    #[test]
    fn a_centroid_reads_back_within_its_step() {
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
                if place == 0 && edge <= 256 {
                    // The far edge: a step short.
                    let far = (i32::from(coordinate) + edge) as i16;
                    let back = in_cell(in_cell_code(far, cell, edge), cell, edge);
                    assert!((i32::from(back) - i32::from(far)).abs() <= 1);
                }
            }
        }
    }
}

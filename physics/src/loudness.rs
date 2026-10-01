//! Loudness of a steady sound from its third-octave band levels: Zwicker's method of ISO 532-1:2017
//! (DIN 45631, the BASIC program of Zwicker and Fastl 1991) for a frontal free field. The levels
//! of the bands up to 250 Hz are corrected by the equal loudness contours and merged into the three
//! lowest critical bands; each of the 20 critical band levels gives a main (core) loudness; the
//! specific loudness pattern rises at each band's core loudness and falls on the upper masking
//! slopes, and its area over the critical band rate is the loudness in sone (1 sone: a 1 kHz tone
//! at 40 dB; twice the sone, twice as loud).

use crate::bands::BANDS;

/// Third-octave bands of the method, 25 Hz to 12.5 kHz.
pub const THIRD_OCTAVES: usize = 28;
/// A-weighting of those bands (IEC 61672-1, dB).
pub const A_WEIGHTING_DB: [f64; THIRD_OCTAVES] = [
    -44.7, -39.4, -34.6, -30.2, -26.2, -22.5, -19.1, -16.1, -13.4, -10.9, -8.6, -6.6, -4.8, -3.2,
    -1.9, -0.8, 0.0, 0.6, 1.0, 1.2, 1.3, 1.2, 1.0, 0.5, -0.1, -1.1, -2.5, -4.3,
];
/// The band of 63 Hz, the first octave band's centre.
pub const OCTAVE_START: usize = 4;
/// Bands below this level are silent (dB).
const SILENT_DB: f64 = -60.0;

/// Ranges of third-octave levels (dB) for the low-frequency corrections.
const RAP: [f64; 8] = [45.0, 55.0, 65.0, 71.0, 80.0, 90.0, 100.0, 120.0];
/// Reduction of the levels of the bands 25-250 Hz within the ranges of [`RAP`] (dB).
const DLL: [[f64; 11]; 8] = [
    [
        -32.0, -24.0, -16.0, -10.0, -5.0, 0.0, -7.0, -3.0, 0.0, -2.0, 0.0,
    ],
    [
        -29.0, -22.0, -15.0, -10.0, -4.0, 0.0, -7.0, -2.0, 0.0, -2.0, 0.0,
    ],
    [
        -27.0, -19.0, -14.0, -9.0, -4.0, 0.0, -6.0, -2.0, 0.0, -2.0, 0.0,
    ],
    [
        -25.0, -17.0, -12.0, -9.0, -3.0, 0.0, -5.0, -2.0, 0.0, -2.0, 0.0,
    ],
    [
        -23.0, -16.0, -11.0, -7.0, -3.0, 0.0, -4.0, -1.0, 0.0, -1.0, 0.0,
    ],
    [
        -20.0, -14.0, -10.0, -6.0, -3.0, 0.0, -4.0, -1.0, 0.0, -1.0, 0.0,
    ],
    [
        -18.0, -12.0, -9.0, -6.0, -2.0, 0.0, -3.0, -1.0, 0.0, -1.0, 0.0,
    ],
    [
        -15.0, -10.0, -8.0, -4.0, -2.0, 0.0, -3.0, -1.0, 0.0, -1.0, 0.0,
    ],
];
/// Critical band level at the absolute threshold, without the ear's transmission (dB).
const LTQ: [f64; 20] = [
    30.0, 18.0, 12.0, 8.0, 7.0, 6.0, 5.0, 4.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0,
    3.0, 3.0,
];
/// The ear's transmission (dB).
const A0: [f64; 20] = [
    0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -0.5, -1.6, -3.2, -5.4, -5.6, -4.0, -1.5,
    2.0, 5.0, 12.0,
];
/// Third-octave to critical band level adaptation (dB).
const DCB: [f64; 20] = [
    -0.25, -0.6, -0.8, -0.8, -0.5, 0.0, 0.5, 1.1, 1.5, 1.7, 1.8, 1.8, 1.7, 1.6, 1.4, 1.2, 0.8, 0.5,
    0.0, -0.5,
];
/// Upper limits of the approximated critical bands (Bark).
const ZUP: [f64; 21] = [
    0.9, 1.8, 2.8, 3.5, 4.4, 5.4, 6.6, 7.9, 9.2, 10.6, 12.3, 13.8, 15.2, 16.7, 18.1, 19.3, 20.6,
    21.8, 22.7, 23.6, 24.0,
];
/// Ranges of specific loudness for the steepness of the upper slopes (sone/Bark).
const RNS: [f64; 18] = [
    21.5, 18.0, 15.1, 11.5, 9.0, 6.1, 4.4, 3.1, 2.13, 1.36, 0.82, 0.42, 0.30, 0.22, 0.15, 0.10,
    0.035, 0.0,
];
/// Steepness of the upper slopes (sone/Bark) per [`RNS`] range and critical band group.
const USL: [[f64; 8]; 18] = [
    [13.0, 8.2, 6.3, 5.5, 5.5, 5.5, 5.5, 5.5],
    [9.0, 7.5, 6.0, 5.1, 4.5, 4.5, 4.5, 4.5],
    [7.8, 6.7, 5.6, 4.9, 4.4, 3.9, 3.9, 3.9],
    [6.2, 5.4, 4.6, 4.0, 3.5, 3.2, 3.2, 3.2],
    [4.5, 3.8, 3.6, 3.2, 2.9, 2.7, 2.7, 2.7],
    [3.7, 3.0, 2.8, 2.35, 2.2, 2.2, 2.2, 2.2],
    [2.9, 2.3, 2.1, 1.9, 1.8, 1.7, 1.7, 1.7],
    [2.4, 1.7, 1.5, 1.35, 1.3, 1.3, 1.3, 1.3],
    [1.95, 1.45, 1.3, 1.15, 1.1, 1.1, 1.1, 1.1],
    [1.5, 1.2, 0.94, 0.86, 0.82, 0.82, 0.82, 0.82],
    [0.72, 0.67, 0.64, 0.63, 0.62, 0.62, 0.62, 0.62],
    [0.59, 0.53, 0.51, 0.50, 0.42, 0.42, 0.42, 0.42],
    [0.40, 0.33, 0.26, 0.24, 0.24, 0.22, 0.22, 0.22],
    [0.27, 0.21, 0.20, 0.18, 0.17, 0.17, 0.17, 0.17],
    [0.16, 0.15, 0.14, 0.12, 0.11, 0.11, 0.11, 0.11],
    [0.12, 0.11, 0.10, 0.08, 0.08, 0.08, 0.08, 0.08],
    [0.09, 0.08, 0.07, 0.06, 0.06, 0.06, 0.06, 0.05],
    [0.06, 0.05, 0.03, 0.02, 0.02, 0.02, 0.02, 0.02],
];

/// The core loudness of the 20 critical bands (and a closing 0) of third-octave levels (dB).
fn core_loudness(levels: &[f64; THIRD_OCTAVES]) -> [f64; 21] {
    // The bands up to 250 Hz, corrected by the equal loudness contours of their level's range.
    let corrected: [f64; 11] = std::array::from_fn(|band| {
        let level = levels[band];
        let range = RAP
            .iter()
            .zip(&DLL)
            .position(|(rap, dll)| level <= rap - dll[band])
            .unwrap_or(RAP.len() - 1);
        10f64.powf((level + DLL[range][band]) / 10.0)
    });
    let lowest = [&corrected[0..6], &corrected[6..9], &corrected[9..11]].map(|bands| {
        let sum: f64 = bands.iter().sum();
        if sum > 0.0 {
            10.0 * sum.log10()
        } else {
            f64::NEG_INFINITY
        }
    });
    let mut core = [0.0; 21];
    for band in 0..20 {
        let mut level = if band < 3 {
            lowest[band]
        } else {
            levels[band + 8]
        } - A0[band];
        if level > LTQ[band] {
            level -= DCB[band];
            let s = 0.25;
            core[band] = (0.0635
                * 10f64.powf(0.025 * LTQ[band])
                * ((1.0 - s + s * 10f64.powf(0.1 * (level - LTQ[band]))).powf(0.25) - 1.0))
                .max(0.0);
        }
    }
    // The threshold varies within the lowest critical band.
    let correction = 0.4 + 0.32 * core[0].powf(0.2);
    if correction <= 1.0 {
        core[0] *= correction;
    }
    core
}

/// Zwicker loudness (sone, ISO 532-1:2017, frontal free field) of steady third-octave band levels
/// 25 Hz to 12.5 kHz (dB re 20 uPa; `-inf` silent).
pub fn zwicker_loudness_sone(levels: &[f64; THIRD_OCTAVES]) -> f64 {
    let levels = levels.map(|level| level.max(SILENT_DB));
    let core = core_loudness(&levels);
    let (mut total, mut z1, mut n1) = (0.0f64, 0.0f64, 0.0f64);
    for (band, &upper) in ZUP.iter().enumerate() {
        let upper = upper + 0.0001;
        // The first band never falls: the pattern starts from nothing.
        let group = band.saturating_sub(1).min(7);
        let core = core[band];
        while z1 < upper {
            if n1 <= core {
                // Up to the band's core loudness, flat to its upper limit.
                total += core * (upper - z1);
                (z1, n1) = (upper, core);
            } else {
                // Down the upper slope of the current range of specific loudness, to the range's
                // floor, the band's core loudness or the band's limit, whichever comes first.
                let range = RNS
                    .iter()
                    .position(|&floor| n1 > floor)
                    .unwrap_or(RNS.len() - 1);
                let steepness = USL[range][group];
                let mut n2 = RNS[range].max(core);
                let mut z2 = z1 + (n1 - n2) / steepness;
                if z2 > upper {
                    z2 = upper;
                    n2 = n1 - (z2 - z1) * steepness;
                }
                total += (z2 - z1) * (n1 + n2) / 2.0;
                (z1, n1) = (z2, n2);
            }
        }
    }
    total.max(0.0)
}

/// Third-octave levels of octave band levels 63 Hz to 8 kHz (dB): each octave's energy shared
/// evenly by its three thirds; 25-40 Hz and 12.5 kHz silent.
pub fn third_octaves_of_octaves(octaves: &[f64; BANDS]) -> [f64; THIRD_OCTAVES] {
    let mut thirds = [f64::NEG_INFINITY; THIRD_OCTAVES];
    for (octave, &level) in octaves.iter().enumerate() {
        for third in 0..3 {
            thirds[OCTAVE_START - 1 + 3 * octave + third] = level - 10.0 * 3f64.log10();
        }
    }
    thirds
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet() -> [f64; THIRD_OCTAVES] {
        [SILENT_DB; THIRD_OCTAVES]
    }

    /// MoSQITo 1.2's ISO 532-1 stationary loudness (free field) of the same levels: a 1 kHz
    /// third at 40 and 70 dB, a car flow's spectrum at three levels, a distant aircraft's and
    /// random spectra; its values are rounded to 3 decimals up to 16 sone, 2 above.
    #[test]
    fn loudness_matches_the_reference_implementation() {
        let mut one_k = quiet();
        one_k[16] = 40.0;
        assert!((zwicker_loudness_sone(&one_k) - 0.927).abs() < 0.002);
        one_k[16] = 70.0;
        assert!((zwicker_loudness_sone(&one_k) - 6.944).abs() < 0.005);
        let road = [98.35, 91.1, 89.4, 90.7, 95.6, 92.4, 84.6, 76.2];
        let shifted = |octaves: &[f64; BANDS], offset: f64| {
            let mut thirds = third_octaves_of_octaves(&octaves.map(|level| level + offset));
            thirds
                .iter_mut()
                .for_each(|level| *level = level.max(SILENT_DB));
            thirds
        };
        for (offset, sone) in [(-60.0, 2.48), (-40.0, 12.075), (-20.0, 43.34)] {
            let n = zwicker_loudness_sone(&shifted(&road, offset));
            assert!(
                (n - sone).abs() / sone < 0.003,
                "{offset}: {n} against {sone}"
            );
        }
        let aircraft = [75.0, 78.0, 76.0, 72.0, 66.0, 58.0, 45.0, 30.0];
        let n = zwicker_loudness_sone(&shifted(&aircraft, -5.0));
        assert!((n - 21.39).abs() / 21.39 < 0.003, "{n}");
        // Random levels of 10-85 dB in every band (MoSQITo's values, rounded to two decimals;
        // the levels here to two).
        let random: [([f64; THIRD_OCTAVES], f64); 4] = [
            (
                [
                    56.88, 77.29, 68.18, 26.89, 32.51, 75.52, 10.39, 71.59, 69.78, 45.10, 32.73,
                    30.88, 29.12, 43.38, 47.84, 51.51, 84.66, 69.45, 56.66, 84.17, 26.15, 22.02,
                    55.94, 13.30, 12.68, 48.62, 44.97, 78.79,
                ],
                53.64,
            ),
            (
                [
                    57.19, 48.56, 47.27, 28.56, 10.88, 24.43, 61.90, 25.05, 37.72, 10.28, 72.25,
                    21.58, 30.07, 76.02, 48.23, 73.54, 57.98, 65.63, 16.86, 50.59, 48.08, 75.35,
                    37.09, 54.86, 14.44, 39.07, 34.23, 21.26,
                ],
                42.0,
            ),
            (
                [
                    71.23, 38.46, 83.41, 54.25, 55.38, 57.85, 60.73, 21.31, 43.02, 27.97, 40.19,
                    17.25, 82.59, 26.13, 60.38, 32.53, 75.56, 59.67, 19.87, 73.38, 80.87, 77.79,
                    52.73, 20.91, 24.43, 79.59, 51.42, 23.54,
                ],
                67.0,
            ),
            (
                [
                    76.30, 58.12, 52.73, 38.22, 40.82, 27.96, 12.85, 75.72, 45.08, 51.07, 34.16,
                    66.35, 11.89, 37.91, 12.28, 19.22, 82.54, 59.33, 42.12, 49.28, 75.46, 35.82,
                    54.27, 61.28, 36.66, 48.93, 67.39, 78.19,
                ],
                46.27,
            ),
        ];
        for (levels, sone) in random {
            let n = zwicker_loudness_sone(&levels);
            assert!((n - sone).abs() / sone < 0.003, "{n} against {sone}");
        }
    }
}

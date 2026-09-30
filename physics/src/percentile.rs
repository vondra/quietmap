//! How a source's level varies in time (Kurze 1971, "Statistics of road traffic noise", J. Sound
//! Vib. 18): a line of emitters (vehicles on a road, trains on a track, flights along a path) placed
//! as a Poisson process at `rho` per metre and moving along it, heard at perpendicular distance `d`,
//! gives an intensity `sum 1 / (d^2 + x_i^2)` whose distribution, normalised by its mean, depends on
//! `lambda = rho d` alone: the mean number of emitters within one distance of the closest point.
//! A dense road (`lambda` of tens) hums; a car every half hour 4 m away (`lambda` of 1e-4) is
//! silence but for its pass-by. The quantiles are tabulated once (a seeded simulation below
//! `lambda` = 10, the normal limit above) and sampled by inverse transform.

/// Tabulated `lambda` from 1e-4 upwards, [`STEPS_PER_DECADE`] a decade up to [`SIMULATED_LAMBDA_MAX`].
pub const LAMBDA_MIN: f64 = 1e-4;
pub const STEPS_PER_DECADE: f64 = 8.0;
/// Above this the intensity is normal: mean 1, standard deviation `1 / sqrt(2 pi lambda)`.
pub const SIMULATED_LAMBDA_MAX: f64 = 10.0;
/// Quantiles per table row: the intensity at probabilities `(k + 0.5) / QUANTILES`.
pub const QUANTILES: usize = 100;
/// Table rows: `lambda` = LAMBDA_MIN * 10^(row / STEPS_PER_DECADE).
pub const ROWS: usize = 41;

/// The tabulated quantiles (`percentile_table.rs`, written by the ignored test
/// `writes_the_table`: 20,000 seeded samples a row over at least 200 distances of line each side,
/// each value the shortest decimal that reads back as its `f32`).
static TABLE: [[f32; QUANTILES]; ROWS] = include!("percentile_table.rs");

/// A small deterministic generator (SplitMix64): the same click draws the same numbers.
#[derive(Clone)]
pub struct Random(u64);

impl Random {
    pub fn new(seed: u64) -> Self {
        Random(seed)
    }

    /// Uniform in [0, 1).
    pub fn uniform(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// The standard normal quantile at probability `p` (Acklam's approximation, 1e-9 relative).
fn normal_quantile(p: f64) -> f64 {
    const A: [f64; 6] = [
        -3.969683028665376e1,
        2.209460984245205e2,
        -2.759285104469687e2,
        1.38357751867269e2,
        -3.066479806614716e1,
        2.506628277459239,
    ];
    const B: [f64; 5] = [
        -5.447609879822406e1,
        1.615858368580409e2,
        -1.556989798598866e2,
        6.680131188771972e1,
        -1.328068155288572e1,
    ];
    const C: [f64; 6] = [
        -7.784894002430293e-3,
        -3.223964580411365e-1,
        -2.400758277161838,
        -2.549671010366478,
        4.374664141464968,
        2.938163982698783,
    ];
    const D: [f64; 4] = [
        7.784695709041462e-3,
        3.224671290700398e-1,
        2.445134137142996,
        3.754408661907416,
    ];
    let tail = |q: f64| {
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    };
    if p < 0.02425 {
        tail((-2.0 * p.ln()).sqrt())
    } else if p > 1.0 - 0.02425 {
        -tail((-2.0 * (1.0 - p).ln()).sqrt())
    } else {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    }
}

/// The intensity, relative to its mean, that a line source of `lambda` stays below with
/// probability `p` (0 < p < 1); `lambda` infinite is a steady source (always 1).
pub fn relative_intensity(lambda: f64, p: f64) -> f64 {
    let p = p.clamp(0.5 / QUANTILES as f64, 1.0 - 0.5 / QUANTILES as f64);
    if !lambda.is_finite() {
        return 1.0;
    }
    if lambda >= SIMULATED_LAMBDA_MAX {
        let deviation = 1.0 / (2.0 * std::f64::consts::PI * lambda).sqrt();
        return (1.0 + deviation * normal_quantile(p)).max(0.0);
    }
    let position = ((lambda.max(LAMBDA_MIN) / LAMBDA_MIN).log10() * STEPS_PER_DECADE)
        .clamp(0.0, (ROWS - 1) as f64);
    let (row, fraction) = (position.floor() as usize, position.fract());
    let quantile = |values: &[f32; QUANTILES]| {
        let at = (p * QUANTILES as f64 - 0.5).clamp(0.0, (QUANTILES - 1) as f64);
        let (k, t) = (at.floor() as usize, at.fract());
        let (here, next) = (
            f64::from(values[k]),
            f64::from(values[(k + 1).min(QUANTILES - 1)]),
        );
        here + t * (next - here)
    };
    let low = quantile(&TABLE[row]).max(1e-30).ln();
    let high = quantile(&TABLE[(row + 1).min(ROWS - 1)]).max(1e-30).ln();
    // Interpolated in log intensity between the rows.
    (low + fraction * (high - low)).exp()
}

/// The level exceeded a fraction `exceeded` of the time by a source of mean level `leq_db`: L10 is
/// `exceeded` = 0.1.
pub fn exceeded_level_db(leq_db: f64, lambda: f64, exceeded: f64) -> f64 {
    leq_db + 10.0 * relative_intensity(lambda, 1.0 - exceeded).log10()
}

#[cfg(test)]
#[path = "percentile_tests.rs"]
mod tests;

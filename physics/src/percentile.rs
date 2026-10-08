//! How a source's level varies in time (Kurze 1971, "Statistics of road traffic noise", J. Sound
//! Vib. 18): a line of emitters (vehicles on a road, trains on a track, flights along a path) placed
//! as a Poisson process at `rho` per metre and moving along it, heard at perpendicular distance `d`,
//! gives an intensity `sum 1 / (d^2 + x_i^2)` whose distribution, normalised by its mean, depends on
//! `lambda = rho d` alone: the mean number of emitters within one distance of the closest point.
//! A dense road (`lambda` of tens) hums; a car every half hour 4 m away (`lambda` of 1e-4) is
//! silence but for its pass-by. The quantiles are tabulated once (a seeded simulation below
//! `lambda` = 10, the normal limit above) and sampled by inverse transform; above the table's last
//! quantile a sparse line's loudest moments are its nearest emitter passing, in closed form, so the
//! rare passes keep their energy (a line of `lambda` 1e-4 held a tenth of its mean without them).

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
/// `writes_the_table`: 20,000 seeded samples a row, up to a million for sparse lines, over at least
/// 200 distances of line each side, each value the shortest decimal that reads back as its `f32`).
static TABLE: [[f32; QUANTILES]; ROWS] = include!("percentile_table.rs");

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

/// The intensity, relative to its mean, of a sparse line's nearest emitter passing that is exceeded
/// with probability `1 - p`: an emitter within `s` distances of the closest point, which happens
/// with probability `1 - exp(-2 lambda s)`, gives `1 / (lambda pi (1 + s^2))` (the simulation's
/// own law; its L10 emitter is 0.105 / (2 lambda) away).
fn passing_intensity(lambda: f64, p: f64) -> f64 {
    let s = -p.ln() / (2.0 * lambda);
    1.0 / (lambda * std::f64::consts::PI * (1.0 + s * s))
}

/// The probability of quantile `k` of a table row.
fn probability(k: usize) -> f64 {
    (k as f64 + 0.5) / QUANTILES as f64
}

/// The intensity, relative to its mean, that a line source of `lambda` stays below with
/// probability `p` (0 < p < 1); `lambda` infinite is a steady source (always 1). Between quantiles
/// the log intensity is linear in the log of `1 - p` (a sparse line's upper quantiles fall as a
/// power of it: interpolated linearly, lambda 1e-4's 0.99 quantile read 3.6 times too high); above
/// the last one the table goes on as its nearest emitter's passing over the others held at the
/// last quantile (added, so that the noise of the simulated last quantile stays in its own 0.5 %
/// of the time: scaled with it, it moved lambda 7.5e-4's mean 10 %).
pub fn relative_intensity(lambda: f64, p: f64) -> f64 {
    if !lambda.is_finite() {
        return 1.0;
    }
    // No emitters (a count rounded to none): its rare passes lie beyond any draw.
    if lambda <= 0.0 {
        return 0.0;
    }
    let (bottom, top) = (probability(0), probability(QUANTILES - 1));
    if lambda >= SIMULATED_LAMBDA_MAX {
        let deviation = 1.0 / (2.0 * std::f64::consts::PI * lambda).sqrt();
        return (1.0 + deviation * normal_quantile(p.clamp(bottom, top))).max(0.0);
    }
    if p > top {
        return relative_intensity(lambda, top) + passing_intensity(lambda, p)
            - passing_intensity(lambda, top);
    }
    // Sparser than the table, a line's nearest emitters lie about 1 / lambda distances away and
    // its intensity, relative to its mean, is in proportion to lambda below the last quantile.
    if lambda < LAMBDA_MIN {
        return lambda / LAMBDA_MIN * relative_intensity(LAMBDA_MIN, p);
    }
    let p = p.max(bottom);
    let position = ((lambda / LAMBDA_MIN).log10() * STEPS_PER_DECADE).clamp(0.0, (ROWS - 1) as f64);
    let (row, fraction) = (position.floor() as usize, position.fract());
    let k = ((p * QUANTILES as f64 - 0.5).floor() as usize).min(QUANTILES - 2);
    let t = ((1.0 - p).ln() - (1.0 - probability(k)).ln())
        / ((1.0 - probability(k + 1)).ln() - (1.0 - probability(k)).ln());
    let log_quantile = |values: &[f32; QUANTILES]| {
        let (here, next) = (
            f64::from(values[k]).max(1e-30).ln(),
            f64::from(values[k + 1]).max(1e-30).ln(),
        );
        here + t * (next - here)
    };
    let low = log_quantile(&TABLE[row]);
    let high = log_quantile(&TABLE[(row + 1).min(ROWS - 1)]);
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

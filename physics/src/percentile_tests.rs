//! The line-source statistics: the table's own simulation, its limits and a few known values.

use super::*;

/// Samples per simulated row. The line is simulated for at least 200 distances each side and for
/// 5 / lambda (ten emitters on average), so that even the sparsest line's L10 emitter, about
/// 0.05 / lambda away, is drawn; the emitters beyond add their mean.
const SAMPLES: usize = 20_000;
fn half_length(lambda: f64) -> f64 {
    (5.0 / lambda).max(200.0)
}

fn poisson(random: &mut Random, mean: f64) -> usize {
    if mean > 50.0 {
        let normal = (-2.0 * (1.0 - random.uniform()).ln()).sqrt()
            * (2.0 * std::f64::consts::PI * random.uniform()).cos();
        return (mean + mean.sqrt() * normal).round().max(0.0) as usize;
    }
    let limit = (-mean).exp();
    let (mut count, mut product) = (0, random.uniform());
    while product > limit {
        count += 1;
        product *= random.uniform();
    }
    count
}

/// One row: the normalised intensity (mean 1) at each tabulated probability, ascending.
fn simulate_row(lambda: f64, random: &mut Random) -> Vec<f64> {
    let half_length = half_length(lambda);
    let mean_inside = lambda * 2.0 * half_length;
    let outside = lambda * 2.0 * (std::f64::consts::FRAC_PI_2 - half_length.atan());
    let mean = lambda * std::f64::consts::PI;
    let mut samples: Vec<f64> = (0..SAMPLES)
        .map(|_| {
            let count = poisson(random, mean_inside);
            let inside: f64 = (0..count)
                .map(|_| {
                    let x = (2.0 * random.uniform() - 1.0) * half_length;
                    1.0 / (1.0 + x * x)
                })
                .sum();
            (inside + outside) / mean
        })
        .collect();
    samples.sort_by(f64::total_cmp);
    (0..QUANTILES)
        .map(|k| samples[((k as f64 + 0.5) / QUANTILES as f64 * SAMPLES as f64) as usize])
        .collect()
}

/// Writes `percentile_table.rs` (run once with `cargo test -p physics --release -- --ignored
/// writes_the_table`).
#[test]
#[ignore]
fn writes_the_table() {
    let mut random = Random::new(0x5eed_1971);
    let mut text = String::from("[\n");
    for row in 0..ROWS {
        let lambda = LAMBDA_MIN * 10f64.powf(row as f64 / STEPS_PER_DECADE);
        let values = simulate_row(lambda, &mut random);
        text.push_str("    [");
        text.push_str(
            &values
                .iter()
                .map(|&v| format!("{:e}", v as f32))
                .collect::<Vec<_>>()
                .join(", "),
        );
        text.push_str("],\n");
    }
    text.push_str("]\n");
    std::fs::write(
        concat!(env!("CARGO_MANIFEST_DIR"), "/src/percentile_table.rs"),
        text,
    )
    .unwrap();
}

/// Sparse traffic is quiet most of the time and loud rarely; dense traffic hums at its mean; a
/// steady source is its mean; the levels exceeded more often are lower. For a sparse line the
/// intensity exceeded 10 % of the time is an emitter's about 0.105 / (2 lambda) distances away:
/// 1 / (1 + x^2) over the mean lambda pi.
#[test]
fn sparse_lines_are_quiet_most_of_the_time_and_dense_ones_hum() {
    let l90 = |lambda: f64| exceeded_level_db(50.0, lambda, 0.9);
    let l10 = |lambda: f64| exceeded_level_db(50.0, lambda, 0.1);
    for lambda in [1e-4, 1e-3] {
        let x = 0.105 / (2.0 * lambda);
        let expected =
            50.0 + 10.0 * (1.0 / (1.0 + x * x) / (lambda * std::f64::consts::PI)).log10();
        assert!(
            (l10(lambda) - expected).abs() < 1.0,
            "{lambda}: {} vs {expected}",
            l10(lambda)
        );
    }
    assert!(l90(1e-3) < l10(1e-3));
    assert!((l10(100.0) - 50.0).abs() < 1.0 && (l90(100.0) - 50.0).abs() < 1.0);
    assert_eq!(exceeded_level_db(50.0, f64::INFINITY, 0.1), 50.0);
    for lambda in [1e-4, 1e-2, 0.3, 3.0, 9.9, 10.0, 30.0] {
        assert!(l90(lambda) <= l10(lambda), "{lambda}");
    }
}

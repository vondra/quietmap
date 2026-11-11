//! Weighted least squares by normal equations, every country weighed alike, for the road-model fits.

use std::collections::BTreeMap;

/// A country enters a fit, weighing as much as any other, with at least this many counted rows
/// (the 2026 fit's threshold for a country held out). A city's few counted streets are its busy
/// ones: on the 2026 counts, 43 to 449 rows of Luxembourg's, Czechia's and Ireland's city sets
/// read 6 to 8 dB off the unclassified law and turned its slope negative.
pub(crate) const MINIMUM_COUNTRY_ROWS: usize = 500;

/// The normal equations `X'WX b = X'Wy` of a weighted least-squares fit, accumulated row by row.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NormalEquations {
    size: usize,
    xtx: Vec<f64>,
    xty: Vec<f64>,
}

impl NormalEquations {
    pub(crate) fn new(size: usize) -> Self {
        Self {
            size,
            xtx: vec![0.0; size * size],
            xty: vec![0.0; size],
        }
    }

    pub(crate) fn add(&mut self, x: &[f64], y: f64, weight: f64) {
        debug_assert_eq!(x.len(), self.size);
        for (i, &xi) in x.iter().enumerate().filter(|(_, xi)| **xi != 0.0) {
            self.xty[i] += weight * xi * y;
            for (j, &xj) in x.iter().enumerate() {
                self.xtx[i * self.size + j] += weight * xi * xj;
            }
        }
    }

    /// The equations without the rows of `part` (a country held out).
    pub(crate) fn without(&self, part: &Self) -> Self {
        Self {
            size: self.size,
            xtx: self.xtx.iter().zip(&part.xtx).map(|(a, b)| a - b).collect(),
            xty: self.xty.iter().zip(&part.xty).map(|(a, b)| a - b).collect(),
        }
    }

    /// The weight behind column `column`: its weighted sum of squares.
    pub(crate) fn support(&self, column: usize) -> f64 {
        self.xtx[column * (self.size + 1)]
    }

    /// The weighted mean of `y` over the rows of an indicator column, `None` without such rows.
    pub(crate) fn indicator_mean(&self, column: usize) -> Option<f64> {
        let weight = self.support(column);
        (weight > SUPPORT_TOLERANCE).then(|| self.xty[column] / weight)
    }

    /// The coefficients; a column no row informs takes zero. `None` when the equations are
    /// singular in the informed columns.
    pub(crate) fn solve(&self) -> Option<Vec<f64>> {
        let n = self.size;
        let informed: Vec<usize> = (0..n)
            .filter(|&i| self.support(i) > SUPPORT_TOLERANCE)
            .collect();
        let m = informed.len();
        let mut a: Vec<f64> = informed
            .iter()
            .flat_map(|&i| informed.iter().map(move |&j| (i, j)))
            .map(|(i, j)| self.xtx[i * n + j])
            .collect();
        let mut b: Vec<f64> = informed.iter().map(|&i| self.xty[i]).collect();
        // Gaussian elimination with partial pivoting; a pivot vanishing against its column's
        // scale is a singular design.
        for k in 0..m {
            let pivot =
                (k..m).max_by(|&p, &q| a[p * m + k].abs().total_cmp(&a[q * m + k].abs()))?;
            let scale = self.support(informed[k]).max(SUPPORT_TOLERANCE);
            if a[pivot * m + k].abs() <= 1e-10 * scale {
                return None;
            }
            if pivot != k {
                for j in 0..m {
                    a.swap(k * m + j, pivot * m + j);
                }
                b.swap(k, pivot);
            }
            for r in k + 1..m {
                let factor = a[r * m + k] / a[k * m + k];
                for j in k..m {
                    a[r * m + j] -= factor * a[k * m + j];
                }
                b[r] -= factor * b[k];
            }
        }
        let mut solution = vec![0.0; m];
        for k in (0..m).rev() {
            let known: f64 = (k + 1..m).map(|j| a[k * m + j] * solution[j]).sum();
            solution[k] = (b[k] - known) / a[k * m + k];
        }
        let mut coefficients = vec![0.0; n];
        for (k, &i) in informed.iter().enumerate() {
            coefficients[i] = solution[k];
        }
        coefficients
            .iter()
            .all(|c| c.is_finite())
            .then_some(coefficients)
    }
}

/// A column with less weight than this is uninformed (weights of a country sum to one).
const SUPPORT_TOLERANCE: f64 = 1e-12;

/// Each row's weight: its length over its country's length, every country with at least
/// [`MINIMUM_COUNTRY_ROWS`] rows weighing one, others none.
pub(crate) fn country_weights(rows: &[([u8; 2], f64)]) -> Vec<f64> {
    let mut countries: BTreeMap<[u8; 2], (usize, f64)> = BTreeMap::new();
    for &(country, length) in rows {
        let entry = countries.entry(country).or_default();
        entry.0 += 1;
        entry.1 += length;
    }
    rows.iter()
        .map(|(country, length)| match countries[country] {
            (count, total) if count >= MINIMUM_COUNTRY_ROWS && total > 0.0 => length / total,
            _ => 0.0,
        })
        .collect()
}

/// The value below which `quantile` of the weight lies (the lower one at a tie).
pub(crate) fn weighted_quantile(values: &[(f64, f64)], quantile: f64) -> Option<f64> {
    let mut sorted: Vec<(f64, f64)> = values.iter().copied().filter(|v| v.1 > 0.0).collect();
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
    let target = quantile * sorted.iter().map(|v| v.1).sum::<f64>();
    let mut cumulative = 0.0;
    for (value, weight) in sorted {
        cumulative += weight;
        if cumulative >= target {
            return Some(value);
        }
    }
    None
}

/// A natural-log error in decibels.
pub(crate) fn log_error_db(error: f64) -> f64 {
    error.abs() * 10.0 / std::f64::consts::LN_10
}

/// A coefficient as the table stores it: four decimals, far below the fit's own uncertainty.
pub(crate) fn rounded(value: f64) -> f64 {
    (value * 1e4).round() / 1e4
}

#[cfg(test)]
mod tests {
    use super::*;

    /// y = 1 + 2 x exactly on three rows, a fourth row's weight pulls nothing off the line; an
    /// unused column takes zero, a duplicated one is singular.
    #[test]
    fn weighted_least_squares_recovers_a_line_and_refuses_a_singular_design() {
        let mut equations = NormalEquations::new(3);
        for (x, weight) in [(0.0, 1.0), (1.0, 2.0), (3.0, 0.5), (4.0, 7.0)] {
            equations.add(&[1.0, x, 0.0], 1.0 + 2.0 * x, weight);
        }
        let solution = equations.solve().unwrap();
        assert!((solution[0] - 1.0).abs() < 1e-12 && (solution[1] - 2.0).abs() < 1e-12);
        assert_eq!(solution[2], 0.0);
        let mut duplicate = NormalEquations::new(2);
        for x in [1.0, 2.0, 3.0] {
            duplicate.add(&[x, x], x, 1.0);
        }
        assert!(duplicate.solve().is_none());
    }

    /// Weighted fit by hand: rows (x, y, w) = (0, 0, 1), (1, 3, 1), (2, 2, 2). Normal equations
    /// [4 5; 5 9] b = [7 11]: b = (8/11, 9/11).
    #[test]
    fn a_weighted_fit_matches_its_hand_solved_normal_equations() {
        let mut equations = NormalEquations::new(2);
        for (x, y, w) in [(0.0, 0.0, 1.0), (1.0, 3.0, 1.0), (2.0, 2.0, 2.0)] {
            equations.add(&[1.0, x], y, w);
        }
        let b = equations.solve().unwrap();
        assert!((b[0] - 8.0 / 11.0).abs() < 1e-12, "{b:?}");
        assert!((b[1] - 9.0 / 11.0).abs() < 1e-12, "{b:?}");
        let mut part = NormalEquations::new(2);
        part.add(&[1.0, 2.0], 2.0, 2.0);
        let rest = equations.without(&part).solve().unwrap();
        assert!(
            (rest[0]).abs() < 1e-12 && (rest[1] - 3.0).abs() < 1e-12,
            "{rest:?}"
        );
        assert_eq!(equations.indicator_mean(0), Some(7.0 / 4.0));
    }

    /// A country of 500 rows weighs as much as one of 5,000; one of 499 rows weighs nothing;
    /// within a country a row weighs by its length.
    #[test]
    fn every_country_weighs_alike_and_a_thin_one_not_at_all() {
        let mut rows = vec![(*b"CZ", 2.0); 500];
        rows.extend(vec![(*b"US", 1.0); 5_000]);
        rows.extend(vec![(*b"LU", 1.0); 499]);
        rows[0].1 = 32.0;
        let weights = country_weights(&rows);
        let sum = |range: std::ops::Range<usize>| weights[range].iter().sum::<f64>();
        assert!((sum(0..500) - 1.0).abs() < 1e-12 && (sum(500..5_500) - 1.0).abs() < 1e-12);
        assert_eq!(sum(5_500..5_999), 0.0);
        assert!((weights[0] / weights[1] - 16.0).abs() < 1e-12);
    }

    #[test]
    fn the_weighted_quantile_is_the_value_reaching_its_share_of_the_weight() {
        let values = [(10.0, 1.0), (30.0, 1.0), (20.0, 8.0)];
        assert_eq!(weighted_quantile(&values, 0.9), Some(20.0));
        assert_eq!(weighted_quantile(&values, 0.95), Some(30.0));
        assert_eq!(weighted_quantile(&values, 0.05), Some(10.0));
        assert_eq!(weighted_quantile(&[], 0.5), None);
    }
}

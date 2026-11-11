//! The bus law fitted on the road rows a timetable runs: buses a day = K x (bus + W x trolleybus
//! line directions)^B x (min(trip ends within 2 km, knee) / 100,000)^C, by least squares on ln
//! buses with every place weighing alike and its rows by length; the floor of the trip ends' share,
//! that share where buildings are unmapped, the buses' period shares and a coach line's trips.

use crate::structures::height_calibration::median;
use anyhow::{Result, ensure};

/// The trip ends at which one line direction runs `one_line_direction` buses: a scale, not a fit
/// (any value gives the same law).
pub(super) const CITY_TRIP_ENDS: f64 = 100_000.0;
/// The trolleybus weights scanned: one to six in quarters (the 2026 fit's scan).
const TROLLEYBUS_WEIGHTS_IN_QUARTERS: std::ops::RangeInclusive<u32> = 4..=24;
/// The bounds on the trip ends scanned: the multiples of this up to the first at or above the
/// most trip ends of a fitted row (that one leaves every row unbounded).
const KNEE_STEP: f64 = 100_000.0;

/// A road row a timetable runs and a mapped bus or trolleybus line without a tagged interval
/// serves.
#[derive(Clone, Copy)]
pub(super) struct FitRow {
    pub place: usize,
    /// The timetable's buses a day.
    pub buses: f64,
    /// Bus and trolleybus line directions on its way.
    pub lines: [f64; 2],
    /// Trip ends within 2 km, at least one.
    pub trip_ends: f64,
    pub length_m: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Law {
    pub one_line_direction: f64,
    pub line_directions_exponent: f64,
    pub service_exponent: f64,
    pub trolleybus_line_weight: f64,
    pub most_trip_ends: f64,
    /// The weighted sum of squared ln residuals, the scan's criterion.
    pub squared_error: f64,
}

impl Law {
    /// The buses a day of a row's lines where the trip ends' share is one.
    pub fn line_part(&self, [bus, trolleybus]: [f64; 2]) -> f64 {
        self.one_line_direction
            * (bus + self.trolleybus_line_weight * trolleybus).powf(self.line_directions_exponent)
    }

    /// The trip ends' share without its floor.
    pub fn share(&self, trip_ends: f64) -> f64 {
        (trip_ends.min(self.most_trip_ends) / CITY_TRIP_ENDS).powf(self.service_exponent)
    }
}

/// Each row's weight: its length over its place's, so that every place weighs one.
pub(super) fn place_weights(rows: &[FitRow]) -> Vec<f64> {
    let places = rows.iter().map(|row| row.place + 1).max().unwrap_or(0);
    let mut lengths = vec![0.0; places];
    for row in rows {
        lengths[row.place] += row.length_m;
    }
    rows.iter()
        .map(|row| row.length_m / lengths[row.place])
        .collect()
}

/// The coefficients of y on [1, x1, x2] by weighted least squares (`(weight, x1, x2, y)`).
fn least_squares(points: &[(f64, f64, f64, f64)]) -> Option<[f64; 3]> {
    let mut matrix = [[0.0; 4]; 3];
    for &(weight, x1, x2, y) in points {
        let x = [1.0, x1, x2];
        for i in 0..3 {
            for j in 0..3 {
                matrix[i][j] += weight * x[i] * x[j];
            }
            matrix[i][3] += weight * x[i] * y;
        }
    }
    // Gaussian elimination with partial pivoting.
    for column in 0..3 {
        let pivot = (column..3)
            .max_by(|&a, &b| matrix[a][column].abs().total_cmp(&matrix[b][column].abs()))?;
        matrix.swap(column, pivot);
        if matrix[column][column].abs() < 1e-12 {
            return None;
        }
        let pivot_row = matrix[column];
        for row in matrix.iter_mut().skip(column + 1) {
            let factor = row[column] / pivot_row[column];
            for (value, pivot) in row.iter_mut().zip(pivot_row).skip(column) {
                *value -= factor * pivot;
            }
        }
    }
    let mut beta = [0.0; 3];
    for row in (0..3).rev() {
        let known: f64 = (row + 1..3).map(|k| matrix[row][k] * beta[k]).sum();
        beta[row] = (matrix[row][3] - known) / matrix[row][row];
    }
    Some(beta)
}

/// The law of every scanned trolleybus weight and bound on the trip ends with the least weighted
/// squared error (ties to the smaller bound, then the smaller weight), and the best of each bound.
pub(super) fn fit_law(rows: &[FitRow], weights: &[f64]) -> Result<(Law, Vec<Law>)> {
    ensure!(rows.len() >= 3, "bus law: {} fitted rows", rows.len());
    let most = rows.iter().map(|row| row.trip_ends).fold(1.0, f64::max);
    let knees = (1..)
        .map(|step| f64::from(step) * KNEE_STEP)
        .take_while(|&knee| knee < most + KNEE_STEP);
    let ln_buses: Vec<f64> = rows.iter().map(|row| row.buses.ln()).collect();
    let mut best_of_each_knee = Vec::new();
    for knee in knees {
        let ln_service: Vec<f64> = rows
            .iter()
            .map(|row| (row.trip_ends.min(knee) / CITY_TRIP_ENDS).ln())
            .collect();
        let mut best: Option<Law> = None;
        for quarters in TROLLEYBUS_WEIGHTS_IN_QUARTERS {
            let weight = f64::from(quarters) / 4.0;
            let points: Vec<_> = rows
                .iter()
                .enumerate()
                .map(|(i, row)| {
                    let lines = (row.lines[0] + weight * row.lines[1]).ln();
                    (weights[i], lines, ln_service[i], ln_buses[i])
                })
                .collect();
            let Some([intercept, slope, service]) = least_squares(&points) else {
                continue;
            };
            let squared_error = points
                .iter()
                .map(|&(w, x1, x2, y)| w * (y - intercept - slope * x1 - service * x2).powi(2))
                .sum();
            let law = Law {
                one_line_direction: intercept.exp(),
                line_directions_exponent: slope,
                service_exponent: service,
                trolleybus_line_weight: weight,
                most_trip_ends: knee,
                squared_error,
            };
            if best.is_none_or(|best| squared_error < best.squared_error) {
                best = Some(law);
            }
        }
        best_of_each_knee.extend(best);
    }
    let law = best_of_each_knee
        .iter()
        .copied()
        .reduce(|best, law| {
            if law.squared_error < best.squared_error {
                law
            } else {
                best
            }
        })
        .ok_or_else(|| anyhow::anyhow!("bus law: the rows determine no law"))?;
    Ok((law, best_of_each_knee))
}

/// The floor of the trip ends' share, where the timetables stop falling with the trip ends: the
/// weighted geometric mean of the timetable's buses over the law's line part on the largest set of
/// lowest-share rows whose mean is at least the highest share among them (on the 2026 fit's 35
/// places 0.1249, the 0.125 read off its binned residuals). A fixed point of "the rows below the
/// floor run the floor", without the cycles of iterating it. Returns the floor and its rows; where
/// the law holds down to the lowest row, the floor is that row's share and binds no row.
pub(super) fn least_service(rows: &[FitRow], weights: &[f64], law: &Law) -> (f64, usize) {
    let mut ranked: Vec<(f64, f64, f64)> = rows
        .iter()
        .zip(weights)
        .map(|(row, &weight)| {
            let ratio = row.buses / law.line_part(row.lines);
            (law.share(row.trip_ends), ratio.ln(), weight)
        })
        .collect();
    ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut floor = (ranked.first().map_or(1.0, |row| row.0), 0);
    let (mut sum, mut total) = (0.0, 0.0);
    for (taken, &(share, ln_ratio, weight)) in ranked.iter().enumerate() {
        sum += weight * ln_ratio;
        total += weight;
        let mean = (sum / total).exp();
        if mean >= share {
            floor = (mean, taken + 1);
        }
    }
    floor
}

/// The trip ends' share where the buildings are not mapped, by built-up flag (unknown, rural,
/// urban): the length-weighted geometric mean of the law's share on the bus rows of each flag
/// where they are mapped (`(flag, share, length)`); unknown takes the mean of rural and urban, as
/// every built-up rule does.
pub(super) fn service_by_built_up(rows: &[(u8, f64, f64)]) -> Result<[f64; 3]> {
    let mean = |flag: u8| -> Result<f64> {
        let (sum, length) = rows
            .iter()
            .filter(|(row_flag, _, _)| *row_flag == flag)
            .fold((0.0, 0.0), |(sum, total), &(_, share, length)| {
                (sum + length * share.ln(), total + length)
            });
        ensure!(
            length > 0.0,
            "bus law: no bus row with built-up flag {flag}"
        );
        Ok((sum / length).exp())
    };
    let (rural, urban) = (mean(1)?, mean(2)?);
    Ok([0.5 * (rural + urban), rural, urban])
}

/// The buses' day, evening and night shares: each period's median over the places with stop calls
/// of their shares, normalized to sum to one (the 2026 table's 0.731, 0.157, 0.112 of 43 places).
pub(super) fn period_shares(calls: &[[f64; 3]]) -> Result<[f64; 3]> {
    let shares: Vec<[f64; 3]> = calls
        .iter()
        .filter_map(|calls| {
            let total: f64 = calls.iter().sum();
            (total > 0.0).then(|| calls.map(|period| period / total))
        })
        .collect();
    ensure!(!shares.is_empty(), "bus law: no place has stop calls");
    let medians: [f64; 3] = std::array::from_fn(|period| {
        median(&mut shares.iter().map(|share| share[period]).collect::<Vec<_>>())
    });
    let total: f64 = medians.iter().sum();
    Ok(medians.map(|median| median / total))
}

/// A coach line direction's trips a day: the median over the coach route directions the
/// timetables run through the places.
pub(super) fn coach_departures(directions: &[f64]) -> Result<f64> {
    ensure!(
        !directions.is_empty(),
        "bus law: no timetable runs a coach route through a place"
    );
    Ok(median(&mut directions.to_vec()))
}

#[cfg(test)]
#[path = "bus_model_fit_tests.rs"]
mod tests;

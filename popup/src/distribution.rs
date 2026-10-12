//! How levels add over time: a distribution of the summed level on a 0.5 dB grid that keeps each
//! bin's mean intensity, and a line's intensities at fixed probabilities (Kurze's quantiles,
//! `physics::percentile`), so independent sounds are added exactly in energy and nearly in level.

use physics::percentile::relative_intensity;

/// The grid of the level distributions: levels at multiples of a step (the resolution's,
/// `crate::percentiles::Resolution`) from [`LEVEL_MIN_DB`] over [`SPAN_DB`], each bin holding the
/// levels rounding to it at their mean intensity; a level below the grid counts as silence.
const LEVEL_MIN_DB: f64 = -30.0;
const SPAN_DB: f64 = 180.0;
/// The probabilities at which a line's distribution is read, each with the share of the time it
/// stands for: every percent to 0.99 (the levels exceeded 5 to 90 % of the time within one), then
/// evenly in log(1 - p) to 1 - 1e-8, where a sparse line's rare passes lie (a car a day 10 m away
/// is loud for a few seconds of it).
fn nodes() -> &'static [(f64, f64)] {
    static NODES: std::sync::OnceLock<Vec<(f64, f64)>> = std::sync::OnceLock::new();
    NODES.get_or_init(|| {
        const EVEN: usize = 99;
        const TAIL_STEPS: usize = 24;
        let mut nodes: Vec<(f64, f64)> = (0..EVEN)
            .map(|k| ((k as f64 + 0.5) / 100.0, 0.01))
            .collect();
        // -log10(1 - p) from 2 to 8 in steps of a quarter, and the last 1e-8 in one.
        let at = |t: f64| 1.0 - 10f64.powf(-t);
        for k in 0..TAIL_STEPS {
            let (a, b) = (2.0 + k as f64 * 0.25, 2.25 + k as f64 * 0.25);
            nodes.push((at((a + b) / 2.0), at(b) - at(a)));
        }
        nodes.push((1.0 - 0.5e-8, 1e-8));
        nodes
    })
}

/// How the summed level is spread over a period: the share of the time in each bin of the level
/// grid with the mean intensity of its moments (so adding sounds loses no energy to the grid), and
/// the share in silence.
#[derive(Clone, Debug, PartialEq)]
pub struct Distribution {
    pub silent: f64,
    /// The grid's step (dB).
    bin_db: f64,
    /// Per bin its share of the time and that share times its moments' mean intensity.
    bins: Vec<[f64; 2]>,
}

impl Distribution {
    pub(crate) fn empty(bin_db: f64) -> Self {
        Distribution {
            silent: 0.0,
            bin_db,
            bins: vec![[0.0; 2]; (SPAN_DB / bin_db).round() as usize],
        }
    }

    /// The grid's bins.
    pub(crate) fn bin_count(&self) -> usize {
        self.bins.len()
    }

    /// The bin of a level; a level under the grid is held in its lowest bin, at its own intensity.
    fn bin(&self, level_db: f64) -> usize {
        nearest_index((level_db - LEVEL_MIN_DB) / self.bin_db).min(self.bins.len() - 1)
    }

    /// Each bin holding a share of the time: its moments' mean intensity and its share.
    pub(crate) fn intensities(&self) -> impl Iterator<Item = (f64, f64)> + '_ {
        self.bins
            .iter()
            .filter(|[share, _]| *share > 0.0)
            .map(|&[share, weighted]| (weighted / share, share))
    }

    /// Each bin holding a share of the time: its index, its moments' mean intensity and its share.
    pub(crate) fn bins(&self) -> impl Iterator<Item = (usize, f64, f64)> + '_ {
        self.bins
            .iter()
            .enumerate()
            .filter(|(_, [share, _])| *share > 0.0)
            .map(|(bin, &[share, weighted])| (bin, weighted / share, share))
    }

    /// The bin of a moment at `level_db` with a value at `value_db` added (`-inf` silent; the level
    /// from the two levels, a table); `None` for a silent sum.
    pub(crate) fn sum_bin(&self, level_db: f64, value_db: f64) -> Option<usize> {
        (level_db.is_finite() || value_db.is_finite())
            .then(|| self.bin(power_sum_db(level_db, value_db)))
    }

    /// Each level (dB) holding a share of the time, with its share; silence apart.
    pub fn levels(&self) -> impl Iterator<Item = (f64, f64)> + '_ {
        self.intensities()
            .map(|(intensity, share)| (10.0 * intensity.log10(), share))
    }

    /// The level exceeded `exceeded` of the time (dB, `-inf` where silence is).
    pub fn exceeded_db(&self, exceeded: f64) -> f64 {
        let mut above = 0.0;
        for &[share, weighted] in self.bins.iter().rev() {
            above += share;
            if share > 0.0 && above > exceeded * (1.0 - 1e-12) {
                return 10.0 * (weighted / share).log10();
            }
        }
        f64::NEG_INFINITY
    }

    /// A distribution of given levels (dB, `-inf` silent) with their shares of the time.
    #[cfg(test)]
    pub fn of_levels(levels: &[(f64, f64)]) -> Self {
        let mut silence = Distribution::empty(crate::percentiles::Resolution::CLICK.bin_db);
        silence.silent = 1.0;
        let values: Vec<(f64, f64)> = levels
            .iter()
            .map(|&(level, share)| (10f64.powf(level / 10.0), share))
            .collect();
        silence.with_line(&values)
    }

    /// The distribution with independent `values` (intensities with their shares of the time)
    /// added at every moment. A sum's bin is [`Self::sum_bin`]; its intensity is kept exact.
    pub(crate) fn with_line(&self, values: &[(f64, f64)]) -> Self {
        let mut sum = Distribution::empty(self.bin_db);
        let levels = value_levels(values);
        let mut add = |level: f64, intensity: f64, share: f64| {
            for (&(value, weight), &value_db) in values.iter().zip(&levels) {
                match self.sum_bin(level, value_db) {
                    Some(bin) => {
                        sum.bins[bin][0] += share * weight;
                        sum.bins[bin][1] += share * weight * (intensity + value);
                    }
                    None => sum.silent += share * weight,
                }
            }
        };
        add(f64::NEG_INFINITY, 0.0, self.silent);
        for (intensity, share) in self.intensities() {
            add(10.0 * intensity.log10(), intensity, share);
        }
        sum
    }

    /// The distribution with a steady `intensity` added at every moment.
    pub(crate) fn over(&self, intensity: f64) -> Self {
        self.with_line(&[(intensity, 1.0)])
    }

    /// Adds `other`'s shares times `weight`.
    pub(crate) fn add_scaled(&mut self, other: &Distribution, weight: f64) {
        self.silent += other.silent * weight;
        for (into, from) in self.bins.iter_mut().zip(&other.bins) {
            into[0] += from[0] * weight;
            into[1] += from[1] * weight;
        }
    }
}

/// The nearest whole number at or above zero of `value` (0 for anything under a half): `round` and
/// clamped, without the library call that took a fifth of a pixel's loudness (`Distribution::
/// with_line`); equal to it at every half below 6,002, the most any index here reaches.
fn nearest_index(value: f64) -> usize {
    if value < 0.5 {
        0
    } else {
        (value + 0.5) as usize
    }
}

/// The levels of a line's values (dB, `-inf` silent).
pub(crate) fn value_levels(values: &[(f64, f64)]) -> Vec<f64> {
    values
        .iter()
        .map(|&(value, _)| {
            if value > 0.0 {
                10.0 * value.log10()
            } else {
                f64::NEG_INFINITY
            }
        })
        .collect()
}

/// The level of two levels' summed energy (dB): the louder one and the step the quieter adds,
/// tabled every 0.01 dB of their difference (a silent one adds nothing).
fn power_sum_db(a: f64, b: f64) -> f64 {
    static STEPS: std::sync::OnceLock<Vec<f64>> = std::sync::OnceLock::new();
    const PER_DB: f64 = 100.0;
    const SPAN_DB: f64 = 60.0;
    let steps = STEPS.get_or_init(|| {
        (0..=(SPAN_DB * PER_DB) as usize)
            .map(|k| 10.0 * (1.0 + 10f64.powf(-(k as f64 / PER_DB) / 10.0)).log10())
            .collect()
    });
    let (high, difference) = if a >= b { (a, a - b) } else { (b, b - a) };
    if !difference.is_finite() {
        return high;
    }
    high + steps
        .get(nearest_index(difference * PER_DB))
        .copied()
        .unwrap_or(0.0)
}

/// A line's intensities at the nodes, each with its share of the time: Kurze's quantiles of
/// `mean` at `lambda`, or an event's mean over its duty while it sounds and silence otherwise.
pub(crate) fn line_values(mean: f64, lambda: f64, duty: Option<f64>) -> Vec<(f64, f64)> {
    match duty {
        Some(duty) => vec![(0.0, 1.0 - duty), (mean / duty, duty)],
        None => nodes()
            .iter()
            .map(|&(p, weight)| (mean * relative_intensity(lambda, p), weight))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The index is `round`'s, clamped at zero, at every half up to the largest table's and four
    /// representable numbers either side of it.
    #[test]
    fn the_nearest_index_rounds_as_the_library_does() {
        for k in 0..6_002u64 {
            let half = (k as f64 + 0.5).to_bits();
            for step in -4i64..=4 {
                let value = f64::from_bits((half as i64 + step) as u64);
                assert_eq!(nearest_index(value), value.round() as usize, "{value}");
            }
        }
        assert_eq!(nearest_index(-0.7), 0);
    }

    /// Adding many lines loses no energy to the grid: 500 identical lines, each soon a small part
    /// of a step of their sum, keep their summed mean (rounding to the grid lost 1.2 dB).
    #[test]
    fn many_quiet_lines_keep_their_energy() {
        let mut sum = Distribution::empty(crate::percentiles::Resolution::CLICK.bin_db);
        sum.silent = 1.0;
        let values = line_values(1.0, 0.5, None);
        let mean: f64 = values.iter().map(|(value, weight)| value * weight).sum();
        for _ in 0..500 {
            sum = sum.with_line(&values);
        }
        let total: f64 = sum
            .intensities()
            .map(|(intensity, share)| intensity * share)
            .sum();
        assert!((total / (500.0 * mean) - 1.0).abs() < 1e-9, "{total}");
    }
}

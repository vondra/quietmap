//! How levels add over time: a distribution of the summed level on a 0.1 dB grid that keeps each
//! bin's mean intensity, and a line's intensities at fixed probabilities (Kurze's quantiles,
//! `physics::percentile`), so independent sounds are added exactly in energy and nearly in level.

use physics::percentile::relative_intensity;

/// The grid of the level distributions: levels at multiples of [`BIN_DB`] from [`LEVEL_MIN_DB`]
/// (each bin holds the levels rounding to it); a level below the grid counts as silence.
pub const BIN_DB: f64 = 0.1;
const LEVEL_MIN_DB: f64 = -30.0;
const BINS: usize = 1_800;
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
    /// Per bin its share of the time and that share times its moments' mean intensity.
    bins: Vec<[f64; 2]>,
}

impl Distribution {
    pub(crate) fn empty() -> Self {
        Distribution {
            silent: 0.0,
            bins: vec![[0.0; 2]; BINS],
        }
    }

    /// The bin of a level; a level under the grid is held in its lowest bin, at its own intensity.
    fn bin(level_db: f64) -> usize {
        (((level_db - LEVEL_MIN_DB) / BIN_DB).round().max(0.0) as usize).min(BINS - 1)
    }

    /// Each bin holding a share of the time: its moments' mean intensity and its share.
    pub(crate) fn intensities(&self) -> impl Iterator<Item = (f64, f64)> + '_ {
        self.bins
            .iter()
            .filter(|[share, _]| *share > 0.0)
            .map(|&[share, weighted]| (weighted / share, share))
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
        let mut distribution = Distribution::empty();
        for &(level, share) in levels {
            distribution.add(10f64.powf(level / 10.0), share);
        }
        distribution
    }

    /// The distribution with independent `values` (intensities with their shares of the time)
    /// added at every moment. A sum's bin is found from the two levels (a table); its intensity
    /// is kept exact.
    pub(crate) fn with_line(&self, values: &[(f64, f64)]) -> Self {
        let mut sum = Distribution::empty();
        let levels: Vec<f64> = values
            .iter()
            .map(|&(value, _)| {
                if value > 0.0 {
                    10.0 * value.log10()
                } else {
                    f64::NEG_INFINITY
                }
            })
            .collect();
        for &(value, weight) in values {
            sum.add(value, self.silent * weight);
        }
        for (intensity, share) in self.intensities() {
            let level = 10.0 * intensity.log10();
            for (&(value, weight), &value_level) in values.iter().zip(&levels) {
                let bin = Self::bin(power_sum_db(level, value_level));
                sum.bins[bin][0] += share * weight;
                sum.bins[bin][1] += share * weight * (intensity + value);
            }
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

    fn add(&mut self, intensity: f64, share: f64) {
        if intensity > 0.0 {
            let bin = Self::bin(10.0 * intensity.log10());
            self.bins[bin][0] += share;
            self.bins[bin][1] += share * intensity;
        } else {
            self.silent += share;
        }
    }
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
        .get((difference * PER_DB).round() as usize)
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

    /// Adding many lines loses no energy to the grid: 500 identical lines, each soon a small part
    /// of a 0.1 dB step of their sum, keep their summed mean (rounding to the grid lost 1.2 dB).
    #[test]
    fn many_quiet_lines_keep_their_energy() {
        let mut sum = Distribution::empty();
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

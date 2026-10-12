//! How levels add over time: a distribution of the summed level on a grid of a half or a whole
//! decibel that keeps each bin's mean intensity, and a line's intensities at fixed probabilities (Kurze's quantiles,
//! `physics::percentile`), so independent sounds are added exactly in energy and nearly in level.

use physics::bands::energy;
use physics::percentile::AtProbabilities;

/// The grid of the level distributions: levels at multiples of a step (the resolution's,
/// `crate::percentiles::Resolution`) from [`LEVEL_MIN_DB`] over [`SPAN_DB`], each bin holding the
/// levels rounding to it at their mean intensity; a level below the grid is held in its lowest bin.
pub const LEVEL_MIN_DB: f64 = -30.0;
pub const SPAN_DB: f64 = 180.0;
/// The probabilities at which a line's distribution is read, each with the share of the time it
/// stands for: every percent to 0.99 (the levels exceeded 5 to 90 % of the time within one), then
/// evenly in log(1 - p) to 1 - 1e-8, where a sparse line's rare passes lie (a car a day 10 m away
/// is loud for a few seconds of it).
pub fn node_probabilities() -> Vec<(f64, f64)> {
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
}

/// The nodes ([`node_probabilities`]) with the intensities' parts read once at their probabilities.
fn nodes() -> &'static (AtProbabilities, Vec<f64>) {
    static NODES: std::sync::OnceLock<(AtProbabilities, Vec<f64>)> = std::sync::OnceLock::new();
    NODES.get_or_init(|| {
        let (probabilities, shares): (Vec<f64>, Vec<f64>) =
            node_probabilities().into_iter().unzip();
        (AtProbabilities::new(&probabilities), shares)
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
        (((level_db - LEVEL_MIN_DB) / self.bin_db).round() as usize).min(self.bins.len() - 1)
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

    /// The bins a moment at `intensity` (0 silent) moves to with each of `line`'s values times
    /// `scale` (above 0) added (`None`: silence), each visited once, in increasing order, with the
    /// shares of the values that take it there and their shares times their intensities: a
    /// logarithm a moment and a short search a bin, not a bin's arithmetic a value.
    pub(crate) fn runs(
        &self,
        intensity: f64,
        (line, scale): (&LineValues, f64),
        visit: &mut impl FnMut(Option<usize>, [f64; 2]),
    ) {
        let (pairs, sums) = (&line.pairs, &line.sums);
        let run = |from: usize, to: usize| {
            [
                sums[to][0] - sums[from][0],
                scale * (sums[to][1] - sums[from][1]),
            ]
        };
        let mut from = 0;
        if intensity <= 0.0 {
            from = pairs.partition_point(|&(value, _)| value <= 0.0);
            if from > 0 {
                visit(None, run(0, from));
            }
        }
        if from == pairs.len() {
            return;
        }
        // A sum's bin is that of its level: the first value's by its logarithm, then a run ends at
        // the first value whose sum reaches the next bin's lowest intensity, a step of the grid up
        // at a time (each run takes at least one value, each next one a higher bin).
        let last = self.bins.len() - 1;
        let step = energy(self.bin_db);
        let mut bin = self.bin(10.0 * (intensity + scale * pairs[from].0).log10());
        let mut ceiling = energy(LEVEL_MIN_DB + (bin as f64 + 0.5) * self.bin_db);
        loop {
            let to = if bin == last {
                pairs.len()
            } else {
                first_at_or_above(pairs, from + 1, (ceiling - intensity) / scale)
            };
            visit(Some(bin), run(from, to));
            from = to;
            let Some(&(value, _)) = pairs.get(from) else {
                break;
            };
            loop {
                bin += 1;
                ceiling *= step;
                if bin == last || intensity + scale * value < ceiling {
                    break;
                }
            }
        }
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
        silence.with_line((&LineValues::of(values), 1.0))
    }

    /// The distribution with an independent line (its values times a scale above 0) added at
    /// every moment: each sum in the bin of its level ([`Self::runs`]), its intensity kept exact.
    pub(crate) fn with_line(&self, line: (&LineValues, f64)) -> Self {
        let mut sum = Distribution::empty(self.bin_db);
        let mut add = |intensity: f64, share: f64| {
            self.runs(intensity, line, &mut |bin, [shares, weighted]| match bin {
                Some(bin) => {
                    sum.bins[bin][0] += share * shares;
                    sum.bins[bin][1] += share * (intensity * shares + weighted);
                }
                None => sum.silent += share * shares,
            });
        };
        if self.silent > 0.0 {
            add(0.0, self.silent);
        }
        for (intensity, share) in self.intensities() {
            add(intensity, share);
        }
        sum
    }

    /// The distribution with an independent `other` added at every moment: its levels as a line's
    /// values, its silence a value of nothing.
    pub(crate) fn with_distribution(&self, other: &Distribution) -> Self {
        let values = (other.intensities())
            .chain([(0.0, other.silent)])
            .filter(|&(_, share)| share > 0.0)
            .collect();
        self.with_line((&LineValues::of(values), 1.0))
    }

    /// The distribution with a steady `intensity` added at every moment.
    pub(crate) fn over(&self, intensity: f64) -> Self {
        self.with_line((&LineValues::of(vec![(intensity, 1.0)]), 1.0))
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

/// The first of `pairs` from `from` whose value reaches `limit` (`pairs.len()` if none): probing one,
/// two, four... ahead, then halving, as a run is mostly a few values.
fn first_at_or_above(pairs: &[(f64, f64)], from: usize, limit: f64) -> usize {
    let (mut low, mut step) = (from, 1);
    while low + step - 1 < pairs.len() && pairs[low + step - 1].0 < limit {
        low += step;
        step *= 2;
    }
    let high = (low + step - 1).min(pairs.len());
    low + pairs[low..high].partition_point(|&(value, _)| value < limit)
}

/// A line's intensities with their shares of the time, in increasing order, and the running sums
/// of the shares and of the shares times the intensities ([`Distribution::runs`]).
pub(crate) struct LineValues {
    pairs: Vec<(f64, f64)>,
    /// `sums[k]`: of the first `k` pairs, their shares and their shares times their intensities.
    sums: Vec<[f64; 2]>,
}

impl LineValues {
    /// Of intensities with their shares of the time, in any order.
    pub(crate) fn of(mut pairs: Vec<(f64, f64)>) -> Self {
        pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut sums = Vec::with_capacity(pairs.len() + 1);
        sums.push([0.0; 2]);
        for &(value, share) in &pairs {
            let [shares, weighted] = sums[sums.len() - 1];
            sums.push([shares + share, weighted + share * value]);
        }
        LineValues { pairs, sums }
    }

    /// Each intensity with its share of the time.
    pub(crate) fn pairs(&self) -> &[(f64, f64)] {
        &self.pairs
    }
}

/// A line's intensities relative to its mean, each with its share of the time: Kurze's quantiles
/// at `lambda` (at the nodes), or an event's mean over its duty while it sounds and silence
/// otherwise.
pub(crate) fn line_values(lambda: f64, duty: Option<f64>) -> LineValues {
    LineValues::of(match duty {
        Some(duty) => vec![(0.0, 1.0 - duty), (1.0 / duty, duty)],
        None => {
            let (at, shares) = nodes();
            (at.relative_intensities(lambda))
                .zip(shares.iter().copied())
                .collect()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Adding many lines loses no energy to the grid: 500 identical lines, each soon a small part
    /// of a step of their sum, keep their summed mean (rounding to the grid lost 1.2 dB).
    #[test]
    fn many_quiet_lines_keep_their_energy() {
        let mut sum = Distribution::empty(crate::percentiles::Resolution::CLICK.bin_db);
        sum.silent = 1.0;
        let values = line_values(0.5, None);
        let mean: f64 = values
            .pairs()
            .iter()
            .map(|(value, weight)| value * weight)
            .sum();
        for _ in 0..500 {
            sum = sum.with_line((&values, 1.0));
        }
        let total: f64 = sum
            .intensities()
            .map(|(intensity, share)| intensity * share)
            .sum();
        assert!((total / (500.0 * mean) - 1.0).abs() < 1e-9, "{total}");
    }

    /// The runs put every sum in the bin of its level, as adding the values one by one does: sparse,
    /// dense and steady lines, an event, a silent line, over silence and moments under, within and
    /// above the grid, at both grids.
    #[test]
    fn runs_put_each_sum_in_the_bin_of_its_level() {
        let lines = [
            (line_values(1e-3, None), 1e3),
            (line_values(0.5, None), 1e5),
            (line_values(30.0, None), 2e4),
            (line_values(0.0, Some(0.02)), 1e6),
            (LineValues::of(vec![(0.0, 0.3), (0.0, 0.7)]), 1.0),
            (LineValues::of(vec![(4e4, 1.0)]), 1.0),
        ];
        for bin_db in [0.5, 1.0] {
            let grid = Distribution::empty(bin_db);
            for (line, scale) in &lines {
                for intensity in [0.0, 1e-7, 3.3e3, 1e5, 2.2e7, 1e19] {
                    let mut ours = vec![None; grid.bin_count() + 1];
                    grid.runs(intensity, (line, *scale), &mut |bin, sums| {
                        let at = bin.map_or(grid.bin_count(), |bin| bin);
                        assert!(ours[at].is_none(), "a bin visited twice");
                        ours[at] = Some(sums);
                    });
                    let mut expected = vec![[0.0; 2]; grid.bin_count() + 1];
                    for &(relative, share) in line.pairs() {
                        let value = scale * relative;
                        let sum = intensity + value;
                        let at = if sum > 0.0 {
                            grid.bin(10.0 * sum.log10())
                        } else {
                            grid.bin_count()
                        };
                        expected[at][0] += share;
                        expected[at][1] += share * value;
                    }
                    for (at, expected) in expected.iter().enumerate() {
                        let [shares, weighted] = ours[at].unwrap_or([0.0; 2]);
                        let close = |a: f64, b: f64| (a - b).abs() <= 1e-12 * b.abs().max(1e-300);
                        assert!(
                            close(shares, expected[0]) && close(weighted, expected[1]),
                            "{bin_db} {intensity} bin {at}: {shares} {weighted} {expected:?}"
                        );
                    }
                }
            }
        }
    }
}

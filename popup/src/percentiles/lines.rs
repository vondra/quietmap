//! Source groups as fluctuating lines and their shapes over time.

use super::*;

/// One source whose level varies in time: its mean energy, its means in the weather states and
/// lambda per period (infinite for a steady source, whose level follows the weather alone).
#[derive(Clone)]
pub struct Line {
    /// The contributor's group: the lines are added in its order, so a click repeats exactly.
    pub(crate) key: u64,
    pub energy: [f64; PERIODS],
    /// Per period its mean in each of the [`WEATHER_STATES`] ([`Line::state_means`]).
    pub(super) means: [[f64; WEATHER_STATES]; PERIODS],
    pub(super) lambda: [f64; PERIODS],
    /// A road's hourly profile ([`hour_factor`]); `None` keeps the period's mean every hour.
    pub(super) profile: Option<usize>,
    /// An event source's share of each period it sounds: on, it is its mean over the duty; off,
    /// silent.
    pub(super) duty: Option<[f64; PERIODS]>,
    /// A stream of passes of one level (the flights of one class, `crate::aircraft::FlightPasses`):
    /// the passes in progress per period on average, their number at a moment Poisson, each at
    /// the line's mean over that average.
    pub(super) passing: Option<[f64; PERIODS]>,
}

/// A source group heard at a place, what its line is made of: its group, layer, energy per period
/// and weather, its nearest piece's emitters' distance from the receiver and where its display
/// fields are read (a click's contributor, or a painted pixel's exactly evaluated sources of one
/// group).
#[derive(Clone, Copy)]
pub struct Group<'a> {
    pub key: u64,
    pub layer: Layer,
    pub energy: [f64; PERIODS],
    pub weather: &'a Weather,
    pub slant_distance_m: f64,
    pub display: DisplayRef,
}

impl crate::update::Contributor {
    pub fn group(&self) -> Group<'_> {
        Group {
            key: self.group_key,
            layer: self.layer,
            energy: self.energy,
            weather: &self.weather,
            slant_distance_m: self.slant_distance_m,
            display: self.display,
        }
    }
}

/// What a line is made of, period by period: its energy, its mean in each
/// weather state and its lambda; its hourly profile, an event's duty and a stream of passes'
/// mean number in progress.
pub struct LineParts {
    pub energy: [f64; PERIODS],
    pub means: [[f64; WEATHER_STATES]; PERIODS],
    pub lambda: [f64; PERIODS],
    pub profile: Option<usize>,
    pub duty: Option<[f64; PERIODS]>,
    pub passing: Option<[f64; PERIODS]>,
}

impl Line {
    /// Its parts in the same weather states as the click (a port of the distributions reads them).
    pub fn parts(&self) -> LineParts {
        LineParts {
            energy: self.energy,
            means: self.means,
            lambda: self.lambda,
            profile: self.profile,
            duty: self.duty,
            passing: self.passing,
        }
    }

    /// A group's line by its `passes`: steady but for its events, or its passes; none where its
    /// traffic is unknown.
    pub fn of(group: &Group, passes: &Passes) -> Option<Self> {
        let Group {
            key,
            energy,
            weather,
            slant_distance_m,
            ..
        } = *group;
        let (lambda, profile, duty) = match *passes {
            Passes::Steady(duty) => ([f64::INFINITY; PERIODS], None, duty),
            _ => {
                let (per_hour, speed, profile) = passes.traffic(energy)?;
                (lambda(per_hour, speed, slant_distance_m), profile, None)
            }
        };
        Some(Line {
            key,
            energy,
            means: weather_means(weather, energy),
            lambda,
            profile,
            duty,
            passing: None,
        })
    }

    /// The flights heard, a line a class of their passes that sounds
    /// (`crate::aircraft::FlightPasses`), in the classes' order; flights sound alike in every
    /// weather state.
    pub fn flight_passes(passes: &crate::aircraft::FlightPasses) -> Vec<Self> {
        (0..crate::aircraft::PASS_CLASSES)
            .filter(|&class| (0..PERIODS).any(|p| passes.energy[p][class] > 0.0))
            .map(|class| {
                let energy: [f64; PERIODS] = std::array::from_fn(|p| passes.energy[p][class]);
                Line {
                    key: u64::MAX,
                    energy,
                    means: energy.map(|mean| [mean; WEATHER_STATES]),
                    lambda: [f64::INFINITY; PERIODS],
                    profile: None,
                    duty: None,
                    passing: Some(std::array::from_fn(|p| passes.passing[p][class])),
                }
            })
            .collect()
    }

    /// A line of passes at `lambda` with `weather`'s means: the distributions' tests'.
    #[cfg(test)]
    pub(crate) fn at_lambda(
        weather: &Weather,
        (energy, lambda): ([f64; PERIODS], [f64; PERIODS]),
    ) -> Self {
        Line {
            key: u64::MAX,
            energy,
            means: weather_means(weather, energy),
            lambda,
            profile: None,
            duty: None,
            passing: None,
        }
    }

    /// The line's mean in each weather state of `period`.
    pub(crate) fn state_means(&self, period: usize) -> [f64; WEATHER_STATES] {
        self.means[period]
    }

    /// The line's shape in hours of `period` with `factor` its flow then against the period's
    /// mean; `None` where it holds its mean (a dense line) or is silent (an event off then).
    pub(crate) fn shape(&self, period: usize, factor: f64) -> Option<Shape> {
        if let Some(passing) = self.passing {
            // A class moves the level whenever it sounds, as an event does: held at its mean
            // beside a louder class, a quieter one would hum on between both classes' passes.
            // (A share of the time under the least normal number holds its mean too: no f64
            // holds its intensity while passing.)
            let mean = passing[period];
            return (f64::MIN_POSITIVE..PASSES_HOLD_MEAN)
                .contains(&mean)
                .then(|| Shape {
                    values: pass_values(mean),
                    exceeded: None,
                });
        }
        let lambda = self.lambda[period] * factor;
        match self.duty.map(|duty| duty[period]) {
            Some(duty) if duty > 0.0 => Some(Shape {
                values: line_values(0.0, Some(duty)),
                exceeded: None,
            }),
            Some(_) => None,
            None if lambda >= HOLDS_MEAN_LAMBDA => None,
            None => Some(Shape {
                values: line_values(lambda, None),
                exceeded: Some(relative_intensity(lambda, 1.0 - HELD_EXCEEDED)),
            }),
        }
    }

    /// The line alone over a steady `floor` in `period`: each of its levels (dB) with its share of
    /// the period, over every hour and weather state; silence apart.
    pub fn own_levels(&self, period: usize, floor: f64, visit: &mut dyn FnMut(f64, f64)) {
        if self.energy[period] <= 0.0 {
            if floor > 0.0 {
                visit(10.0 * floor.log10(), 1.0);
            }
            return;
        }
        let factors: Vec<f64> = match self.profile {
            Some(profile) => (0..PERIOD_HOURS[period] as usize)
                .map(|slot| hour_factor(profile, period, slot))
                .collect(),
            None => vec![1.0],
        };
        let weight = 1.0 / (factors.len() * WEATHER_STATES) as f64;
        for &factor in &factors {
            let shape = self.shape(period, factor);
            let values: &[(f64, f64)] = match &shape {
                Some(shape) => shape.values.pairs(),
                None if self.duty.is_some() => &[],
                None => &[(1.0, 1.0)],
            };
            let silent = 1.0 - values.iter().map(|(_, share)| share).sum::<f64>();
            for mean in self.state_means(period) {
                let scaled = values
                    .iter()
                    .map(|&(relative, share)| (mean * factor * relative, share));
                for (value, share) in scaled.chain([(0.0, silent)]) {
                    if floor + value > 0.0 && share > 0.0 {
                        visit(10.0 * (floor + value).log10(), share * weight);
                    }
                }
            }
        }
    }
}

/// A line at least this dense (Kurze's lambda) holds its mean: its level stays within about 0.5 dB.
pub const HOLDS_MEAN_LAMBDA: f64 = 10.0;
/// A stream of passes with at least this many in progress on average holds its mean: their number
/// then varies as little as that line's intensity (1 / sqrt(2 pi lambda) of the mean).
pub const PASSES_HOLD_MEAN: f64 = 2.0 * std::f64::consts::PI * HOLDS_MEAN_LAMBDA;
/// A line held at its mean: one whose level exceeded a thousandth of the time stays a hundredth of
/// the steady energy's (20 dB under it), so it moves no level by more than a bin but for moments
/// too rare to count.
pub const HELD_EXCEEDED: f64 = 1e-3;
pub const HELD_SHARE: f64 = 1e-2;

/// Every line at its period's mean flow every hour.
pub(crate) fn at_mean_flow(lines: &mut [Line]) {
    for line in lines {
        line.profile = None;
    }
}

/// The lines of `groups` summing to `total` per period: every one with traffic in its fields (or
/// steady but for its events) over `share_min` of its period, in a fixed order; and what is left
/// of each period's energy, steady.
fn lines<'a>(
    groups: impl Iterator<Item = Group<'a>>,
    total: [f64; PERIODS],
    share_min: f64,
    passes: &dyn Fn(&Group) -> Result<Passes, String>,
) -> Result<(Vec<Line>, [f64; PERIODS]), String> {
    let mut lines = groups
        .filter(|group| (0..PERIODS).any(|p| group.energy[p] > share_min * total[p]))
        .filter_map(|group| (passes(&group).map(|passes| Line::of(&group, &passes))).transpose())
        .collect::<Result<Vec<Line>, String>>()?;
    // The contributors come from hash maps: a fixed order keeps a click's rounding the same.
    lines.sort_by_key(|line| line.key);
    let steady = std::array::from_fn(|p| {
        (total[p] - lines.iter().map(|line| line.energy[p]).sum::<f64>()).max(0.0)
    });
    Ok((lines, steady))
}

/// The lines of a click, every contributor of `selections` and the `flights` heard
/// ([`Line::flight_passes`]), and what is left of each period's energy, steady.
pub fn click_lines(
    selections: &[LayerSelection],
    flights: &[Line],
    passes: &dyn Fn(&Group) -> Result<Passes, String>,
) -> Result<(Vec<Line>, [f64; PERIODS]), String> {
    let total: [f64; PERIODS] =
        std::array::from_fn(|p| selections.iter().map(|s| s.answer_energy()[p]).sum::<f64>());
    let groups = (selections.iter())
        .flat_map(|selection| selection.contributors.values())
        .map(|contributor| contributor.group());
    place_lines(groups, total, flights, passes)
}

/// The lines of a place heard with `total` energy per period: its `groups` that move the level and
/// the `flights` heard (the lines of [`Line::flight_passes`]), and what is left of each period's
/// energy, steady.
pub fn place_lines<'a>(
    groups: impl Iterator<Item = Group<'a>>,
    total: [f64; PERIODS],
    flights: &[Line],
    passes: &dyn Fn(&Group) -> Result<Passes, String>,
) -> Result<(Vec<Line>, [f64; PERIODS]), String> {
    let (mut lines, mut steady) = lines(groups, total, FLUCTUATING_SHARE_MIN, passes)?;
    for flight_line in flights {
        for (steady, energy) in steady.iter_mut().zip(flight_line.energy) {
            *steady = (*steady - energy).max(0.0);
        }
    }
    lines.extend_from_slice(flights);
    Ok((lines, steady))
}

/// Contributors under this share of their period's energy count as steady (their fluctuation does
/// not move a percentile of the sum).
const FLUCTUATING_SHARE_MIN: f64 = 1e-4;

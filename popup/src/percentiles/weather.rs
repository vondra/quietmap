//! Favourable-share bins and their equivalent meteorological states.

use super::*;

/// Bins of the favourable share in which a contributor keeps its pieces' weather apart.
const SHARE_BINS: usize = 10;

/// A contributor's energy per period in each meteorological state (homogeneous, favourable) and
/// their mix, binned by its pieces' favourable share p (CNOSSOS-EU 2.5.9, by direction): the
/// pieces on either side of a click are favourable at different times (ERA5's sectors differ by
/// up to 50 points across), so one share for the whole would make them all favourable or all calm
/// together. Pieces louder in calm air (tree crowns or the ground taking more from the arc) keep
/// bins of their own: summed with the others the states' difference cancels, and the share no
/// longer gives back the mean. A draw's weather is favourable in the bins whose share lies above
/// it.
#[derive(Clone, Default)]
pub struct Weather(pub(super) [[[f64; PERIODS]; 3]; 2 * SHARE_BINS]);

impl Weather {
    /// Where a piece's `energy` of one period falls, the mix of its `states` (homogeneous,
    /// favourable): its bin, and its energy in either state and mixed; a piece without states
    /// (none computed) holds its mean in either.
    pub fn binned(energy: f64, states: [f64; 2]) -> (usize, [f64; 3]) {
        let [homogeneous, favourable] = if states[0] > 0.0 || states[1] > 0.0 {
            states
        } else {
            [energy; 2]
        };
        let share = favourable_share(energy, homogeneous, favourable);
        let calm_louder = usize::from(favourable < homogeneous);
        let bin =
            calm_louder * SHARE_BINS + ((share * SHARE_BINS as f64) as usize).min(SHARE_BINS - 1);
        (bin, [homogeneous, favourable, energy])
    }

    /// Adds energy (homogeneous, favourable, mixed) to `bin` of `period` ([`Weather::binned`]).
    pub fn add_binned(&mut self, period: usize, bin: usize, values: [f64; 3]) {
        for (state, value) in self.0[bin].iter_mut().zip(values) {
            state[period] += value;
        }
    }

    /// Adds a piece of `energy` per period, the mix of its `states`.
    pub fn add(&mut self, energy: &[f64; PERIODS], states: &[[f64; PERIODS]; 2]) {
        for period in 0..PERIODS {
            let (bin, values) =
                Self::binned(energy[period], [states[0][period], states[1][period]]);
            self.add_binned(period, bin, values);
        }
    }
}

/// The share of the time a source is heard in its favourable state, from its mean being the mix
/// of the two.
pub(crate) fn favourable_share(mean: f64, homogeneous: f64, favourable: f64) -> f64 {
    if (favourable - homogeneous).abs() <= f64::EPSILON * favourable.abs() {
        return 0.0;
    }
    ((mean - homogeneous) / (favourable - homogeneous)).clamp(0.0, 1.0)
}

/// Per period the mean of `energy` in `weather` in each weather state: a piece is favourable while
/// the place's weather u lies under its favourable share, so over the state's span of u it is
/// favourable for the part of the span under the share (no step where a share crosses a state's
/// middle). Each bin's calm energy counts in every state and its excess downwind in the states
/// wholly under its share and in part in the state its share falls in: one pass over the bins.
pub(super) fn weather_means(
    weather: &Weather,
    energy: [f64; PERIODS],
) -> [[f64; WEATHER_STATES]; PERIODS] {
    std::array::from_fn(|period| {
        // Per state the excess of the bins whose share falls in it, whole and its part in it.
        let (mut whole, mut part) = ([0.0; WEATHER_STATES + 1], [0.0; WEATHER_STATES + 1]);
        let (mut calm, mut silent) = (0.0, true);
        for [homogeneous, favourable, mix] in &weather.0 {
            let (homogeneous, favourable) = (homogeneous[period], favourable[period]);
            silent &= homogeneous + favourable <= 0.0;
            let share = favourable_share(mix[period], homogeneous, favourable);
            let position = share * WEATHER_STATES as f64;
            let state = (position as usize).min(WEATHER_STATES);
            whole[state] += favourable - homogeneous;
            part[state] += (position - state as f64) * (favourable - homogeneous);
            calm += homogeneous;
        }
        if silent {
            return [energy[period]; WEATHER_STATES];
        }
        let mut means = [0.0; WEATHER_STATES];
        let mut above = 0.0;
        for state in (0..WEATHER_STATES).rev() {
            above += whole[state + 1];
            means[state] = calm + above + part[state];
        }
        means
    })
}

/// Equally likely weather states of the place, one for every ray of it (a night's inversion bends
/// them alike).
pub const WEATHER_STATES: usize = 10;
/// The weather states merged where their difference cannot show: per period, each run of
/// consecutive states in which every line's mean stays finite and within `spread` (its largest
/// over its smallest) takes each line's mean over the run in all its states. The energy stays; the
/// distributions compute a state equal to the one before once (`layer_states`, the card's blocks
/// of distinct states), so a run costs one state. A moment's intensity moves by no more than
/// `spread` where the run's states hold and move the same lines; a line held steady in one state and
/// moving in another can change its rare loud moments by more.
pub fn merge_close_states(layers: &mut [(Vec<Line>, [f64; PERIODS])], spread: f64) {
    let within = |means: &[f64]| {
        let (mut low, mut high) = (f64::INFINITY, 0.0f64);
        for &mean in means {
            if !mean.is_finite() {
                return false;
            }
            (low, high) = (low.min(mean), high.max(mean));
        }
        high <= spread * low
    };
    for period in 0..PERIODS {
        let mut start = 0;
        while start < WEATHER_STATES {
            let mut end = start + 1;
            while end < WEATHER_STATES
                && (layers.iter().flat_map(|(lines, _)| lines))
                    .all(|line| within(&line.means[period][start..=end]))
            {
                end += 1;
            }
            if end - start > 1 {
                for line in layers.iter_mut().flat_map(|(lines, _)| lines) {
                    let run = &mut line.means[period][start..end];
                    if run.iter().any(|&mean| mean != run[0]) {
                        let mean = run.iter().sum::<f64>() / run.len() as f64;
                        run.fill(mean);
                    }
                }
            }
            start = end;
        }
    }
}

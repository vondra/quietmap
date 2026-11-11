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
    /// Adds a piece of `energy` per period, the mix of its `states`; a piece without states
    /// (none computed) holds its mean in either.
    pub fn add(&mut self, energy: &[f64; PERIODS], states: &[[f64; PERIODS]; 2]) {
        for period in 0..PERIODS {
            let [homogeneous, favourable] = if states[0][period] > 0.0 || states[1][period] > 0.0 {
                [states[0][period], states[1][period]]
            } else {
                [energy[period]; 2]
            };
            let share = favourable_share(energy[period], homogeneous, favourable);
            let calm_louder = usize::from(favourable < homogeneous);
            let bin = &mut self.0[calm_louder * SHARE_BINS
                + ((share * SHARE_BINS as f64) as usize).min(SHARE_BINS - 1)];
            bin[0][period] += homogeneous;
            bin[1][period] += favourable;
            bin[2][period] += energy[period];
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
/// The flights' weather: their mean in either state.
pub fn flight_weather(flights: &([f64; PERIODS], [f64; PERIODS])) -> Weather {
    let mut weather = Weather::default();
    weather.add(&flights.0, &[flights.0; 2]);
    weather
}

//! One number for how unpleasant a place's noise is to live with: the share of residents expected
//! to be highly annoyed (%HA).
//!
//! Per source the WHO 2018 exposure-response curves in Lden (Guski et al. 2017, the curves of
//! Directive 2020/367 Annex III) recast as logistic curves, which follow the published quadratics
//! within 1 to 2.5 points over 50-75 dB and keep falling below the surveys' 40 dB where the
//! quadratics turn (road traffic's rises again below 45.6 dB, aircraft's goes negative below 39
//! dB); industry and the other stationary sources take Miedema and Vos (2004) recast alike. Road
//! traffic heard as occasional pass-bys annoys less than a steady flow of the same Lden: SiRENE
//! (Brink et al. 2019) found its curve shifted by up to about 6 dB between low and high
//! intermittency ratio (Wunderli et al. 2016), and none for railways and aircraft; here the credit
//! grows linearly from 0 dB at a ratio of 1/3 to 6 dB at 2/3 (a provisional reading, SiRENE gives
//! no formula). The sources combine as annoyance equivalents (Miedema 2004): each one's
//! road-traffic level of the same %HA, summed in energy, read off the road curve.

use physics::bands::{PERIODS, energy, level_db};
use tiles::sources::Layer;

/// A logistic %HA curve: logit(%HA / 100) = a + b Lden.
#[derive(Debug, Clone, Copy)]
struct Curve {
    a: f64,
    b: f64,
}

const ROAD: Curve = Curve {
    a: -6.27,
    b: 0.0763,
};
const RAIL: Curve = Curve {
    a: -7.67,
    b: 0.1005,
};
const AIRCRAFT: Curve = Curve {
    a: -6.24,
    b: 0.0936,
};
/// Miedema and Vos (2004) industry, fitted over 45-65 dB.
const STATIONARY: Curve = Curve {
    a: -8.67,
    b: 0.1136,
};

/// SiRENE's shift between steady and intermittent road traffic (dB), and the intermittency ratios
/// over which it grows.
const ROAD_INTERMITTENCY_CREDIT_DB: f64 = 6.0;
const ROAD_INTERMITTENCY_RAMP: (f64, f64) = (1.0 / 3.0, 2.0 / 3.0);

impl Curve {
    fn percent(self, lden_db: f64) -> f64 {
        100.0 / (1.0 + (-(self.a + self.b * lden_db)).exp())
    }

    /// The Lden at which this curve reaches `percent`.
    fn level_db(self, percent: f64) -> f64 {
        ((percent / (100.0 - percent)).ln() - self.a) / self.b
    }
}

fn curve(layer: Layer) -> Curve {
    match layer {
        Layer::Road => ROAD,
        Layer::Railway => RAIL,
        Layer::Aircraft => AIRCRAFT,
        Layer::Industry | Layer::Building | Layer::Ship => STATIONARY,
    }
}

/// The expected share highly annoyed and how it came about.
#[derive(Debug, Clone, PartialEq)]
pub struct Annoyance {
    /// All sources together (%).
    pub percent: f64,
    /// Each sounding layer's own share (%), as if it were alone.
    pub by_layer: Vec<(Layer, f64)>,
    /// The layer of the highest road-equivalent level.
    pub dominant: Option<Layer>,
    /// Road traffic's intermittency ratio over day and evening (NaN without road traffic) and the
    /// credit it earned (dB).
    pub road_intermittency: f64,
    pub road_credit_db: f64,
}

/// The credit (dB) of road traffic with intermittency ratio `ratio`.
pub fn road_credit_db(ratio: f64) -> f64 {
    if !ratio.is_finite() {
        return 0.0;
    }
    let (low, high) = ROAD_INTERMITTENCY_RAMP;
    ROAD_INTERMITTENCY_CREDIT_DB * ((ratio - low) / (high - low)).clamp(0.0, 1.0)
}

/// The share highly annoyed of layers heard at `layer_lden` (dB, `-inf` silent), road traffic
/// with intermittency ratio `road_intermittency` over day and evening.
pub fn annoyance(layer_lden: &[(Layer, f64)], road_intermittency: f64) -> Annoyance {
    let credit = road_credit_db(road_intermittency);
    let mut by_layer = Vec::new();
    let (mut equivalent_energy, mut dominant) = (0.0, None::<(Layer, f64)>);
    for &(layer, lden) in layer_lden.iter().filter(|(_, lden)| lden.is_finite()) {
        let rated = if layer == Layer::Road {
            lden - credit
        } else {
            lden
        };
        let percent = curve(layer).percent(rated);
        by_layer.push((layer, percent));
        let equivalent = ROAD.level_db(percent);
        equivalent_energy += energy(equivalent);
        if dominant.is_none_or(|(_, level)| equivalent > level) {
            dominant = Some((layer, equivalent));
        }
    }
    let percent = if equivalent_energy > 0.0 {
        ROAD.percent(level_db(equivalent_energy))
    } else {
        0.0
    };
    Annoyance {
        percent,
        by_layer,
        dominant: dominant.map(|(layer, _)| layer),
        road_intermittency,
        road_credit_db: credit,
    }
}

/// The day-evening intermittency ratio of road traffic from its per-period ratios and energies.
pub fn day_evening_intermittency(ratio: [f64; PERIODS], road_energy: [f64; PERIODS]) -> f64 {
    let weights = [12.0 * road_energy[0], 4.0 * road_energy[1]];
    let total = weights[0] + weights[1];
    if total > 0.0 && ratio[0].is_finite() && ratio[1].is_finite() {
        (weights[0] * ratio[0] + weights[1] * ratio[1]) / total
    } else {
        f64::NAN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The logistic curves follow WHO's quadratics where the surveys have data, and keep falling
    /// below 40 dB where the quadratics turn.
    #[test]
    fn the_curves_follow_who_where_the_surveys_hold() {
        let quadratic = |c: [f64; 3], l: f64| c[0] + c[1] * l + c[2] * l * l;
        let road = [78.9270, -3.1162, 0.0342];
        let rail = [38.1596, -2.05538, 0.0285];
        let aircraft = [-50.9693, 1.0168, 0.0072];
        for level in [50.0, 55.0, 60.0, 65.0, 70.0] {
            assert!(
                (ROAD.percent(level) - quadratic(road, level)).abs() < 1.0,
                "{level}"
            );
            assert!(
                (RAIL.percent(level) - quadratic(rail, level)).abs() < 2.5,
                "{level}"
            );
            assert!(
                (AIRCRAFT.percent(level) - quadratic(aircraft, level)).abs() < 2.5,
                "{level}"
            );
        }
        assert!(ROAD.percent(35.0) < ROAD.percent(45.0) && ROAD.percent(35.0) > 0.0);
        assert!(AIRCRAFT.percent(35.0) > 0.0);
        assert!((ROAD.level_db(ROAD.percent(47.3)) - 47.3).abs() < 1e-9);
    }

    /// The owner's rural hotel: 43 dB of road traffic from a car every half hour reads about 3 %,
    /// the same Lden from a steady flow about 5 %; aircraft of the same Lden annoy more than both.
    #[test]
    fn occasional_cars_annoy_less_than_steady_traffic_and_aircraft_more() {
        let hotel = annoyance(&[(Layer::Road, 43.0)], 0.95);
        let steady = annoyance(&[(Layer::Road, 43.0)], 0.1);
        let aircraft = annoyance(&[(Layer::Aircraft, 43.0)], f64::NAN);
        assert!((hotel.road_credit_db - 6.0).abs() < 1e-12 && steady.road_credit_db == 0.0);
        assert!(hotel.percent < steady.percent && steady.percent < aircraft.percent);
        assert!((hotel.percent - 3.1).abs() < 0.3, "{}", hotel.percent);
        assert_eq!(aircraft.dominant, Some(Layer::Aircraft));
    }

    /// Two sources: the total exceeds either alone and follows the most annoying; silence is 0.
    #[test]
    fn sources_add_as_annoyance_equivalents() {
        let both = annoyance(&[(Layer::Road, 45.0), (Layer::Aircraft, 45.0)], 0.2);
        let road = annoyance(&[(Layer::Road, 45.0)], 0.2).percent;
        let aircraft = annoyance(&[(Layer::Aircraft, 45.0)], f64::NAN).percent;
        assert!(both.percent > road.max(aircraft));
        assert_eq!(both.dominant, Some(Layer::Aircraft));
        assert_eq!(
            annoyance(&[(Layer::Road, f64::NEG_INFINITY)], f64::NAN).percent,
            0.0
        );
        assert!((road_credit_db(0.5) - 3.0).abs() < 1e-9);
    }
}

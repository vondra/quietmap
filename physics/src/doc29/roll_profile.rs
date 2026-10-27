//! A Doc 29 ground roll's segments (4th ed. Vol 2 3.6.2, 3.6.3): a take-off from rest to lift-off,
//! a landing from touchdown to the runway exit, cut where the speed has changed by at most 10 m/s at
//! constant acceleration, each segment at its mean speed and thrust: a take-off at its rating, a
//! landing from its final approach power into reverse thrust with its provisional 5 dB (Fig. 3-6).

use super::npd::{class_anchor, is_helicopter_class};
use super::roll::{RollKind, roll_speed_correction_db};
use super::segment::{AircraftType, SegmentEmission};
use super::thrust::{PowerBracket, SegmentFlight, segment_thrust_lb};

/// A roll segment's speed change (m/s): Doc 29 3.6.2 takes no more than 10.
const SPEED_STEP_MS: f64 = 10.0;
/// A landing roll ends where the aircraft leaves the runway, at this speed (m/s; Fig. 3-6).
pub const LANDING_EXIT_SPEED_MS: f64 = 15.0;
/// Reverse thrust (3.6.3): from the final approach power at touchdown to this share of the maximum
/// power over the first tenth of the stop distance, then to the last share at the stop; the
/// maximum is the take-off rating at the touchdown speed (a propeller's Eq. B-5 has no value at
/// rest: at 1 kt it put a Cessna 172 at 33,855 lb).
const REVERSE_POWER_SHARE: f64 = 0.2;
const STOP_POWER_SHARE: f64 = 0.1;
const REVERSE_SHARE_OF_STOP: f64 = 0.1;
/// Reverse thrust's level increment at its peak (dB): 3.6.3 recommends it provisionally where no
/// operational information is available. The German test tasks, which give their own reverse
/// thrust (the A320's 64 % of its maximum, the CRJ900's none), add none.
pub const REVERSE_INCREMENT_DB: f64 = 5.0;
/// The final approach's glideslope (deg): the landing configuration's thrust at touchdown.
const GLIDESLOPE_DEG: f64 = 3.0;
const METRES_PER_SECOND_PER_KNOT: f64 = 1_852.0 / 3_600.0;

/// A roll as Doc 29 models it: a take-off from rest to lift-off at `speed_ms` over `length_m` (the
/// equivalent take-off distance), or a landing from touchdown at `speed_ms` to the runway exit's
/// 15 m/s over `length_m` (the stop distance), of noise class `class` at a field `field_altitude_m`
/// above sea level (the thrust's pressure altitude).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RollProfile {
    pub kind: RollKind,
    pub class: usize,
    pub length_m: f64,
    pub speed_ms: f64,
    pub field_altitude_m: f64,
}

/// One segment of a roll: from and to along it (m from the start of roll or from touchdown), what
/// it emits, and at its start and end its thrust per engine (lb; `None` for a class without a
/// thrust model) and reverse thrust's increment (dB), which a receiver reads at the segment's point
/// nearest it (Doc 29 4.4.2).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RollStep {
    pub from_m: f64,
    pub to_m: f64,
    pub emission: SegmentEmission,
    pub thrust_lb: Option<[f64; 2]>,
    pub increment_db: [f64; 2],
}

impl RollProfile {
    /// The speeds at the roll's start and end (m/s): a take-off's from rest to lift-off, a
    /// landing's from touchdown to the runway exit.
    fn end_speeds(&self) -> [f64; 2] {
        match self.kind {
            RollKind::TakeOff => [0.0, self.speed_ms],
            RollKind::Landing => [self.speed_ms, LANDING_EXIT_SPEED_MS.min(self.speed_ms)],
        }
    }

    /// The seconds the roll takes from `from_m` to `to_m` along it, the part of it within the roll,
    /// at its constant acceleration (Eq. 3-3): distance over speed at a steady speed, and finite
    /// from the start of roll, where the aircraft sets off from rest.
    pub fn seconds_between(&self, from_m: f64, to_m: f64) -> f64 {
        let (from_m, to_m) = (from_m.max(0.0), to_m.min(self.length_m));
        if to_m <= from_m {
            return 0.0;
        }
        let [start_speed, end_speed] = self.end_speeds();
        let speed_at = |at_m: f64| {
            let squared = start_speed * start_speed
                + (end_speed * end_speed - start_speed * start_speed) * at_m / self.length_m;
            squared.max(0.0).sqrt()
        };
        2.0 * (to_m - from_m) / (speed_at(from_m) + speed_at(to_m))
    }

    /// The roll's segments (Eq. 3-3a to 3-3i): a take-off's speed rises by equal steps of at most
    /// 10 m/s at constant acceleration, at its takeoff rating at each step's mean speed; a
    /// landing's falls to 15 m/s likewise, its power from the final approach's at touchdown to a
    /// fifth of the maximum over the first tenth of the stop, then to a tenth at the stop, with
    /// reverse thrust's increment rising to 5 dB over that tenth and falling to 0 at the stop, and
    /// a segment ending at that tenth, where the power turns (3.6.3's "suitable segmentation").
    /// Empty for a helicopter's class or a roll without length or speed.
    pub fn steps(&self) -> Vec<RollStep> {
        if is_helicopter_class(self.class) || self.length_m <= 0.0 || self.speed_ms <= 0.0 {
            return Vec::new();
        }
        let anchor = class_anchor(self.class);
        let [start_speed, end_speed] = self.end_speeds();
        let count = (1.0 + (end_speed - start_speed).abs() / SPEED_STEP_MS).floor() as usize;
        let step = (end_speed - start_speed) / count as f64;
        // Constant acceleration: the distance covered goes as the change of the squared speed.
        let span = (end_speed * end_speed - start_speed * start_speed)
            .abs()
            .max(1e-9);
        let along =
            |speed: f64| self.length_m * (speed * speed - start_speed * start_speed).abs() / span;
        let mut speeds: Vec<f64> = (0..=count).map(|k| start_speed + k as f64 * step).collect();
        if self.kind == RollKind::Landing {
            let turn = (start_speed * start_speed - REVERSE_SHARE_OF_STOP * span).sqrt();
            if let Some(k) = (speeds.windows(2)).position(|pair| pair[0] > turn && turn > pair[1]) {
                speeds.insert(k + 1, turn);
            }
        }
        let aircraft = AircraftType {
            class: self.class,
            helicopter: None,
        };
        let landing_power = (self.kind == RollKind::Landing).then(|| self.landing_power());
        speeds
            .windows(2)
            .filter_map(|pair| {
                let (from_speed, to_speed) = (pair[0], pair[1]);
                let mean_kt = 0.5 * (from_speed + to_speed) / METRES_PER_SECOND_PER_KNOT;
                let (from_m, to_m) = (along(from_speed), along(to_speed));
                let speed_correction_db = roll_speed_correction_db(anchor.v_ref_kt, mean_kt);
                let take_off_at = |speed_ms: f64| SegmentFlight {
                    departure: true,
                    on_ground: true,
                    speed_kt: speed_ms / METRES_PER_SECOND_PER_KNOT,
                    pressure_altitude_m: self.field_altitude_m,
                    climb_sine: 0.0,
                    acceleration_ms2: None,
                    height_above_field_m: 0.0,
                };
                let shares = [from_m, to_m].map(|at| at / self.length_m);
                let (emission, thrust, increment_db) = match &landing_power {
                    None => (
                        SegmentEmission::new(
                            &aircraft,
                            &take_off_at(0.5 * (from_speed + to_speed)),
                            false,
                        )?,
                        [from_speed, to_speed]
                            .map(|speed| segment_thrust_lb(self.class, &take_off_at(speed))),
                        [0.0; 2],
                    ),
                    Some(power) => (
                        SegmentEmission {
                            class: self.class,
                            departure: false,
                            power: PowerBracket::FIRST_ROW,
                            installation: anchor.installation,
                            speed_correction_db,
                            helicopter_correction_db: 0.0,
                            atmosphere_shift_db: Default::default(),
                        },
                        shares.map(|share| power.thrust_lb(share)),
                        shares.map(reverse_increment_db),
                    ),
                };
                Some(RollStep {
                    from_m,
                    to_m,
                    emission: SegmentEmission {
                        speed_correction_db,
                        ..emission
                    },
                    thrust_lb: thrust[0].zip(thrust[1]).map(|(start, end)| [start, end]),
                    increment_db,
                })
            })
            .collect()
    }

    /// A landing's thrust per engine (lb) at touchdown, and the maximum the reverse and last
    /// shares take.
    fn landing_power(&self) -> LandingPower {
        let touchdown = SegmentFlight {
            departure: false,
            on_ground: false,
            speed_kt: self.speed_ms / METRES_PER_SECOND_PER_KNOT,
            pressure_altitude_m: self.field_altitude_m,
            climb_sine: -GLIDESLOPE_DEG.to_radians().sin(),
            acceleration_ms2: None,
            height_above_field_m: 0.0,
        };
        let take_off_rating = SegmentFlight {
            departure: true,
            on_ground: true,
            climb_sine: 0.0,
            ..touchdown
        };
        LandingPower {
            touchdown_lb: segment_thrust_lb(self.class, &touchdown),
            maximum_lb: segment_thrust_lb(self.class, &take_off_rating),
        }
    }
}

/// A landing roll's thrust: the final approach's at touchdown and the maximum available (lb per
/// engine); `None` for a class without a thrust model.
struct LandingPower {
    touchdown_lb: Option<f64>,
    maximum_lb: Option<f64>,
}

impl LandingPower {
    /// The thrust (lb per engine) at `share` of the stop distance (0 at touchdown, 1 at the stop).
    fn thrust_lb(&self, share: f64) -> Option<f64> {
        let (touchdown, maximum) = (self.touchdown_lb?, self.maximum_lb?);
        let reverse = REVERSE_POWER_SHARE * maximum;
        Some(if share <= REVERSE_SHARE_OF_STOP {
            touchdown + (reverse - touchdown) * share / REVERSE_SHARE_OF_STOP
        } else {
            reverse
                + (STOP_POWER_SHARE * maximum - reverse) * (share - REVERSE_SHARE_OF_STOP)
                    / (1.0 - REVERSE_SHARE_OF_STOP)
        })
    }
}

/// Reverse thrust's increment (dB) at `share` of the stop distance: rising to 5 dB over its first
/// tenth, then falling to 0 at the stop (3.6.3).
pub fn reverse_increment_db(share: f64) -> f64 {
    if share <= REVERSE_SHARE_OF_STOP {
        REVERSE_INCREMENT_DB * share.max(0.0) / REVERSE_SHARE_OF_STOP
    } else {
        REVERSE_INCREMENT_DB * (1.0 - share).max(0.0) / (1.0 - REVERSE_SHARE_OF_STOP)
    }
}

#[cfg(test)]
#[path = "roll_profile_tests.rs"]
mod tests;

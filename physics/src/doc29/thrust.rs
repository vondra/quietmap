//! Per-segment engine power (Doc 29 4th ed. Vol 2 Eq. 4-3, B-1, B-12): the corrected net thrust a
//! segment flies selects two bracketing NPD power rows and a weight, once per segment.

use super::approach_generated::APPROACH;
use super::npd::METRES_PER_FOOT;
use super::thrust_generated::THRUST;

/// Power-row stride of the per-class thrust tables; generated padding repeats the loudest row.
pub const MAX_POWER_ROWS: usize = 6;

/// The thrust model of one noise class (rows generated in `thrust_generated`).
pub struct ThrustModel {
    pub class_name: &'static str,
    pub anchor_name: &'static str,
    /// False for the pinned classes (the fallback proxy and the helicopters): they read
    /// their anchor profile's curve on row 0.
    pub has_thrust: bool,
    pub engines: u8,
    /// Median DEFAULT stage weight (lb): observed stage lengths are unknown.
    pub weight_lb: f64,
    /// Clean-configuration drag/lift ratio R (the minimum-R departure flap).
    pub drag_ratio: f64,
    /// Cutback height (ft above the field): the initial climb flies MaxTakeoff below it.
    pub cutback_ft_afe: f64,
    /// Eq. B-1 coefficients (E, F, Ga, Gb, H) per rating.
    pub takeoff_coef: [f64; 5],
    pub climb_coef: [f64; 5],
    pub idle_coef: [f64; 5],
    /// Eq. B-5 (propeller efficiency, net propulsive power in hp) at MaxTakeoff and MaxClimb of a
    /// class the ANP rates by its propeller, zeros for the others: those two ratings are then
    /// 326 eta P / (V_T delta), and the class has no idle rating (0 lb: its lowest row bounds it).
    pub propeller: [[f64; 2]; 2],
    /// Departure rows: count, corrected net thrust per engine (lb), SEL and LAmax curves.
    pub dep_rows: u8,
    pub dep_power: [f64; MAX_POWER_ROWS],
    pub dep_sel: [[f64; 10]; MAX_POWER_ROWS],
    pub dep_lmax: [[f64; 10]; MAX_POWER_ROWS],
    /// Approach rows, as the departure rows.
    pub app_rows: u8,
    pub app_power: [f64; MAX_POWER_ROWS],
    pub app_sel: [[f64; 10]; MAX_POWER_ROWS],
    pub app_lmax: [[f64; 10]; MAX_POWER_ROWS],
}

impl ThrustModel {
    /// A pinned class: no thrust tables.
    pub const fn pinned(class_name: &'static str, anchor_name: &'static str) -> Self {
        ThrustModel {
            class_name,
            anchor_name,
            has_thrust: false,
            engines: 0,
            weight_lb: 0.0,
            drag_ratio: 0.0,
            cutback_ft_afe: 0.0,
            takeoff_coef: [0.0; 5],
            climb_coef: [0.0; 5],
            idle_coef: [0.0; 5],
            propeller: [[0.0; 2]; 2],
            dep_rows: 1,
            dep_power: [0.0; MAX_POWER_ROWS],
            dep_sel: [[0.0; 10]; MAX_POWER_ROWS],
            dep_lmax: [[0.0; 10]; MAX_POWER_ROWS],
            app_rows: 1,
            app_power: [0.0; MAX_POWER_ROWS],
            app_sel: [[0.0; 10]; MAX_POWER_ROWS],
            app_lmax: [[0.0; 10]; MAX_POWER_ROWS],
        }
    }
}

/// The final approach configuration of a noise class (Doc 29 Vol 2 B11, `approach_generated`):
/// below `from_ft_afe` an arrival flies its landing flap with the gear down at 90 % of its maximum
/// landing weight, and its thrust is the glideslope's force balance with that drag (Eq. B-25).
pub struct ApproachConfiguration {
    pub class_name: &'static str,
    pub anchor: &'static str,
    pub flap: &'static str,
    pub drag_ratio: f64,
    pub from_ft_afe: f64,
    pub landing_weight_lb: f64,
}

impl ApproachConfiguration {
    /// A class without a thrust model.
    pub const fn none(class_name: &'static str) -> Self {
        ApproachConfiguration {
            class_name,
            anchor: "",
            flap: "",
            drag_ratio: 0.0,
            from_ft_afe: 0.0,
            landing_weight_lb: 0.0,
        }
    }
}

/// Eq. B-25's constant: the deceleration of a constant-CAS descent into an 8 kt headwind.
const APPROACH_K: f64 = 1.03;

/// Doc 29 Eq. 4-3 bracket: the lower power row and the weight toward the next one (0 at and past
/// the table's edges, so a bracket never reads beyond the tabulated rows).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PowerBracket {
    pub row: usize,
    pub weight: f64,
}

impl PowerBracket {
    /// Row 0 alone: every pinned class.
    pub const FIRST_ROW: PowerBracket = PowerBracket {
        row: 0,
        weight: 0.0,
    };

    /// The bracket as stored in a tile (the boxes' loudest pieces): the row in the top 3 bits,
    /// the weight in the low 13 (steps of 1/8191, under 1e-3 dB of level).
    pub fn code(self) -> u16 {
        assert!(self.row < MAX_POWER_ROWS && (0.0..=1.0).contains(&self.weight));
        ((self.row as u16) << 13) | (self.weight * f64::from(WEIGHT_STEPS)).round() as u16
    }

    /// The bracket of a stored [`code`](Self::code).
    pub fn from_code(code: u16) -> Self {
        PowerBracket {
            row: usize::from(code >> 13).min(MAX_POWER_ROWS - 1),
            weight: f64::from(code & WEIGHT_STEPS) / f64::from(WEIGHT_STEPS),
        }
    }
}

/// Weight steps of a stored power bracket.
const WEIGHT_STEPS: u16 = (1 << 13) - 1;

/// What a segment flies, independent of any receiver: the inputs of its power bracket and of its
/// speed correction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentFlight {
    /// Doc 29 A.3.2 climb classification by Stage 1: departure NPDs, else approach NPDs.
    pub departure: bool,
    /// On the runway or taxiway: takeoff roll at MaxTakeoff, landing roll and taxi at idle
    /// (reversers have no ANP model).
    pub on_ground: bool,
    /// Ground speed (kt); the calibrated airspeed is taken as ground speed x sqrt(sigma) (no wind).
    pub speed_kt: f64,
    /// Pressure altitude of the segment's middle (m): the ISA state of Eq. B-1.
    pub pressure_altitude_m: f64,
    /// Sine of the climb angle over the segment's 3-D length.
    pub climb_sine: f64,
    /// Along-track acceleration (m/s^2) from the flight's neighbouring airborne segments, `None`
    /// unknown: the energy an accelerating climb puts into speed, which the climb angle alone does
    /// not show.
    pub acceleration_ms2: Option<f64>,
    /// Height above the field (m): for a departure the altitude minus the terrain under its own
    /// takeoff roll, else (and when the roll was not observed) the height above the local ground.
    pub height_above_field_m: f64,
}

/// ISA pressure ratio delta at pressure altitude `h_ft`.
fn isa_pressure_ratio(h_ft: f64) -> f64 {
    (1.0 - 6.8756e-6 * h_ft).powf(5.2559)
}

/// ISA density ratio sigma at pressure altitude `h_ft`.
fn isa_density_ratio(h_ft: f64) -> f64 {
    (1.0 - 6.8756e-6 * h_ft).powf(4.2559)
}

/// ISA temperature (deg C) at pressure altitude `h_ft`.
fn isa_temperature_c(h_ft: f64) -> f64 {
    15.0 - 1.98 * h_ft / 1000.0
}

/// Eq. B-1: corrected net thrust per engine (lb) at a rating, calibrated airspeed `vc_kt`,
/// pressure altitude `h_ft` and ambient `temperature_c`.
fn rated_thrust_lb(coef: &[f64; 5], vc_kt: f64, h_ft: f64, temperature_c: f64) -> f64 {
    coef[0] + coef[1] * vc_kt + coef[2] * h_ft + coef[3] * h_ft * h_ft + coef[4] * temperature_c
}

/// The engine ratings of the thrust model.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Rating {
    Takeoff,
    Climb,
    Idle,
}

impl ThrustModel {
    /// Corrected net thrust per engine (lb) at a rating: Eq. B-1, or for a propeller class Eq. B-5
    /// with the true airspeed V_C / sqrt(sigma) (Eq. B-6; below 1 kt, where B-5 has no meaning,
    /// read at 1 kt: the table's top row).
    fn rated_thrust_lb(&self, rating: Rating, vc_kt: f64, h_ft: f64, temperature_c: f64) -> f64 {
        if self.propeller[0][1] > 0.0 {
            let [efficiency, power_hp] = match rating {
                Rating::Takeoff => self.propeller[0],
                Rating::Climb => self.propeller[1],
                Rating::Idle => return 0.0,
            };
            let vt_kt = (vc_kt / isa_density_ratio(h_ft).sqrt()).max(1.0);
            return 326.0 * efficiency * power_hp / vt_kt / isa_pressure_ratio(h_ft);
        }
        let coef = match rating {
            Rating::Takeoff => &self.takeoff_coef,
            Rating::Climb => &self.climb_coef,
            Rating::Idle => &self.idle_coef,
        };
        rated_thrust_lb(coef, vc_kt, h_ft, temperature_c)
    }
}

/// Standard gravity (m/s^2).
const GRAVITY_MS2: f64 = 9.806_65;

/// The climb and acceleration term of the force balance: with an observed acceleration G + a/g
/// (Eq. B-20), else G / K (Eqs. B-12, B-25: K stands for the acceleration a climb at constant
/// calibrated airspeed implies). Without a/g an accelerating climb after cutback, which flies
/// MaxClimb, read as a shallow climb at some 30 % less thrust, 3-4 dB of the departure NPD.
fn climb_term(climb_sine: f64, acceleration_ms2: Option<f64>, k: f64) -> f64 {
    match acceleration_ms2 {
        Some(acceleration) => climb_sine + acceleration / GRAVITY_MS2,
        None => climb_sine / k,
    }
}

/// Eqs. B-12, B-17 and B-20 inverted (no bank): corrected thrust per engine holding the climb
/// angle and the acceleration, N Fn/delta = (W/delta)(R + the climb term); `k` 1.01 at Vc <= 200
/// kt, else 0.95.
fn force_balance_thrust_lb(
    model: &ThrustModel,
    climb_sine: f64,
    acceleration_ms2: Option<f64>,
    k: f64,
    delta: f64,
) -> f64 {
    (model.weight_lb / delta) * (model.drag_ratio + climb_term(climb_sine, acceleration_ms2, k))
        / f64::from(model.engines)
}

/// The power bracket of a segment of noise class `class`. Pinned classes read row 0; ground
/// rolls fly their rating (takeoff or idle); a departure below the cutback height above its field
/// flies MaxTakeoff; everything else holds its climb angle by force balance within [Idle,
/// MaxClimb]. Where those two fits cross, both are outside the envelope they were fitted on (the
/// A380's idle fit passes its climb fit at 19,600 ft at 250 kt, the E170's climb fit is negative
/// at FL290), and the force balance stands alone: dev4 dropped such segments, a quiet place's
/// cruise traffic. `None` for a non-finite thrust; impossible observations are Stage 1's to drop.
pub fn power_bracket(class: usize, flight: &SegmentFlight) -> Option<PowerBracket> {
    let model = &THRUST[class];
    if !model.has_thrust {
        return Some(PowerBracket::FIRST_ROW);
    }
    let (powers, rows) = if flight.departure {
        (&model.dep_power, model.dep_rows)
    } else {
        (&model.app_power, model.app_rows)
    };
    let h_ft = flight.pressure_altitude_m / METRES_PER_FOOT;
    let delta = isa_pressure_ratio(h_ft);
    let vc_kt = flight.speed_kt * isa_density_ratio(h_ft).sqrt();
    let temperature_c = isa_temperature_c(h_ft);
    let rated = |rating| model.rated_thrust_lb(rating, vc_kt, h_ft, temperature_c);
    let within_ratings = |balance: f64| {
        let (idle, climb) = (rated(Rating::Idle), rated(Rating::Climb));
        if idle.is_finite() && climb.is_finite() && idle <= climb {
            balance.clamp(idle, climb)
        } else {
            balance
        }
    };
    let thrust_lb = if flight.on_ground {
        rated(if flight.departure {
            Rating::Takeoff
        } else {
            Rating::Idle
        })
    } else if flight.departure
        && flight.height_above_field_m < model.cutback_ft_afe * METRES_PER_FOOT
    {
        rated(Rating::Takeoff)
    } else if !flight.departure
        && flight.height_above_field_m <= APPROACH[class].from_ft_afe * METRES_PER_FOOT
    {
        // B11: the landing flap and the gear hold the glideslope's thrust well above idle; with
        // the clean ratio of the climb an approach read idle all the way down (1-4 dB of the
        // approach NPD of the A320neo family, 10 dB of a business jet's).
        let approach = &APPROACH[class];
        within_ratings(
            (approach.landing_weight_lb / delta)
                * (approach.drag_ratio
                    + climb_term(flight.climb_sine, flight.acceleration_ms2, APPROACH_K))
                / f64::from(model.engines),
        )
    } else {
        let k = if vc_kt <= 200.0 { 1.01 } else { 0.95 };
        within_ratings(force_balance_thrust_lb(
            model,
            flight.climb_sine,
            flight.acceleration_ms2,
            k,
            delta,
        ))
    };
    // A NaN fails every comparison of the bracket and would read the loudest row.
    thrust_lb
        .is_finite()
        .then(|| bracket_power(powers, rows, thrust_lb))
}

/// Eq. 4-3 bracket of corrected thrust `thrust_lb` over the first `rows` tabulated `powers`;
/// thrust outside the table takes the edge row with weight 0.
pub(crate) fn bracket_power(
    powers: &[f64; MAX_POWER_ROWS],
    rows: u8,
    thrust_lb: f64,
) -> PowerBracket {
    let last = usize::from(rows) - 1;
    let edge = |row| PowerBracket { row, weight: 0.0 };
    if thrust_lb <= powers[0] {
        return edge(0);
    }
    match (0..last).find(|&row| thrust_lb < powers[row + 1]) {
        Some(row) => PowerBracket {
            row,
            weight: (thrust_lb - powers[row]) / (powers[row + 1] - powers[row]),
        },
        None => edge(last),
    }
}

#[cfg(test)]
#[path = "thrust_tests.rs"]
mod tests;

//! The corrections of an NPD level in Doc 29 4th ed. Vol 2 Eq. 4-8b: speed Delta_V, finite
//! segment Delta_F, lateral attenuation Lambda and engine installation Delta_I.

use std::f64::consts::FRAC_1_PI;

use super::npd::Installation;

/// Largest installation correction over every installation and angle: Eq. 4-15 for wing-mounted
/// engines peaks at 0.401395 dB at 50.43 deg; fuselage and propeller never exceed 0.
pub const INSTALLATION_CORRECTION_MAX_DB: f64 = 0.4014;

/// Delta_V = 10 lg(V_ref / V) (Eq. 4-14) for ground speed `speed_kt`; 0 at or below 10 kt (dev4:
/// a hovering or stationary sample keeps the reference level instead of an unbounded gain).
pub fn speed_correction_db(reference_speed_kt: f64, speed_kt: f64) -> f64 {
    if speed_kt > 10.0 {
        10.0 * (reference_speed_kt / speed_kt).log10()
    } else {
        0.0
    }
}

/// Delta_F (Eq. 4-20): the share of the infinite line's sound energy that a segment of
/// `length_m` delivers, its closest point `along_m` from the segment's start along the track
/// (negative before it, beyond `length_m` after it), with scaled distance `scaled_distance_m`.
/// Never positive; 0 for a segment or scaled distance under 1 m (dev4).
pub fn finite_segment_correction_db(along_m: f64, length_m: f64, scaled_distance_m: f64) -> f64 {
    if length_m < 1.0 || scaled_distance_m < 1.0 {
        return 0.0;
    }
    let alpha_start = -along_m / scaled_distance_m;
    let alpha_end = -(along_m - length_m) / scaled_distance_m;
    let energy = |alpha: f64| alpha / (1.0 + alpha * alpha) + alpha.atan();
    let fraction = (energy(alpha_end) - energy(alpha_start)) * FRAC_1_PI;
    10.0 * fraction.max(1e-15).log10()
}

/// Lateral attenuation Gamma(l) Lambda(beta) (Eq. 4-17, 4-18) at elevation `height_m` above the
/// receiver and horizontal distance `lateral_m`: the ground's excess attenuation, for every
/// installation (helicopters by AEDT 2c Eq. 4-70), 0 above 50 deg; below the receiver (beta < 0)
/// dev4 keeps Lambda(0) = 10.857 dB scaled by Gamma. Never negative.
pub fn lateral_attenuation_db(height_m: f64, lateral_m: f64) -> f64 {
    let beta_deg = (height_m / lateral_m.max(0.01)).atan().to_degrees();
    if beta_deg > 50.0 {
        return 0.0;
    }
    let gamma = if lateral_m <= 914.0 {
        1.089 * (1.0 - (-0.00274 * lateral_m).exp())
    } else {
        1.0
    };
    let lambda_beta = if beta_deg < 0.0 {
        10.857
    } else {
        1.137 - 0.0229 * beta_deg + 9.72 * (-0.142 * beta_deg).exp()
    };
    gamma * lambda_beta
}

/// Installation correction Delta_I (Eq. 4-15) at elevation `height_m` above the receiver and
/// slant `slant_m`, the depression angle taken as the elevation angle (no bank); a receiver above
/// the aircraft reads it at 0 deg.
pub fn installation_correction_db(installation: Installation, height_m: f64, slant_m: f64) -> f64 {
    let (a, b, c) = match installation {
        Installation::Wing => (0.0039, 0.062, 0.8786),
        Installation::Fuselage => (0.1225, 0.329, 1.0),
        Installation::Propeller => return 0.0,
    };
    let height_m = height_m.max(0.0);
    let sin_squared = height_m * height_m / (slant_m * slant_m).max(1e-12);
    let cos_squared = 1.0 - sin_squared;
    let numerator = a * cos_squared + sin_squared;
    let denominator = c * 4.0 * sin_squared * cos_squared + (cos_squared - sin_squared).powi(2);
    if denominator > 0.0 && numerator > 0.0 {
        10.0 * (b * numerator.log10() - denominator.log10())
    } else {
        0.0
    }
}

#[cfg(test)]
#[path = "corrections_tests.rs"]
mod tests;

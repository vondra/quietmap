//! Take-off and landing ground rolls at a receiver by Doc 29 (4th ed. Vol 2), a roll's segments
//! ([`super::roll_profile`]) each with its true duration correction (Eq. 4-14 at the segment's mean
//! speed, no floor: the first take-off segment moves at a few metres a second, and its slowness is
//! most of what is heard beside the start of roll). Behind a take-off segment its level is the one
//! beside its start at the same distance (Eq. 4-21a, the start's lateral attenuation) turned by the
//! start-of-roll directivity (4.5.7, Eq. 4-22 to 4-25); ahead of a landing segment the one beside
//! its end (Eq. 4-21b, a semi-circular directivity). Beside a segment, or ahead of a take-off and
//! behind a landing segment, a segment is any segment (`super::segment`).

use super::corrections::{installation_energy_gain, lateral_attenuation_db};
use super::npd::Installation;
use super::roll_profile::RollStep;
use super::screening::{ReceiverHorizons, screened_sel_db};
use super::segment::{
    SegmentEmission, SegmentGeometry, closest_points, screened_segment_sel, segment_screening_db,
};
use super::thrust::{PowerBracket, bracket_of};
use std::f64::consts::PI;

/// Beyond this distance from a segment's start the start-of-roll directivity weakens (m; 2,500 ft,
/// Eq. 4-25).
const SOR_NORMALISING_M: f64 = 762.0;
/// The engines above the runway (m), as the German test tasks' ground segments hold them.
pub const ENGINE_HEIGHT_M: f64 = 2.0;

/// A take-off roll accelerates from rest to lift-off; a landing roll decelerates from touchdown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RollKind {
    TakeOff,
    Landing,
}

/// Delta_V of a ground roll's segment (Eq. 4-14) at its mean speed: no floor, the roll's
/// segmentation keeps the slowest segment at half a speed step.
pub fn roll_speed_correction_db(reference_speed_kt: f64, speed_kt: f64) -> f64 {
    10.0 * (reference_speed_kt / speed_kt).log10()
}

/// The energy fraction of a segment of `length_m` heard from its start, behind a take-off
/// segment (Eq. 4-21a, q = 0), or from its end, ahead of a landing segment (Eq. 4-21b, q = lambda),
/// with `scaled_distance_m` its d_lambda there: the same function of lambda / d_lambda.
pub fn end_energy_fraction(length_m: f64, scaled_distance_m: f64) -> f64 {
    let alpha = length_m / scaled_distance_m.max(1e-9);
    ((alpha / (1.0 + alpha * alpha) + alpha.atan()) / PI).max(1e-15)
}

/// Delta_SOR (dB) at azimuth `psi_deg` from the aircraft's heading (90 beside, 180 behind) and
/// `distance_m` from the segment's start: Eq. 4-24a for jets, 4-24b for propellers, weakened past
/// 762 m by Eq. 4-25.
pub fn start_of_roll_directivity_db(psi_deg: f64, distance_m: f64, propeller: bool) -> f64 {
    let psi = psi_deg.clamp(90.0, 179.999);
    let at_reference = if propeller {
        const C: [f64; 8] = [
            -34_643.898,
            30_722_161.987,
            -11_491_573_930.510,
            2_349_285_669_062.0,
            -283_584_441_904_272.0,
            20_227_150_391_251_300.0,
            -790_084_471_305_203_000.0,
            13_050_687_178_273_800_000.0,
        ];
        C.iter()
            .enumerate()
            .map(|(power, c)| c / psi.powi(power as i32))
            .sum()
    } else {
        let radians = psi * PI / 180.0;
        2_329.44 - 8.0573 * psi + 11.51 * radians.exp()
            - 3.4601 * psi / radians.ln()
            - 17_403_338.3 * radians.ln() / (psi * psi)
    };
    if distance_m <= SOR_NORMALISING_M {
        at_reference
    } else {
        at_reference * SOR_NORMALISING_M / distance_m
    }
}

/// One ground-roll segment at the receiver: its geometry with the start where the aircraft enters
/// it, what it emits (its Delta_V the roll's, [`roll_speed_correction_db`]), its thrust per engine
/// at start and end (lb; `None` keeps the emission's power) read into NPD rows by `bracket`, and
/// reverse thrust's increment at start and end (dB): both taken at the segment's point nearest the
/// receiver, an end behind or ahead of it (Doc 29 4.4.2, Eq. 4-12).
#[derive(Debug, Clone, Copy)]
pub struct RollSegment {
    pub geometry: SegmentGeometry,
    pub emission: SegmentEmission,
    pub thrust_lb: Option<[f64; 2]>,
    pub bracket: fn(usize, bool, f64) -> PowerBracket,
    pub increment_db: [f64; 2],
}

/// A ground-roll segment's SEL at the receiver and the slant its NPD curves were read at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RollSel {
    pub sel_db: f64,
    pub slant_m: f64,
}

/// The SEL of one ground-roll segment of a `kind` roll at the receiver, terrain and buildings
/// screening it as any low segment; behind a take-off or ahead of a landing segment, on the ray to
/// the end it is heard from (the infinite line's projection there lies under the receiver).
pub fn roll_segment_sel(
    kind: RollKind,
    segment: &RollSegment,
    horizons: &impl ReceiverHorizons,
) -> RollSel {
    let geometry = &segment.geometry;
    let closest = closest_points(geometry.start_m, geometry.end_m);
    let share = closest.along.clamp(0.0, 1.0);
    let mut emission = segment.emission;
    if let Some([start, end]) = segment.thrust_lb {
        let thrust_lb = (start * start + share * (end * end - start * start)).sqrt();
        emission.power = (segment.bracket)(emission.class, emission.departure, thrust_lb);
    }
    let [start_db, end_db] = segment.increment_db;
    let increment_db = start_db + share * (end_db - start_db);
    let end = match kind {
        RollKind::TakeOff if closest.along < 0.0 => Some(geometry.start_m),
        RollKind::Landing if closest.along > 1.0 => Some(geometry.end_m),
        _ => None,
    };
    let Some(end_m) = end else {
        let [east_m, north_m, up_m] = closest.on_line_m;
        let screening = segment_screening_db(geometry, &closest, horizons);
        let sel = screened_segment_sel(&emission, closest, screening);
        return RollSel {
            sel_db: sel.sel_db + increment_db,
            slant_m: east_m.hypot(north_m).hypot(up_m),
        };
    };
    let (terrain_loss_db, building_loss_db) = (
        horizons.terrain_loss_db(end_m),
        horizons.building_loss_db(end_m),
    );
    // The level beside the nearer end at the same distance (4.5.6), its lateral attenuation that of
    // the end's elevation angle (4.5.5).
    let [east_m, north_m, up_m] = end_m;
    let lateral_m = east_m.hypot(north_m);
    let distance_m = lateral_m.hypot(up_m).max(1.0);
    let npd = emission.read_npd(distance_m);
    let fraction = end_energy_fraction(closest.length_m, npd.scaled_distance_m);
    let lateral_attenuation = lateral_attenuation_db(up_m, lateral_m);
    let installation =
        10.0 * installation_energy_gain(emission.installation, up_m, distance_m).log10();
    let directivity = match kind {
        RollKind::TakeOff => {
            // Eq. 4-22: q the receiver's position along the track from the segment's start.
            let q = closest.along * closest.length_m;
            let psi_deg = (q / distance_m).clamp(-1.0, 1.0).acos().to_degrees();
            start_of_roll_directivity_db(
                psi_deg,
                distance_m,
                emission.installation == Installation::Propeller,
            )
        }
        RollKind::Landing => 0.0,
    };
    let free_sel_db = npd.sel_db + emission.speed_correction_db + installation
        - lateral_attenuation
        + 10.0 * fraction.log10()
        + directivity;
    RollSel {
        sel_db: screened_sel_db(
            free_sel_db,
            lateral_attenuation,
            terrain_loss_db,
            building_loss_db,
        ) + increment_db,
        slant_m: distance_m,
    }
}

/// One movement of `steps` of a `kind` roll at the receiver, its SEL energy (linear, all its
/// segments): the roll from `start_m` along the unit `direction` (receiver frame, metres; height
/// above the receiver in `start_m[2]`), `ground_m` the runway below the receiver's height.
pub fn roll_sel_energy(
    kind: RollKind,
    steps: &[RollStep],
    (start_m, direction): ([f64; 3], [f64; 2]),
    ground_m: f64,
    horizons: &impl ReceiverHorizons,
) -> f64 {
    let at = |along: f64| {
        [
            start_m[0] + direction[0] * along,
            start_m[1] + direction[1] * along,
            start_m[2],
        ]
    };
    (steps.iter())
        .map(|step| {
            let segment = RollSegment {
                geometry: SegmentGeometry {
                    start_m: at(step.from_m),
                    end_m: at(step.to_m),
                    ground_under_start_m: ground_m,
                    ground_under_end_m: ground_m,
                },
                emission: step.emission,
                thrust_lb: step.thrust_lb,
                bracket: bracket_of,
                increment_db: step.increment_db,
            };
            10f64.powf(roll_segment_sel(kind, &segment, horizons).sel_db / 10.0)
        })
        .sum()
}

#[cfg(test)]
#[path = "roll_tests.rs"]
mod tests;

//! One flight segment at one receiver (Doc 29 4th ed. Vol 2 Eq. 4-8b with dev4's screening): the
//! exact SEL the benchmark takes as the reference of the aircraft boxes, and the levels a box
//! sums at the ten NPD distances.

use super::atmosphere::{PlaceAtmosphere, SHIFT_DISTANCES};
use super::corrections::{
    INSTALLATION_CORRECTION_MAX_DB, finite_segment_correction_db, installation_correction_db,
    lateral_attenuation_db, speed_correction_db,
};
use super::helicopters::{HelicopterLevels, helicopter_levels};
use super::npd::{
    Installation, METRES_PER_FOOT, NPD_DISTANCES, NPD_DISTANCES_FT, NpdReading, TAIL_ANCHOR_M,
    class_anchor, is_helicopter_class, read_npd,
};
use super::profiles_generated::{noise_class_of, profile_idx};
use super::screening::{ReceiverHorizons, SCREENING_CEILING_ABOVE_GROUND_M, screened_sel_db};
use super::thrust::{PowerBracket, SegmentFlight, power_bracket};

/// A type designator resolved once per flight: its noise class, and a helicopter's levels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AircraftType {
    pub class: usize,
    pub helicopter: Option<HelicopterLevels>,
}

impl AircraftType {
    /// By the generated designator mapping (`profile_idx`, unknown types on the fallback class).
    pub fn from_designator(designator: &str) -> Self {
        AircraftType::of(profile_idx(designator), designator)
    }

    /// The class of `profile` (the one Stage 1 decided for the flight); a helicopter's certified
    /// levels by its designator.
    pub fn of(profile: u8, designator: &str) -> Self {
        let class = usize::from(noise_class_of(profile));
        let helicopter = is_helicopter_class(class).then(|| helicopter_levels(designator));
        AircraftType { class, helicopter }
    }
}

/// What a segment emits, independent of any receiver: the NPD curves it reads and the offsets it
/// adds to them at every distance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentEmission {
    pub class: usize,
    /// Departure NPDs, else approach NPDs.
    pub departure: bool,
    pub power: PowerBracket,
    pub installation: Installation,
    /// Delta_V (dB).
    pub speed_correction_db: f64,
    /// The helicopter correction (dB); 0 for fixed wing.
    pub helicopter_correction_db: f64,
    /// How far the place's yearly atmosphere moves the curves from the model's, at the ten NPD
    /// distances and the tail anchor (zeros: the model atmosphere; [`Self::in_atmosphere`]).
    pub atmosphere_shift_db: [f64; SHIFT_DISTANCES],
}

/// The place's shift at `slant_m`: linear in lg d between the NPD distances and on to the tail
/// anchor, the first value nearer than 200 ft, past the tail anchor linear in distance (the
/// absorption difference grows with the path).
fn shift_at(shift: &[f64; SHIFT_DISTANCES], slant_m: f64) -> f64 {
    let distance = |k: usize| {
        if k < NPD_DISTANCES {
            NPD_DISTANCES_FT[k] * METRES_PER_FOOT
        } else {
            TAIL_ANCHOR_M
        }
    };
    if slant_m <= distance(0) {
        return shift[0];
    }
    let last = SHIFT_DISTANCES - 1;
    if slant_m >= distance(last) {
        let slope = (shift[last] - shift[last - 1]) / (distance(last) - distance(last - 1));
        return shift[last] + slope * (slant_m - distance(last));
    }
    let k = (0..last)
        .find(|&k| slant_m < distance(k + 1))
        .expect("inside the distances");
    let t = (slant_m / distance(k)).log10() / (distance(k + 1) / distance(k)).log10();
    shift[k] + t * (shift[k + 1] - shift[k])
}

/// The NPD levels of one segment at the ten NPD distances D_k: what a box sums at build time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NpdDistanceLevels {
    /// `L_E(P, D_k) + Delta_V + Delta_heli` (dB): the SEL at perpendicular slant D_k from the
    /// segment's infinite line before Delta_F, Delta_I, Lambda and screening. The weighted
    /// energies 10^(L/10) of a box's pieces add at each distance.
    pub sel_db: [f64; NPD_DISTANCES],
    /// d_lambda (m) at D_k: with the piece's length and the along-track position of the closest
    /// point it gives Delta_F. lg d_lambda is linear in lg d between and beyond the ten distances
    /// (exactly so for one power row: SEL - LAmax is linear in lg d, Eq. 4-11; the dipole limit
    /// d_lambda = d likewise).
    pub scaled_distance_m: [f64; NPD_DISTANCES],
    /// The same SEL at the box tail anchor ([`super::npd::TAIL_ANCHOR_M`]).
    pub tail_sel_db: f64,
}

impl SegmentEmission {
    /// `None` outside the thrust model's domain. `helicopter_descent` is Stage 1's whole-chord
    /// descent state; it matters only for helicopters that are not climbing.
    pub fn new(
        aircraft: &AircraftType,
        flight: &SegmentFlight,
        helicopter_descent: bool,
    ) -> Option<Self> {
        let anchor = class_anchor(aircraft.class);
        Some(SegmentEmission {
            class: aircraft.class,
            departure: flight.departure,
            power: power_bracket(aircraft.class, flight)?,
            installation: anchor.installation,
            speed_correction_db: speed_correction_db(anchor.v_ref_kt, flight.speed_kt),
            helicopter_correction_db: aircraft.helicopter.map_or(0.0, |levels| {
                levels.correction_db(flight.departure, helicopter_descent)
            }),
            atmosphere_shift_db: [0.0; SHIFT_DISTANCES],
        })
    }

    /// The segment's NPD values at `slant_m`, helicopter correction included in SEL and LAmax.
    pub fn read_npd(&self, slant_m: f64) -> NpdReading {
        let reading = read_npd(self.class, self.departure, self.power, slant_m);
        let offset = self.helicopter_correction_db + shift_at(&self.atmosphere_shift_db, slant_m);
        NpdReading {
            sel_db: reading.sel_db + offset,
            lamax_db: reading.lamax_db + offset,
            ..reading
        }
    }

    /// The emission in a place's yearly atmosphere.
    pub fn in_atmosphere(self, place: &PlaceAtmosphere) -> Self {
        SegmentEmission {
            atmosphere_shift_db: *place.shift_db(self.class, self.departure),
            ..self
        }
    }

    /// The segment's levels at the ten NPD distances (see [`NpdDistanceLevels`]).
    pub fn npd_distance_levels(&self) -> NpdDistanceLevels {
        let readings = NPD_DISTANCES_FT.map(|feet| self.read_npd(feet * METRES_PER_FOOT));
        NpdDistanceLevels {
            sel_db: readings.map(|reading| reading.sel_db + self.speed_correction_db),
            scaled_distance_m: readings.map(|reading| reading.scaled_distance_m),
            tail_sel_db: self.read_npd(TAIL_ANCHOR_M).sel_db + self.speed_correction_db,
        }
    }
}

/// A segment in the receiver's frame: metres east and north of the receiver and height above it,
/// all heights in one vertical datum.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentGeometry {
    pub start_m: [f64; 3],
    pub end_m: [f64; 3],
    /// Terrain height under the start and under the end, above the receiver (m).
    pub ground_under_start_m: f64,
    pub ground_under_end_m: f64,
}

/// Where a segment passes the receiver (Doc 29 Vol 2 4.4-4.5, Fig. 4-6 and 4-7): S, the foot of the
/// perpendicular from the receiver to the segment's line in three dimensions, its slant d_p; the
/// receiver's lateral displacement from the ground track; and the height of the equivalent level
/// path the lateral attenuation and the installation effect take (Codex, review of the r054 plan:
/// the foot over the ground track, which dev4 took, read a 27 degree climb at 110 m for 102).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClosestPoints {
    /// Position of S: 0 at the start, 1 at the end, outside on the extensions.
    pub along: f64,
    /// S on the segment's infinite line: its slant d_p is the exposure level's NPD distance.
    pub on_line_m: [f64; 3],
    /// The segment's point nearest the receiver (S clamped to it): the maximum level's distance,
    /// the aircraft's real closest position and the point the building horizon screens.
    pub on_segment_m: [f64; 3],
    /// The perpendicular distance l from the receiver to the (extended) ground track (OR, OC).
    pub lateral_m: f64,
    /// The height of the equivalent level path at slant d_p, (d_p^2 - l^2)^0.5, negative below
    /// the receiver: the elevation and depression angle in the plane normal to the flight path.
    pub height_m: f64,
    /// The equivalent level path's height for the exposure level's lateral attenuation: the
    /// above alongside the segment; behind or ahead of it the perpendicular from the ground track
    /// to the nearer end (RS1 in Fig. 4-7).
    pub lambda_height_m: f64,
    /// Length of the segment, at least 1 m.
    pub length_m: f64,
}

/// The closest points of a segment given in the receiver's frame. A (near-)vertical segment has
/// no ground track: its start stands for it (dev4).
pub fn closest_points(start_m: [f64; 3], end_m: [f64; 3]) -> ClosestPoints {
    let delta = [0, 1, 2].map(|axis| end_m[axis] - start_m[axis]);
    let horizontal_squared = delta[0] * delta[0] + delta[1] * delta[1];
    let length_squared = horizontal_squared + delta[2] * delta[2];
    let point = |t: f64| [0, 1, 2].map(|axis| start_m[axis] + t * delta[axis]);
    if horizontal_squared <= 1e-6 {
        return ClosestPoints {
            along: 0.0,
            on_line_m: start_m,
            on_segment_m: start_m,
            lateral_m: start_m[0].hypot(start_m[1]),
            height_m: start_m[2],
            lambda_height_m: start_m[2],
            length_m: length_squared.sqrt().max(1.0),
        };
    }
    let along = -(0..3).map(|axis| start_m[axis] * delta[axis]).sum::<f64>() / length_squared;
    let on_line_m = point(along);
    let on_segment_m = point(along.clamp(0.0, 1.0));
    let lateral_m =
        (start_m[0] * delta[1] - start_m[1] * delta[0]).abs() / horizontal_squared.sqrt();
    let slant_squared: f64 = on_line_m.iter().map(|value| value * value).sum();
    let height_m = (slant_squared - lateral_m * lateral_m)
        .max(0.0)
        .sqrt()
        .copysign(on_line_m[2]);
    // Behind or ahead: the nearer end's height measured perpendicular to the inclined path.
    let cos_climb = (horizontal_squared / length_squared).sqrt();
    let lambda_height_m = if (0.0..=1.0).contains(&along) {
        height_m
    } else {
        on_segment_m[2] / cos_climb
    };
    ClosestPoints {
        along,
        on_line_m,
        on_segment_m,
        lateral_m,
        height_m,
        lambda_height_m,
        length_m: length_squared.sqrt().max(1.0),
    }
}

/// A segment's SEL at the receiver and the terms of Eq. 4-8b.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentSel {
    /// After terrain and building screening (dB).
    pub sel_db: f64,
    /// Before screening (dB).
    pub free_sel_db: f64,
    pub closest: ClosestPoints,
    /// NPD values at the slant to the line's closest point, helicopter correction included.
    pub npd: NpdReading,
    pub finite_segment_correction_db: f64,
    pub lateral_attenuation_db: f64,
    pub installation_correction_db: f64,
    pub terrain_loss_db: f64,
    pub building_loss_db: f64,
}

/// The exact SEL of one segment at the receiver (Doc 29 4.4-4.5): the NPD level at the slant to
/// the closest point of the segment's line (not clamped), plus Delta_V, the helicopter
/// correction, Delta_I and Delta_F, minus Lambda, then terrain and building screening composed
/// with Lambda. Every segment is heard: dev4's Filter D, which dropped a segment whose extended
/// line passed more than 30 m under the ground at its closest point, is not Doc 29 (Delta_F
/// already keeps only the segment's own extent) and dropped real sound (0.4-0.5 dB 3-5 km beside
/// runways; owner decision 2026-09-30). No reach cut and no event floor: the box it is the
/// reference of can apply neither.
pub fn segment_sel_at_receiver(
    emission: &SegmentEmission,
    geometry: &SegmentGeometry,
    horizons: &impl ReceiverHorizons,
) -> SegmentSel {
    let closest = closest_points(geometry.start_m, geometry.end_m);
    let [east_m, north_m, up_m] = closest.on_line_m;
    let slant_m = east_m.hypot(north_m).hypot(up_m);
    let npd = emission.read_npd(slant_m);
    let finite = finite_segment_correction_db(
        closest.along * closest.length_m,
        closest.length_m,
        npd.scaled_distance_m,
    );
    let lateral_attenuation = lateral_attenuation_db(closest.lambda_height_m, closest.lateral_m);
    let installation = installation_correction_db(emission.installation, closest.height_m, slant_m);
    let free_sel_db =
        npd.sel_db + emission.speed_correction_db + installation - lateral_attenuation + finite;
    let height_above_ground_m = 0.5
        * (geometry.start_m[2] - geometry.ground_under_start_m + geometry.end_m[2]
            - geometry.ground_under_end_m);
    let (terrain_loss_db, building_loss_db) =
        if height_above_ground_m < SCREENING_CEILING_ABOVE_GROUND_M {
            (
                horizons.terrain_loss_db(closest.on_line_m),
                horizons.building_loss_db(closest.on_segment_m),
            )
        } else {
            (0.0, 0.0)
        };
    SegmentSel {
        sel_db: screened_sel_db(
            free_sel_db,
            lateral_attenuation,
            terrain_loss_db,
            building_loss_db,
        ),
        free_sel_db,
        closest,
        npd,
        finite_segment_correction_db: finite,
        lateral_attenuation_db: lateral_attenuation,
        installation_correction_db: installation,
        terrain_loss_db,
        building_loss_db,
    }
}

/// A segment's maximum level at the receiver (Doc 29 Eq. 4-8a, unscreened): the NPD LAmax at the
/// shortest distance to the segment, minus Lambda, plus Delta_I at the depression angle in the
/// plane normal to the flight path, as for the SEL; no Delta_V, no Delta_F. Alongside the segment
/// Lambda is the exposure level's; behind or ahead of it, that of the nearer end's elevation angle
/// and ground distance (4.5.4, Fig. 4-7).
pub fn segment_lmax_db(emission: &SegmentEmission, closest: &ClosestPoints) -> f64 {
    let [east_m, north_m, up_m] = closest.on_segment_m;
    let distance_m = east_m.hypot(north_m).hypot(up_m);
    let [line_east_m, line_north_m, line_up_m] = closest.on_line_m;
    let line_slant_m = line_east_m.hypot(line_north_m).hypot(line_up_m);
    let lateral_attenuation = if (0.0..=1.0).contains(&closest.along) {
        lateral_attenuation_db(closest.height_m, closest.lateral_m)
    } else {
        lateral_attenuation_db(up_m, east_m.hypot(north_m))
    };
    emission.read_npd(distance_m).lamax_db
        + installation_correction_db(emission.installation, closest.height_m, line_slant_m)
        - lateral_attenuation
}

/// The slant (m) within which a segment's maximum level (Eq. 4-8a) can reach `threshold_db`: its
/// LAmax curve plus the most Delta_I adds ([`INSTALLATION_CORRECTION_MAX_DB`]; Lambda never adds).
/// 0 where even its nearest reading stays below.
pub fn lmax_reach_m(emission: &SegmentEmission, threshold_db: f64) -> f64 {
    const NEAREST_M: f64 = 30.0;
    const FARTHEST_M: f64 = 60_000.0;
    let reaches = |slant_m: f64| {
        emission.read_npd(slant_m).lamax_db + INSTALLATION_CORRECTION_MAX_DB >= threshold_db
    };
    if !reaches(NEAREST_M) {
        return 0.0;
    }
    let (mut near, mut far) = (NEAREST_M, FARTHEST_M);
    if reaches(far) {
        return far;
    }
    // The curve falls with the slant: halve the ratio of the bracket in lg d.
    for _ in 0..24 {
        let middle = (near * far).sqrt();
        if reaches(middle) {
            near = middle;
        } else {
            far = middle;
        }
    }
    far
}

#[cfg(test)]
#[path = "segment_tests.rs"]
mod tests;

//! One flight segment at one receiver (Doc 29 4th ed. Vol 2 Eq. 4-8b with dev4's screening): the
//! exact SEL the benchmark takes as the reference of the aircraft boxes, and the levels a box
//! sums at the ten NPD distances.

use super::corrections::{
    finite_segment_correction_db, installation_correction_db, lateral_attenuation_db,
    speed_correction_db,
};
use super::helicopters::{HelicopterLevels, helicopter_levels};
use super::npd::{
    Installation, METRES_PER_FOOT, NPD_DISTANCES, NPD_DISTANCES_FT, NpdReading, class_anchor,
    is_helicopter_class, read_npd,
};
use super::profiles_generated::{noise_class_of, profile_idx};
use super::screening::{ReceiverHorizons, SCREENING_CEILING_ABOVE_GROUND_M, screened_sel_db};
use super::thrust::{PowerBracket, SegmentFlight, power_bracket};

/// dev4 Filter D: a segment whose straight extension, beyond the end nearer the receiver's closest
/// point, sinks more than this below the terrain under that end (touchdown, the last sample) is
/// not heard through that fictitious extension. The margin covers terrain and altitude errors.
const EXTENSION_BELOW_GROUND_M: f64 = 30.0;

/// A type designator resolved once per flight: its noise class, and a helicopter's levels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AircraftType {
    pub class: usize,
    pub helicopter: Option<HelicopterLevels>,
}

impl AircraftType {
    /// By the generated designator mapping (`profile_idx`, unknown types on the fallback class).
    pub fn from_designator(designator: &str) -> Self {
        let class = usize::from(noise_class_of(profile_idx(designator)));
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
        })
    }

    /// The segment's NPD values at `slant_m`, helicopter correction included in SEL and LAmax.
    pub fn read_npd(&self, slant_m: f64) -> NpdReading {
        let reading = read_npd(self.class, self.departure, self.power, slant_m);
        NpdReading {
            sel_db: reading.sel_db + self.helicopter_correction_db,
            lamax_db: reading.lamax_db + self.helicopter_correction_db,
            ..reading
        }
    }

    /// The segment's levels at the ten NPD distances (see [`NpdDistanceLevels`]).
    pub fn npd_distance_levels(&self) -> NpdDistanceLevels {
        let readings = NPD_DISTANCES_FT.map(|feet| self.read_npd(feet * METRES_PER_FOOT));
        NpdDistanceLevels {
            sel_db: readings.map(|reading| reading.sel_db + self.speed_correction_db),
            scaled_distance_m: readings.map(|reading| reading.scaled_distance_m),
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

/// Where a segment passes the receiver (Doc 29 4.4.1, dev4): the closest point of its horizontal
/// track line, not clamped to the segment, and the height of the segment's line there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClosestPoints {
    /// Position of that point: 0 at the start, 1 at the end, outside on the extensions.
    pub along: f64,
    /// The point on the segment's infinite line: the slant and angles of Eq. 4-8b.
    pub on_line_m: [f64; 3],
    /// The same foot clamped to the segment: where the aircraft really passes closest (dev4's
    /// display distance and altitude, and the point the building horizon screens).
    pub on_segment_m: [f64; 3],
    /// Horizontal length of the segment, at least 1 m.
    pub horizontal_length_m: f64,
}

/// The closest points of a segment given in the receiver's frame.
pub fn closest_points(start_m: [f64; 3], end_m: [f64; 3]) -> ClosestPoints {
    let delta = [0, 1, 2].map(|axis| end_m[axis] - start_m[axis]);
    let length_squared = delta[0] * delta[0] + delta[1] * delta[1];
    // A (near-)vertical segment has no track direction: its start stands for it (dev4).
    let along = if length_squared > 1e-6 {
        -(start_m[0] * delta[0] + start_m[1] * delta[1]) / length_squared
    } else {
        0.0
    };
    let point = |t: f64| [0, 1, 2].map(|axis| start_m[axis] + t * delta[axis]);
    ClosestPoints {
        along,
        on_line_m: point(along),
        on_segment_m: point(along.clamp(0.0, 1.0)),
        horizontal_length_m: length_squared.sqrt().max(1.0),
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

/// The exact SEL of one segment at the receiver, as dev4 computes it: the NPD level at the slant
/// to the closest point of the segment's line (not clamped), plus Delta_V, the helicopter
/// correction, Delta_I and Delta_F, minus Lambda, then terrain and building screening composed
/// with Lambda. `None` when dev4's Filter D rejects the segment at this receiver. No reach cut
/// and no event floor: the box it is the reference of can apply neither.
pub fn segment_sel_at_receiver(
    emission: &SegmentEmission,
    geometry: &SegmentGeometry,
    horizons: &impl ReceiverHorizons,
) -> Option<SegmentSel> {
    let closest = closest_points(geometry.start_m, geometry.end_m);
    let [east_m, north_m, height_m] = closest.on_line_m;
    let beyond_start =
        closest.along < 0.0 && height_m < geometry.ground_under_start_m - EXTENSION_BELOW_GROUND_M;
    let beyond_end =
        closest.along > 1.0 && height_m < geometry.ground_under_end_m - EXTENSION_BELOW_GROUND_M;
    if beyond_start || beyond_end {
        return None;
    }
    let lateral_m = east_m.hypot(north_m);
    let slant_m = lateral_m.hypot(height_m);
    let npd = emission.read_npd(slant_m);
    let finite = finite_segment_correction_db(
        closest.along * closest.horizontal_length_m,
        closest.horizontal_length_m,
        npd.scaled_distance_m,
    );
    let lateral_attenuation = lateral_attenuation_db(height_m, lateral_m);
    let installation = installation_correction_db(emission.installation, height_m, slant_m);
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
    Some(SegmentSel {
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
    })
}

#[cfg(test)]
#[path = "segment_tests.rs"]
mod tests;

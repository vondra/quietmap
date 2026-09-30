//! Screening of low aircraft by terrain and buildings around the receiver (dev4). Doc 29 has no
//! barrier term; like FAA AEDT 3f 4.3.6 (line-of-sight blockage) an edge's path difference gives
//! an insertion loss capped at 18 dB, applied as the larger of it and lateral attenuation, never
//! their sum (Berton, AIAA 2021). The loss is the ISO 9613-2 7.4 single-edge form with a 500 Hz
//! wavelength (0.685 m): Quiet Map's approximation, ISO 9613-2 excludes aircraft in flight.

/// `(C2 / lambda) C3` of ISO 9613-2 7.4 at lambda = 0.685 m: 20 / 0.685 per metre.
const PATH_DIFFERENCE_SLOPE_PER_M: f64 = 29.2;
/// 10 lg 3, removed so the loss is 0 dB at grazing: no step at the shadow boundary.
const GRAZING_LOSS_DB: f64 = 4.771_212_547_196_624;
/// AEDT's line-of-sight-blockage cap.
pub const SCREENING_CAP_DB: f64 = 18.0;
/// Sources at least this high above the ground are not screened (dev4's airborne/cruise boundary,
/// now its own constant): within reach their elevation exceeds 28 deg, above real horizons.
pub const SCREENING_CEILING_ABOVE_GROUND_M: f64 = 7_620.0;

/// Insertion loss (dB) of one edge with path difference `path_difference_m`:
/// `10 lg(3 + 29.2 delta) - 10 lg 3`, 0 at grazing, capped at 18 dB.
pub fn path_difference_loss_db(path_difference_m: f64) -> f64 {
    let raw = 10.0 * (3.0 + PATH_DIFFERENCE_SLOPE_PER_M * path_difference_m.max(0.0)).log10();
    (raw - GRAZING_LOSS_DB).clamp(0.0, SCREENING_CAP_DB)
}

/// Insertion loss (dB) of an edge `edge_range_m` from the receiver and `edge_height_m` above it
/// toward a source `source_range_m` away horizontally and `source_height_m` above the receiver, in
/// their vertical plane: the exact single-edge path difference |RE| + |ES| - |RS|, 0 when the edge
/// lies at or beyond the source or at or below the line of sight.
pub fn edge_loss_db(
    edge_range_m: f64,
    edge_height_m: f64,
    source_range_m: f64,
    source_height_m: f64,
) -> f64 {
    let blocks = edge_range_m < source_range_m
        && edge_height_m * source_range_m > source_height_m * edge_range_m;
    if !blocks {
        return 0.0;
    }
    let receiver_to_edge = edge_range_m.hypot(edge_height_m);
    let edge_to_source = (source_range_m - edge_range_m).hypot(source_height_m - edge_height_m);
    let receiver_to_source = source_range_m.hypot(source_height_m);
    path_difference_loss_db(receiver_to_edge + edge_to_source - receiver_to_source)
}

/// SEL (dB) after screening: the terrain and building losses compete by maximum, and only their
/// excess over the lateral attenuation already in `free_sel_db` is new.
pub fn screened_sel_db(
    free_sel_db: f64,
    lateral_attenuation_db: f64,
    terrain_loss_db: f64,
    building_loss_db: f64,
) -> f64 {
    free_sel_db - (terrain_loss_db.max(building_loss_db) - lateral_attenuation_db).max(0.0)
}

/// What the popup's receiver horizons answer: the insertion loss (dB, 0 to 18) toward a source
/// point given as metres east and north of the receiver and height above it. Each is the largest
/// [`edge_loss_db`] over the stored edges of the point's direction.
pub trait ReceiverHorizons {
    /// Terrain horizon, asked at the segment's infinite-line closest point.
    fn terrain_loss_db(&self, point_m: [f64; 3]) -> f64;
    /// Building horizon, asked at the closest point of the finite segment.
    fn building_loss_db(&self, point_m: [f64; 3]) -> f64;
}

/// No horizons: free field.
pub struct Unscreened;

impl ReceiverHorizons for Unscreened {
    fn terrain_loss_db(&self, _point_m: [f64; 3]) -> f64 {
        0.0
    }

    fn building_loss_db(&self, _point_m: [f64; 3]) -> f64 {
        0.0
    }
}

#[cfg(test)]
#[path = "screening_tests.rs"]
mod tests;

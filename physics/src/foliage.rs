//! Sound through tree crowns, the one term beside CNOSSOS-EU's (which has none): ISO 9613-2:1996
//! Annex A, Table A.1, on the metres each meteorological state's ray runs inside the crowns,
//! weighted by the tree cover (dev4's depth). On the ruler, Poland's road stations whose rays cross
//! forest read louder than measured without it, and louder than the stations without forest at
//! the same level; the full table closes that gap, half of it does not
//! (`evidence/2026-10-10/forest/`).

use crate::bands::BANDS;
use crate::cnossos::PlanePoint;
use crate::cnossos::rubber_band::{StateRay, distance};
use crate::profile::Profile;

/// Table A.1 from 10 to 20 m of foliage (dB); nothing under 10 m.
const SHORT_DB: [f64; BANDS] = [0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 2.0, 3.0];
/// Table A.1 from 20 m (dB a metre), the depth taken at most 200 m.
const DB_PER_M: [f64; BANDS] = [0.02, 0.03, 0.04, 0.05, 0.06, 0.08, 0.09, 0.12];
const SHORT_FROM_M: f64 = 10.0;
const PER_METRE_FROM_M: f64 = 20.0;
const DEPTH_MAXIMUM_M: f64 = 200.0;

/// The attenuation of `depth_m` metres of foliage per band (dB).
pub fn attenuation_db(depth_m: f64) -> [f64; BANDS] {
    if depth_m < SHORT_FROM_M {
        [0.0; BANDS]
    } else if depth_m < PER_METRE_FROM_M {
        SHORT_DB
    } else {
        DB_PER_M.map(|per_metre| per_metre * depth_m.min(DEPTH_MAXIMUM_M))
    }
}

/// The metres of `ray` from `source` to `receiver` (horizontal distance from the source,
/// altitude) inside tree crowns over `profile`, each weighted by the tree cover. A sample is inside
/// where the ray passes above the ground and at most the crowns' height above it; between two
/// samples the weight is linear.
pub fn depth_m(profile: &Profile, ray: &StateRay, source: PlanePoint, receiver: PlanePoint) -> f64 {
    let sag = ray.sag(source, receiver);
    let length = receiver.0 - source.0;
    let point = |sample: usize| {
        let t = profile.t[sample];
        (
            t * length,
            source.1 + t * (receiver.1 - source.1) + sag.at(t * length),
        )
    };
    let weight = |sample: usize, (_, altitude): PlanePoint| {
        let above = altitude - profile.ground_m[sample];
        if above > 0.0 && above <= profile.canopy_m[sample] {
            profile.forest_cover[sample]
        } else {
            0.0
        }
    };
    let cover = &profile.forest_cover;
    let mut depth = 0.0;
    for sample in 1..profile.t.len() {
        if cover[sample - 1] <= 0.0 && cover[sample] <= 0.0 {
            continue;
        }
        let (a, b) = (point(sample - 1), point(sample));
        depth += 0.5 * (weight(sample - 1, a) + weight(sample, b)) * distance(a, b);
    }
    depth
}

#[cfg(test)]
#[path = "foliage_tests.rs"]
mod tests;

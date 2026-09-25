//! A_boundary of one state: A_ground of the whole path, or A_dif over the state's diffraction
//! points with the Δground split on both sides (CNOSSOS-EU 2.5.21–2.5.32; 2021/1226 point (9);
//! ISO/TR 17534-4 §5.3, §5.9), band by band.

use super::ground::{
    ground_attenuation_bands, ground_factor_near_source, GroundFactors, MeteorologicalState,
};
use super::mean_plane::{fit_mean_plane, EquivalentGeometry};
use super::rubber_band::{diffraction_path, DiffractionPath, PlanePoint, StateRay};
use super::{StateBoundary, VerticalPath};
use crate::constants::{BAND_FREQ, SPEED_OF_SOUND};
use crate::types::NUM_BANDS;

/// Δdif(S,R) above this is capped (§2.5.6 "Δdif > 25 dB"); the Δground terms are not.
pub const DIFFRACTION_CAP_DB: f64 = 25.0;
/// C″ applies to multiple diffraction when the diffraction points span more than this (2.5.23).
pub const MULTIPLE_DIFFRACTION_MINIMUM_SPAN_M: f64 = 0.3;

/// (2.5.21) with C_h = 1: `10·lg(3 + 40·C″·δ/λ)` while its argument reaches −2, else 0.
fn diffraction_db(path_difference_m: f64, c_second: &[f64; NUM_BANDS]) -> [f64; NUM_BANDS] {
    std::array::from_fn(|band| {
        let lambda = SPEED_OF_SOUND / BAND_FREQ[band];
        let x = 40.0 * c_second[band] * path_difference_m / lambda;
        if x >= -2.0 {
            10.0 * (3.0 + x).log10()
        } else {
            0.0
        }
    })
}

/// (2.5.31)/(2.5.32): `−20·lg(1 + (10^(−A_ground/20) − 1)·10^(−(Δdif′ − Δdif)/20))`.
fn ground_split_db(ground_db: f64, mirrored_db: f64, direct_db: f64) -> f64 {
    -20.0 * (1.0 + (10f64.powf(-ground_db / 20.0) - 1.0) * 10f64.powf(-(mirrored_db - direct_db) / 20.0)).log10()
}

/// The boundary term of `path` in `state` over `candidates` (sorted by distance).
pub(super) fn boundary_for_candidates(
    path: &VerticalPath<'_>,
    state: MeteorologicalState,
    candidates: &[PlanePoint],
    lowered: &mut Vec<(f64, f64, usize)>,
) -> StateBoundary {
    let (source, receiver) = (path.source, path.receiver);
    let length = receiver.0;
    let profile = &path.profile;
    let direct = EquivalentGeometry::over(fit_mean_plane(profile, 0.0, length), source, receiver);
    let path_ground = profile.mean_ground_factor(0.0, length);
    let near_source = ground_factor_near_source(&direct, path_ground, path.source_ground_factor);
    let whole_path = ground_attenuation_bands(&direct, source_side_factors(state, path_ground, near_source), state);
    let ray = StateRay::between(state, source, receiver);
    let mut boundary = StateBoundary {
        attenuation_db: whole_path,
        whole_path_ground_db: whole_path,
        ..StateBoundary::default()
    };
    diffraction_path(&ray, source, receiver, candidates, lowered, &mut boundary.path);
    let DiffractionPath { points, blocked } = &boundary.path;
    let (Some(&first), Some(&last)) = (points.first(), points.last()) else {
        return boundary;
    };
    let delta = ray.path_difference(source, points, receiver);
    boundary.path_difference_m = delta;
    let span = if points.len() > 1 {
        points.windows(2).map(|pair| ray.length(pair[0], pair[1])).sum::<f64>()
    } else {
        0.0
    };
    let c_second: [f64; NUM_BANDS] = std::array::from_fn(|band| {
        if points.len() > 1 && span > MULTIPLE_DIFFRACTION_MINIMUM_SPAN_M {
            let x = 5.0 * SPEED_OF_SOUND / BAND_FREQ[band] / span;
            (1.0 + x * x) / (1.0 / 3.0 + x * x)
        } else {
            1.0
        }
    });
    let source_side = EquivalentGeometry::over(fit_mean_plane(profile, 0.0, first.0), source, first);
    let receiver_side = EquivalentGeometry::over(fit_mean_plane(profile, last.0, length), last, receiver);
    let source_image = source_side.plane.mirror(source);
    let receiver_image = receiver_side.plane.mirror(receiver);
    let direct_db = diffraction_db(delta, &c_second);
    let source_image_db = diffraction_db(ray.path_difference(source_image, points, receiver), &c_second);
    let receiver_image_db = diffraction_db(ray.path_difference(source, points, receiver_image), &c_second);
    let source_side_ground = profile.mean_ground_factor(0.0, first.0);
    let source_side_near = ground_factor_near_source(&source_side, source_side_ground, path.source_ground_factor);
    let ground_so = ground_attenuation_bands(
        &source_side,
        source_side_factors(state, source_side_ground, source_side_near),
        state,
    );
    let receiver_side_ground = profile.mean_ground_factor(last.0, length);
    let ground_or = ground_attenuation_bands(
        &receiver_side,
        GroundFactors {
            path: receiver_side_ground,
            impedance: receiver_side_ground,
            floor: receiver_side_ground,
        },
        state,
    );
    // Rayleigh (2021/1226 point (9)(c); ISO/TR 17534-4 §5.9), only on an unblocked path, with S*
    // and R* mirrored in D's own side planes.
    let rayleigh_star = if *blocked {
        0.0
    } else {
        ray.path_difference(source_image, points, receiver_image)
    };
    for band in 0..NUM_BANDS {
        let lambda = SPEED_OF_SOUND / BAND_FREQ[band];
        let admitted = *blocked || (delta > -lambda / 20.0 && delta > lambda / 4.0 - rayleigh_star);
        if !admitted {
            continue;
        }
        let mut sr = direct_db[band];
        let ground_free = sr.clamp(0.0, DIFFRACTION_CAP_DB);
        // A source (receiver) below its side's plane takes its side's A_ground whole and the
        // mirrored Δdif (2021/1226 point (9)(h); ISO/TR 17534-4 §5.3).
        let mut split_so = if source_side.source_side_below_plane {
            sr = source_image_db[band];
            ground_so[band]
        } else {
            ground_split_db(ground_so[band], source_image_db[band], direct_db[band])
        };
        // A non-positive argument in the ground-split logarithm has no finite real value
        // (a below-plane image can make the receiver side's numerator much smaller than the
        // source side's denominator). Follow NoiseModelling's aDif numerical fallback: the
        // affected side takes its whole ground term and its image diffraction, source then
        // receiver. This supplements the below-plane rule; it is not an attenuation cap.
        if !split_so.is_finite() {
            split_so = ground_so[band];
            sr = source_image_db[band];
        }
        let mut split_or = if receiver_side.receiver_side_below_plane {
            sr = receiver_image_db[band];
            ground_or[band]
        } else {
            ground_split_db(ground_or[band], receiver_image_db[band], sr)
        };
        if !split_or.is_finite() {
            split_or = ground_or[band];
            sr = receiver_image_db[band];
        }
        boundary.attenuation_db[band] = sr.clamp(0.0, DIFFRACTION_CAP_DB) + split_so + split_or;
        boundary.without_ground_db[band] = ground_free;
        boundary.diffracted[band] = true;
    }
    boundary
}

/// Table 2.5.b for a path that starts at the source: homogeneous G′/G′, favourable G/G′.
fn source_side_factors(state: MeteorologicalState, path_ground: f64, near_source: f64) -> GroundFactors {
    GroundFactors {
        path: path_ground,
        impedance: match state {
            MeteorologicalState::Homogeneous => near_source,
            MeteorologicalState::Favourable => path_ground,
        },
        floor: near_source,
    }
}

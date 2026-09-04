//! Receiver-local vector-building horizon for low airborne aircraft.

use std::f64::consts::TAU;
use std::sync::OnceLock;

use crate::propagation::obstacle_index::{CrossingScratch, ObstacleSet};
use crate::types::RasterSampler;

use super::doc29::M_PER_DEG_LAT;

/// Local direction width is 1.40625°. At the measured LKPR farthest
/// shadowed source point (178.589 m) that spans 4.39 m, less than one z13
/// receiver pixel.
pub const BUILDING_LOCAL_HORIZON_SECTORS: usize = 256;
pub const BUILDING_LOCAL_HORIZON_BANDS: usize = 6;

pub const BUILDING_LOCAL_MAX_M: f64 = 512.0;
pub const BUILDING_LOCAL_RANGE_GROWTH: f64 = 2.0;
pub const BUILDING_LOCAL_FIRST_RANGE_BREAK_M: f64 = {
    let mut range_m = BUILDING_LOCAL_MAX_M;
    let mut remaining_bands = BUILDING_LOCAL_HORIZON_BANDS - 1;
    while remaining_bands > 0 {
        range_m /= BUILDING_LOCAL_RANGE_GROWTH;
        remaining_bands -= 1;
    }
    range_m
};
pub const BUILDING_LOCAL_RANGE_BREAK_M: [f64; BUILDING_LOCAL_HORIZON_BANDS] = {
    let mut breaks = [0.0; BUILDING_LOCAL_HORIZON_BANDS];
    let mut band = 0;
    let mut range_m = BUILDING_LOCAL_FIRST_RANGE_BREAK_M;
    while band < BUILDING_LOCAL_HORIZON_BANDS {
        breaks[band] = range_m;
        range_m *= BUILDING_LOCAL_RANGE_GROWTH;
        band += 1;
    }
    breaks
};
/// Centimetre roof ranges fit the 16-bit packed field through the full 512 m horizon.
pub const BUILDING_HORIZON_RANGE_SCALE: f64 = 100.0;
/// A crossing at the receiver itself is a footprint-boundary artefact, not a
/// finite diffraction edge. One centimetre matches the packed range quantum.
pub const BUILDING_MIN_EDGE_RANGE_M: f64 = 0.01;
const BUILDING_TANGENT_EMPTY: u16 = u16::MAX;

const _: () = assert!(BUILDING_LOCAL_MAX_M * BUILDING_HORIZON_RANGE_SCALE <= u16::MAX as f64);
const _: () =
    assert!(BUILDING_LOCAL_RANGE_BREAK_M[BUILDING_LOCAL_HORIZON_BANDS - 1] == BUILDING_LOCAL_MAX_M);

pub const BUILDING_LOCAL_HORIZON_ENTRY_COUNT: usize =
    BUILDING_LOCAL_HORIZON_SECTORS * BUILDING_LOCAL_HORIZON_BANDS;

/// ISO 9613-2 §7.4 broadband slope `C2 / wavelength * C3` used by C2.
const DIFFRACTION_SLOPE_PER_M: f64 = 29.2;
/// `10 log10(3)`, removed so insertion loss is continuous from 0 dB at grazing.
const DIFFRACTION_GRAZING_DB: f64 = 4.771_212_547_196_624;
/// FAA AEDT line-of-sight-blockage cap.
pub(super) const DIFFRACTION_CAP_DB: f64 = 18.0;

/// Anchored broadband single-edge insertion loss from path difference.
#[inline]
pub(super) fn single_edge_diffraction_db(path_difference_m: f64) -> f64 {
    let raw = 10.0 * (3.0 + DIFFRACTION_SLOPE_PER_M * path_difference_m.max(0.0)).log10();
    (raw - DIFFRACTION_GRAZING_DB).clamp(0.0, DIFFRACTION_CAP_DB)
}

/// The fixed sector-centre direction lattice used by both CPU and CUDA
/// horizon builders.
pub fn building_local_directions() -> &'static [(f64, f64); BUILDING_LOCAL_HORIZON_SECTORS] {
    static DIRECTIONS: OnceLock<[(f64, f64); BUILDING_LOCAL_HORIZON_SECTORS]> = OnceLock::new();
    DIRECTIONS.get_or_init(sector_directions)
}

fn sector_directions<const SECTORS: usize>() -> [(f64, f64); SECTORS] {
    std::array::from_fn(|sector| {
        let angle = (sector as f64 + 0.5) * TAU / SECTORS as f64;
        angle.sin_cos()
    })
}

/// Range-max roof edges within 512 m of one receiver.
///
/// Fixed sector-centre rays are Quiet Map's explicit approximation. On the
/// staged LKPR loudest-path census, 256 local sectors retained all 23 exact
/// shadows with no false positive among 42,529 clear outdoor receivers; the
/// maximum blocked-ray loss difference was 0.569 dB, and every measured
/// shadowed source point was within 179 m. Every blocking edge is nearer than
/// its source, so the 512 m limit is at least a measured 2.9× range margin for
/// those material paths; farther building shadows are not represented.
/// Every operation after choosing the direction uses exact vector crossings,
/// sampled roof elevation, and two-dimensional single-edge path difference. One
/// neighbourhood scan projects each edge onto the sector-centre rays it
/// intersects, avoiding 256 independent grid walks.
pub struct BuildingHorizon {
    local: [[(u16, u16); BUILDING_LOCAL_HORIZON_BANDS]; BUILDING_LOCAL_HORIZON_SECTORS],
    local_max_tangent_bits: [u16; BUILDING_LOCAL_HORIZON_SECTORS],
}

impl BuildingHorizon {
    /// Build once for an outdoor receiver.
    pub fn build(
        obstacles: &ObstacleSet,
        rasters: &dyn RasterSampler,
        receiver_lat: f64,
        receiver_lon: f64,
        receiver_alt_m: f64,
        crossing_scratch: &mut CrossingScratch,
    ) -> Self {
        let (local, local_max_tangent_bits) = build_sector_bands(
            building_local_directions(),
            BUILDING_LOCAL_MAX_M,
            &BUILDING_LOCAL_RANGE_BREAK_M,
            obstacles,
            rasters,
            receiver_lat,
            receiver_lon,
            receiver_alt_m,
            crossing_scratch,
        );
        Self {
            local,
            local_max_tangent_bits,
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.local_max_tangent_bits
            .iter()
            .all(|&tangent| tangent == BUILDING_TANGENT_EMPTY)
    }

    /// Strongest roof-edge diffraction on the physical receiver-to-subsegment ray.
    #[inline]
    pub fn screening_dz(
        &self,
        source_east_m: f64,
        source_north_m: f64,
        source_rel_alt_m: f64,
    ) -> f64 {
        let lateral_m = source_east_m.hypot(source_north_m);
        if lateral_m <= 1.0 {
            return 0.0;
        }
        let angle = source_north_m.atan2(source_east_m).rem_euclid(TAU);
        let local_sector = ((angle * BUILDING_LOCAL_HORIZON_SECTORS as f64 / TAU) as usize)
            .min(BUILDING_LOCAL_HORIZON_SECTORS - 1);
        screening_from_bands(
            &self.local[local_sector],
            self.local_max_tangent_bits[local_sector],
            lateral_m,
            source_rel_alt_m,
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn build_sector_bands<const SECTORS: usize, const BANDS: usize>(
    directions: &[(f64, f64); SECTORS],
    ray_m: f64,
    breaks_m: &[f64; BANDS],
    obstacles: &ObstacleSet,
    rasters: &dyn RasterSampler,
    receiver_lat: f64,
    receiver_lon: f64,
    receiver_alt_m: f64,
    crossing_scratch: &mut CrossingScratch,
) -> ([[(u16, u16); BANDS]; SECTORS], [u16; SECTORS]) {
    let m_per_deg_lon = M_PER_DEG_LAT * receiver_lat.to_radians().cos().max(0.2);
    let mut best = [[(f64::NEG_INFINITY, 0.0_f64); BANDS]; SECTORS];
    obstacles.visit_building_sector_crossings(
        receiver_lat,
        receiver_lon,
        M_PER_DEG_LAT,
        m_per_deg_lon,
        ray_m,
        directions,
        crossing_scratch,
        &mut |sector, range_m, height_m| {
            if range_m <= BUILDING_MIN_EDGE_RANGE_M {
                return;
            }
            let (sin_angle, cos_angle) = directions[sector];
            let edge_lat = receiver_lat + sin_angle * range_m / M_PER_DEG_LAT;
            let edge_lon = receiver_lon + cos_angle * range_m / m_per_deg_lon;
            let edge_rel_alt_m =
                rasters.elevation(edge_lat, edge_lon) + f64::from(height_m) - receiver_alt_m;
            let tangent = edge_rel_alt_m / range_m;
            let band = breaks_m
                .iter()
                .position(|&break_m| range_m <= break_m)
                .unwrap_or(BANDS - 1);
            if tangent > best[sector][band].0 {
                best[sector][band] = (tangent, range_m);
            }
        },
    );

    pack_sector_bands(&best)
}

fn pack_sector_bands<const SECTORS: usize, const BANDS: usize>(
    best: &[[(f64, f64); BANDS]; SECTORS],
) -> ([[(u16, u16); BANDS]; SECTORS], [u16; SECTORS]) {
    let mut sectors = [[(BUILDING_TANGENT_EMPTY, 0_u16); BANDS]; SECTORS];
    let mut max_tangent_bits = [BUILDING_TANGENT_EMPTY; SECTORS];
    for sector in 0..SECTORS {
        for (band, &(tangent, range_m)) in best[sector].iter().enumerate() {
            if range_m == 0.0 {
                continue;
            }
            let tangent_bits = encode_building_tangent_floor(tangent);
            sectors[sector][band] = (
                tangent_bits,
                (range_m * BUILDING_HORIZON_RANGE_SCALE)
                    .ceil()
                    .min(u16::MAX as f64) as u16,
            );
        }
        if let Some((tangent, _)) = best[sector].iter().max_by(|a, b| a.0.total_cmp(&b.0)) {
            if tangent.is_finite() {
                max_tangent_bits[sector] = encode_building_tangent_floor(*tangent);
            }
        }
    }
    (sectors, max_tangent_bits)
}

/// The upper half of an IEEE-754 `f32` is bfloat16: it retains the full
/// dynamic range needed for a receiver centimetres from a façade while using
/// the same 16-bit field as the terrain fixed point. Quantize tangent toward
/// negative infinity and range away from the receiver so packing can only
/// reduce a real shadow, never create one by moving an edge in front of the
/// source.
#[inline]
fn encode_building_tangent_floor(tangent: f64) -> u16 {
    debug_assert!(tangent.is_finite());
    let tangent_f32 = tangent.clamp(-(f32::MAX as f64), f32::MAX as f64) as f32;
    let mut bits = (tangent_f32.to_bits() >> 16) as u16;
    if decode_building_tangent(bits) > tangent {
        bits = if tangent_f32.is_sign_negative() {
            bits.saturating_add(1)
        } else {
            bits.saturating_sub(1)
        };
    }
    debug_assert_ne!(bits, BUILDING_TANGENT_EMPTY);
    bits
}

#[inline]
fn decode_building_tangent(bits: u16) -> f64 {
    f64::from(f32::from_bits(u32::from(bits) << 16))
}

#[inline]
fn screening_from_bands<const BANDS: usize>(
    bands: &[(u16, u16); BANDS],
    max_tangent_bits: u16,
    lateral_m: f64,
    source_rel_alt_m: f64,
) -> f64 {
    if max_tangent_bits == BUILDING_TANGENT_EMPTY {
        return 0.0;
    }
    let source_tangent = source_rel_alt_m / lateral_m;
    if source_tangent >= decode_building_tangent(max_tangent_bits) {
        return 0.0;
    }
    let direct_m = lateral_m.hypot(source_rel_alt_m);
    let mut best_db = 0.0_f64;
    for &(tangent_bits, range_q) in bands {
        if range_q == 0 {
            continue;
        }
        let packed_range_m = f64::from(range_q) / BUILDING_HORIZON_RANGE_SCALE;
        if packed_range_m >= lateral_m {
            continue;
        }
        let tangent = decode_building_tangent(tangent_bits);
        if tangent <= source_tangent {
            continue;
        }
        // The old nearest-centimetre representation could have been either end
        // of this one-quantum interval, so use the smaller diffraction of both
        // endpoints. It is no greater than the old value whichever endpoint
        // nearest rounding selected, while ceiling remains conservative for
        // before-source eligibility. This mirrors the terrain query and GPU.
        let edge_db = [(range_q - 1) as f64, f64::from(range_q)]
            .into_iter()
            .map(|range_q| {
                let range_m = range_q / BUILDING_HORIZON_RANGE_SCALE;
                let edge_rel_alt_m = tangent * range_m;
                let receiver_to_edge_m = range_m.hypot(edge_rel_alt_m);
                let source_to_edge_m =
                    (lateral_m - range_m).hypot(source_rel_alt_m - edge_rel_alt_m);
                let delta = (receiver_to_edge_m + source_to_edge_m - direct_m).max(0.0);
                single_edge_diffraction_db(delta)
            })
            .fold(f64::INFINITY, f64::min);
        best_db = best_db.max(edge_db);
    }
    best_db
}

#[cfg(test)]
mod tests;

//! Flat-earth geometry for propagation and popup math: distances, segment feet,
//! audibility gates, finite-line corrections, reach boxes.
//!
//! The single home of this math — `noise-compute::propagation::geo` and
//! `square-store::geo` re-export it.

/// Metres per degree latitude (WGS-84 mean).
pub const M_PER_DEG_LAT: f64 = 110_540.0;
/// Metres per degree longitude at the equator.
pub const M_PER_DEG_LON_EQ: f64 = 111_320.0;

/// Fold a longitude into the one canonical interval used by stored points.
#[inline]
pub fn normalize_longitude(lon_deg: f64) -> f64 {
    if (-180.0..180.0).contains(&lon_deg) {
        lon_deg
    } else {
        (lon_deg + 180.0).rem_euclid(360.0) - 180.0
    }
}

/// Signed shortest longitude delta from `from_deg` to `to_deg`.
#[inline]
pub fn wrapped_longitude_delta(from_deg: f64, to_deg: f64) -> f64 {
    normalize_longitude(to_deg - from_deg)
}

/// Midpoint longitude on the short arc, including across the antimeridian.
#[inline]
pub fn wrapped_longitude_midpoint(a_deg: f64, b_deg: f64) -> f64 {
    interpolate_longitude_short_arc(a_deg, b_deg, 0.5)
}

/// Canonical longitude at fraction `t` along the short source–receiver arc.
#[inline]
pub fn interpolate_longitude_short_arc(from_deg: f64, to_deg: f64, t: f64) -> f64 {
    normalize_longitude(from_deg + t * wrapped_longitude_delta(from_deg, to_deg))
}

/// Metres per degree longitude at `lat_rad` radians.
pub fn m_per_deg_lon(lat_rad: f64) -> f64 {
    M_PER_DEG_LON_EQ * lat_rad.cos().max(0.01)
}

/// Flat-earth distance in meters (accurate <0.3% at <50km).
pub fn flat_dist(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let mid_lat = ((lat1 + lat2) / 2.0).to_radians();
    let dx = wrapped_longitude_delta(lon1, lon2) * m_per_deg_lon(mid_lat);
    let dy = (lat2 - lat1) * M_PER_DEG_LAT;
    (dx * dx + dy * dy).sqrt()
}

/// 3D slant distance: horizontal distance + height difference.
/// Used for geometric divergence and atmospheric absorption.
pub fn slant_dist(d_horizontal: f64, source_alt: f64, receiver_alt: f64) -> f64 {
    let dz = source_alt - receiver_alt;
    (d_horizontal * d_horizontal + dz * dz).sqrt()
}

/// Effective propagation distance for discretized area sources.
/// Clamps distance to exclusion_radius (= equivalent area-patch radius)
/// to prevent 1/r² singularity when receiver is inside the source polygon.
/// For pure point sources (exclusion_radius_m = 0), returns dist_m unchanged.
#[inline]
pub fn effective_area_source_dist(dist_m: f64, exclusion_radius_m: f64) -> f64 {
    if exclusion_radius_m > 0.0 {
        dist_m.max(exclusion_radius_m)
    } else {
        dist_m
    }
}

/// Result of closest-point-on-segment computation.
#[derive(Debug, Clone, Copy)]
pub struct ClosestPoint {
    pub lat: f64,
    pub lon: f64,
    pub dist_m: f64,
    pub fraction: f64, // 0.0 = at start, 1.0 = at end
}

/// Three-semantic decomposition for line-source receiver math.
///
/// `point_to_segment` returns a fused (distance, foot, fraction) tuple
/// that collapses three physically different quantities into one; for
/// CNOSSOS-EU line-source receivers off the segment endpoints (e.g.
/// past a runway threshold) this loses signed information needed by
/// the finite-line angle integral. [`point_to_segment_full`] returns
/// each as a dedicated field:
///
/// - `d_perp_m`: perpendicular to the EXTENDED line (unclamped foot).
///   This is the `d` in the CNOSSOS line-source receiver formula
///   `recv = LW' + 10·log10(θ / d)`.
/// - `d_endpoint_m`: Euclidean to the nearest endpoint (clamped foot
///   distance). Conservative prune metric — a receiver close to the
///   line but far past its end has small `d_perp` yet large
///   `d_endpoint`.
/// - `cp_lat` / `cp_lon`: foot CLAMPED to segment for path sampling
///   (terrain / screening). Unclamped foot would land off-segment and
///   sample the wrong source location.
/// - `fraction`: along-line position, UNCLAMPED + signed (can be < 0
///   or > 1). The FLC angle integral relies on signed `d1 = f·L` and
///   `d2 = (1-f)·L` so off-segment receivers get the right subtended
///   angle via `atan(d1/d_perp) − atan(−d2/d_perp)`.
#[derive(Debug, Clone, Copy)]
pub struct PointToSegment {
    pub d_perp_m: f64,
    pub d_endpoint_m: f64,
    pub cp_lat: f64,
    pub cp_lon: f64,
    pub fraction: f64,
}

/// Horizontal distance from a point to a line segment.
/// Returns (distance_m, closest_point_lat, closest_point_lon, fraction 0-1).
///
/// Distance and fraction are clamped (segment, not extended line) —
/// road / rail / barrier callers want the on-segment foot. Aircraft
/// ground-ops receivers need the unclamped signed-fraction variant
/// from [`point_to_segment_full`].
pub fn point_to_segment(
    p_lat: f64,
    p_lon: f64,
    a_lat: f64,
    a_lon: f64,
    b_lat: f64,
    b_lon: f64,
) -> (f64, f64, f64, f64) {
    let pts = point_to_segment_full(p_lat, p_lon, a_lat, a_lon, b_lat, b_lon);
    (
        pts.d_endpoint_m,
        pts.cp_lat,
        pts.cp_lon,
        pts.fraction.clamp(0.0, 1.0),
    )
}

/// Full three-semantic decomposition. See [`PointToSegment`] docstring.
pub fn point_to_segment_full(
    p_lat: f64,
    p_lon: f64,
    a_lat: f64,
    a_lon: f64,
    b_lat: f64,
    b_lon: f64,
) -> PointToSegment {
    let mid_lat = ((a_lat + b_lat) / 2.0).to_radians();
    let m_lon = m_per_deg_lon(mid_lat);

    // Project to local meters (A at origin).
    let segment_lon_delta = wrapped_longitude_delta(a_lon, b_lon);
    let bx = segment_lon_delta * m_lon;
    let by = (b_lat - a_lat) * M_PER_DEG_LAT;
    let px = wrapped_longitude_delta(a_lon, p_lon) * m_lon;
    let py = (p_lat - a_lat) * M_PER_DEG_LAT;

    let ab_len_sq = bx * bx + by * by;
    let t_unclamped = if ab_len_sq < 1e-10 {
        0.0
    } else {
        (px * bx + py * by) / ab_len_sq
    };
    let t_clamped = t_unclamped.clamp(0.0, 1.0);

    // d_perp: unclamped foot → distance to extended line.
    let foot_x = t_unclamped * bx;
    let foot_y = t_unclamped * by;
    let d_perp_m = ((px - foot_x).powi(2) + (py - foot_y).powi(2)).sqrt();

    // d_endpoint: clamped foot → Euclidean to nearest segment point.
    let cp_x = t_clamped * bx;
    let cp_y = t_clamped * by;
    let d_endpoint_m = ((px - cp_x).powi(2) + (py - cp_y).powi(2)).sqrt();

    let cp_lat = a_lat + t_clamped * (b_lat - a_lat);
    let cp_lon = normalize_longitude(a_lon + t_clamped * segment_lon_delta);

    PointToSegment {
        d_perp_m,
        d_endpoint_m,
        cp_lat,
        cp_lon,
        fraction: t_unclamped,
    }
}

/// Find closest point on a line segment to a given point (struct return variant).
pub fn closest_point_on_segment(
    p_lat: f64,
    p_lon: f64,
    a_lat: f64,
    a_lon: f64,
    b_lat: f64,
    b_lon: f64,
) -> ClosestPoint {
    let (dist_m, lat, lon, fraction) = point_to_segment(p_lat, p_lon, a_lat, a_lon, b_lat, b_lon);
    ClosestPoint {
        lat,
        lon,
        dist_m,
        fraction,
    }
}

/// A-weighted conservative atmospheric absorption coefficient (dB/m).
/// ISO 9613-1 at 15 °C / 70 % RH, dB/km per octave band: 63 Hz ≈ 0.1,
/// 500 Hz ≈ 1.7, 1 kHz ≈ 5, 2 kHz ≈ 10. Road and rail A-weighted energy
/// is dominated by 500 Hz – 2 kHz, giving an effective A-weighted α of
/// roughly 3 – 5 dB/km. 2 dB/km (= 0.002 dB/m) leaves ≥ 1 dB safety
/// margin so the cutoff never discards a contribution that would reach
/// the threshold in reality.
pub const ATM_ALPHA_A_WEIGHTED: f64 = 0.002;

/// Conservative reach for a point source whose loudest day band is
/// `max_emission_db`, bounded by the layer's physical `cap_m`.
///
/// This inverts only geometric divergence, deliberately omitting atmospheric
/// absorption: at the uncapped result [`below_free_field_threshold`] is already
/// negative by `ATM_ALPHA_A_WEIGHTED * radius`, and decreases monotonically at
/// every greater distance. A caller may therefore use this as an enumeration
/// bound without dropping any pair that the exact free-field gate would keep.
#[inline]
pub fn point_source_audibility_radius(max_emission_db: f64, cap_m: f64) -> f64 {
    10f64.powf((max_emission_db - 11.0) / 20.0).min(cap_m)
}

/// Check if a point source is too weak to contribute at this distance.
/// Geometric divergence ~20*log10(d) + 11 dB plus conservative A-weighted
/// atmospheric absorption. Path effects (ground, screening, vegetation)
/// only attenuate further, so if this bound is already below threshold
/// the ray-cast can be skipped without loss.
#[inline]
pub fn below_free_field_threshold(max_emission_db: f64, dist_m: f64, threshold_db: f64) -> bool {
    let geo_approx = 20.0 * dist_m.log10() + 11.0;
    let atm_approx = ATM_ALPHA_A_WEIGHTED * dist_m;
    max_emission_db - geo_approx - atm_approx < threshold_db
}

/// Check if a line source is too weak to contribute at this distance.
/// Cylindrical divergence L_r = L_W - 10*log10(d) - 8 plus conservative
/// A-weighted atmospheric absorption. Tighter than the point-source bound.
#[inline]
pub fn below_free_field_threshold_line(
    max_emission_db: f64,
    dist_m: f64,
    threshold_db: f64,
) -> bool {
    let geo_approx = 10.0 * dist_m.log10() + 8.0;
    let atm_approx = ATM_ALPHA_A_WEIGHTED * dist_m;
    max_emission_db - geo_approx - atm_approx < threshold_db
}

/// Perpendicular distance floor (m) for the finite-line geometry.
///
/// A receiver sitting on a segment's EXTENDED line has `d_perp = 0`, where the
/// exact line integral `θ/d_perp` is 0/0. Clamping to half a metre evaluates
/// the segment as a line half a metre off the receiver — exact for a line at
/// that distance (`θ(d)/d = ∫dy/(d²+y²)` holds for any `d`), and the same kind
/// of regularisation the divergence term applies with `d.max(1.0)`.
pub const FLC_MIN_PERP_M: f64 = 0.5;

/// Finite-line correction paired with the DIVERGENCE distance the propagation
/// kernel is fed — the form line callers (road, rail) must use.
///
/// The kernel's line chain is `−10·log10(2π·d_div) + FLC`, while the exact
/// free-field energy of a straight finite line is `∝ θ/d_perp`
/// (`∫dy/(d_perp² + y²)`): θ the angle the segment subtends at the receiver,
/// `d_perp` the perpendicular distance to its INFINITE line. The correction
/// that makes that pair exact is therefore
///
/// ```text
/// FLC = 10·log10(θ/π) + 10·log10(d_div / d_perp)
/// ```
///
/// and collapses to plain [`finite_line_correction`] whenever the receiver's
/// perpendicular foot lies ON the segment (`d_div == d_perp` — the common
/// case, and the only geometry the plain form is valid for). Passing the
/// ENDPOINT distance as if it were `d_perp` is what made segments the receiver
/// sits PAST read loud: a 250 m segment starting 250 m up the road from the
/// perpendicular foot came out +1.9 dB, ≈ +0.9 dB on the whole line
/// (screening fix-pack C, 2026-08-03).
///
/// `signed_fraction` is [`PointToSegment::fraction`] UNCLAMPED: past an
/// endpoint `d1 = f·L` goes negative and the subtended angle becomes the
/// DIFFERENCE of the two end angles instead of their sum — the clamped
/// fraction would instead claim the whole segment sits on one side of the
/// perpendicular foot (measured +2.7 dB on the fixture's scene B).
pub fn finite_line_correction_for_divergence(
    seg_length_m: f64,
    d_perp_m: f64,
    signed_fraction: f64,
    d_divergence_m: f64,
) -> f64 {
    if seg_length_m < 0.1 {
        return 0.0;
    }
    let d_perp = d_perp_m.max(FLC_MIN_PERP_M);
    // `.max(d_perp)`: the endpoint distance is ≥ the perpendicular one by
    // construction, except when the floor lifted `d_perp` above it.
    finite_line_correction(seg_length_m, d_perp, signed_fraction)
        + 4.342944819032518_f64 * (d_divergence_m.max(d_perp) / d_perp).ln()
}

/// Finite-line correction using HORIZONTAL distance and end angles, for a
/// receiver whose perpendicular foot lies ON the segment.
///
/// ISO 9613-2: correction for finite line source vs infinite.
/// Uses HORIZONTAL distances (not 3D slant — fix from V33/V44).
///
/// Returns correction in dB (always ≤ 0). Line callers whose receiver may sit
/// past an endpoint want [`finite_line_correction_for_divergence`].
pub fn finite_line_correction(
    seg_length_m: f64,
    d_perp_horizontal: f64,
    fraction: f64, // 0-1 position of closest point along segment
) -> f64 {
    if seg_length_m < 0.1 || d_perp_horizontal < 0.1 {
        return 0.0;
    }

    // Distances from closest point to segment endpoints (along segment)
    let d1 = fraction * seg_length_m;
    let d2 = (1.0 - fraction) * seg_length_m;
    let inv_d = 1.0 / d_perp_horizontal;
    let a1 = d1 * inv_d;
    let a2 = d2 * inv_d;

    // Angle subtended by segment as seen from receiver.
    // Use atan addition formula: atan(a1) + atan(a2) = atan((a1+a2)/(1-a1*a2)) + k*π
    // For positive a1,a2: if a1*a2 < 1, k=0; if a1*a2 >= 1, k=1 (theta > π/2).
    let prod = a1 * a2;
    let theta = if prod < 0.98 {
        // Single atan instead of two
        ((a1 + a2) / (1.0 - prod)).atan()
    } else {
        // a1*a2 >= 1: denominator near zero or negative, use two atans (rare case)
        a1.atan() + a2.atan()
    };

    // Correction: ratio of subtended angle to π (full infinite line)
    // Use ln for speed: 10*log10(x) = (10/ln10)*ln(x)
    let correction = 4.342944819032518_f64 * (theta / std::f64::consts::PI).ln();

    correction.min(0.0)
}

/// Half-extents in DEGREES of the bounding box that must cover a source's reach
/// disk of `reach_m`, given the widest ABSOLUTE latitude the disk spans.
///
/// The one place this geometry lives. It exists because the same expression was
/// written twice, fixed once, and left broken in the sibling: `ground_ops` used
/// the poleward edge and a `0.01` cosine floor, while `scatter_point` used the
/// SOURCE latitude and a `0.2` floor — which under-covers longitude by up to
/// 2.3x above 78.46 deg and silently drops audible industrial / building /
/// leisure sources at Ny-Alesund, Station Nord and Alert.
///
/// Two rules, both load-bearing, both violated by the old point version:
///  * take `cos` at the POLEWARD EDGE (`widest_abs_lat_deg + reach_lat_deg`), not
///    at the source — the box has to cover its own poleward corner, where a
///    degree of longitude is shortest;
///  * clamp `cos` at `0.01`, matching [`m_per_deg_lon`], the
///    function the EXACT per-pixel distance gate uses. A looser clamp makes the
///    box smaller than the disk that gate accepts, so the box — not the physics —
///    decides audibility.
///
/// Conservative by construction: the returned box always CONTAINS the reach disk,
/// so the exact `flat_dist` gate downstream stays the only thing that culls.
#[inline]
pub fn reach_box_half_extents_deg(widest_abs_lat_deg: f64, reach_m: f64) -> (f64, f64) {
    let reach_lat_deg = reach_m / M_PER_DEG_LAT;
    let poleward_lat = widest_abs_lat_deg.abs() + reach_lat_deg;
    let reach_lon_deg = reach_m / m_per_deg_lon(poleward_lat.to_radians());
    (reach_lat_deg, reach_lon_deg)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The box must CONTAIN the reach disk at every latitude a source can sit at,
    /// including both hemispheres of the 79-83 deg inhabited polar band where
    /// the retired `cos().max(0.2)` clamp began under-covering.
    #[test]
    fn reach_box_contains_the_disk_including_above_the_78_5_clamp() {
        for &reach_m in &[281.84_f64, 2_000.0, 4_000.0] {
            for abs_lat in [
                0.0_f64, 50.0, 70.0, 78.0, 79.0, 80.0, 81.0, 81.6, 82.0, 82.5, 83.0, 85.0,
            ] {
                for lat in [abs_lat, -abs_lat] {
                    let (lat_deg, lon_deg) = reach_box_half_extents_deg(lat, reach_m);
                    for north_step in -8..=8 {
                        let north_m = reach_m * f64::from(north_step) / 8.0;
                        let east_m = (reach_m * reach_m - north_m * north_m).max(0.0).sqrt();
                        let receiver_lat = lat + north_m / M_PER_DEG_LAT;
                        let mid_lat = (lat + receiver_lat) * 0.5;
                        let receiver_lon = east_m / m_per_deg_lon(mid_lat.to_radians());
                        assert!((receiver_lat - lat).abs() <= lat_deg + 1e-12);
                        assert!(
                            receiver_lon <= lon_deg + 1e-12,
                            "lat={lat} reach={reach_m}: half-width {lon_deg} < disk point {receiver_lon}"
                        );
                    }
                }
            }
        }
    }

    /// The specific regression: at 80 deg with a 281.84 m reach (an office-class
    /// industrial grid point, Lw ~= 60 dB), the retired form yields a box narrower
    /// than the disk, so a receiver ~257 m east — one the free-field gate keeps —
    /// was never enumerated.
    #[test]
    fn the_retired_0_2_clamp_really_did_under_cover() {
        let (lat_deg, lon_deg) = reach_box_half_extents_deg(80.0, 281.84);
        let retired = 281.84 / (111_320.0 * 80.0_f64.to_radians().cos().max(0.2));
        assert!(
            lon_deg > retired,
            "fixed {lon_deg} must exceed retired {retired}"
        );
        let receiver_east_deg = 257.10 / m_per_deg_lon((80.0 + lat_deg).to_radians());
        assert!(
            lon_deg >= receiver_east_deg,
            "{lon_deg} < {receiver_east_deg}"
        );
        assert!(
            retired < receiver_east_deg,
            "retired form must have missed it"
        );
    }

    #[test]
    fn test_flat_dist() {
        let d = flat_dist(50.08, 14.42, 50.08, 14.434);
        assert!((d - 1000.0).abs() < 50.0, "d={d}");
    }

    #[test]
    fn antimeridian_geometry_uses_the_short_arc() {
        for t in (0..=10).map(|step| f64::from(step) / 10.0) {
            let ordinary = 14.9 + t * (15.1 - 14.9);
            assert_eq!(
                interpolate_longitude_short_arc(14.9, 15.1, t).to_bits(),
                ordinary.to_bits()
            );
        }
        let distance = flat_dist(0.0, 179.999, 0.0, -179.999);
        assert!((distance - 222.64).abs() < 0.5, "distance={distance}");
        assert_eq!(wrapped_longitude_midpoint(179.0, -179.0), -180.0);

        let point = point_to_segment_full(0.001, -180.0, 0.0, 179.999, 0.0, -179.999);
        assert!(
            (point.fraction - 0.5).abs() < 1e-9,
            "fraction={}",
            point.fraction
        );
        assert!(
            wrapped_longitude_delta(point.cp_lon, -180.0).abs() < 1e-9,
            "lon={}",
            point.cp_lon
        );
        assert!((point.d_endpoint_m - M_PER_DEG_LAT * 0.001).abs() < 0.1);
    }

    #[test]
    fn test_closest_point_projection_and_clamp() {
        let mid = closest_point_on_segment(0.005, 0.005, 0.0, 0.0, 0.01, 0.0);
        assert!((mid.fraction - 0.5).abs() < 1e-6, "f={}", mid.fraction);
        assert!((mid.dist_m - 556.6).abs() < 15.0, "d={}", mid.dist_m);
        let past = closest_point_on_segment(0.02, 0.005, 0.0, 0.0, 0.01, 0.0);
        assert_eq!(past.fraction, 1.0);
        assert!((past.lat - 0.01).abs() < 1e-12);
    }

    #[test]
    fn test_slant_dist() {
        let s = slant_dist(100.0, 10.0, 1.5);
        // √(100² + 8.5²) ≈ 100.36
        assert!((s - 100.36).abs() < 0.1, "s={s}");
    }

    #[test]
    fn point_source_audibility_radius_never_preempts_the_free_field_gate() {
        for max_emission_db in (10..=120).map(f64::from) {
            let uncapped = point_source_audibility_radius(max_emission_db, f64::INFINITY);
            assert!(
                below_free_field_threshold(max_emission_db, uncapped, 0.0),
                "free-field gate still keeps {max_emission_db} dB at {uncapped} m"
            );
            assert!(below_free_field_threshold(
                max_emission_db,
                uncapped.next_up(),
                0.0
            ));
        }
        assert_eq!(point_source_audibility_radius(120.0, 4_000.0), 4_000.0);
    }

    #[test]
    fn test_finite_line_midpoint() {
        // Receiver perpendicular to midpoint of 200m segment, 100m away
        let flc = finite_line_correction(200.0, 100.0, 0.5);
        // θ = 2 × atan(100/100) = 2 × π/4 = π/2
        // FLC = 10 × log₁₀(0.5/π) ≈ 10 × log₁₀(0.5) ≈ -3.0 dB
        assert!((flc - (-3.01)).abs() < 0.1, "flc={flc}");
    }

    #[test]
    fn test_finite_line_near_endpoint() {
        // Receiver near one endpoint — more correction
        let flc = finite_line_correction(200.0, 100.0, 0.05);
        // θ ≈ atan(10/100) + atan(190/100) ≈ 0.1 + 1.08 ≈ 1.18
        // FLC ≈ 10 × log₁₀(1.18/π) ≈ -4.3 dB
        assert!(flc < -3.5 && flc > -5.0, "flc={flc}");
    }

    /// Splitting a microsegment into two halves must conserve the
    /// receiver-side energy. If FLC delta scales with the wrong
    /// length or the wrong fraction, this test catches it: the
    /// linear-energy sum of two 100 m halves should match the
    /// linear energy of the parent 200 m segment.
    ///
    /// Math:
    ///   parent 200 m, perpendicular d=500 m, midpoint receiver:
    ///     θ_parent = 2·atan(100/500) = 0.395 rad
    ///   each half 100 m, the receiver's perpendicular foot is at
    ///   the END of half-1 and the START of half-2:
    ///     θ_each = atan(0/500) + atan(100/500) = 0.197 rad
    ///   sum_of_halves_θ = 2 × 0.197 = 0.395 rad = θ_parent ✓
    ///
    /// The energy is proportional to θ at a given d, so doubling
    /// the halves equals the parent — provided FLC uses the actual
    /// fraction (not a hardcoded 0.5).
    /// Received energy of one straight element under the engine's line chain
    /// (`−10·log10(2π·d_div) + FLC`), in arbitrary but comparable units.
    fn line_element_energy(seg_len_m: f64, d_perp_m: f64, y0: f64, y1: f64) -> f64 {
        let fraction = (0.0 - y0) / (y1 - y0); // receiver's foot at y = 0, signed
        let nearest = if y0 <= 0.0 && y1 >= 0.0 {
            0.0
        } else {
            y0.abs().min(y1.abs())
        };
        let d_div = (d_perp_m * d_perp_m + nearest * nearest).sqrt();
        let flc = finite_line_correction_for_divergence(seg_len_m, d_perp_m, fraction, d_div);
        10f64.powf(flc / 10.0) / d_div
    }

    /// With the foot ON the segment the divergence distance IS the
    /// perpendicular one, so the paired form must reduce to the plain one.
    #[test]
    fn flc_for_divergence_reduces_to_plain_on_perpendicular_foot() {
        let plain = finite_line_correction(200.0, 100.0, 0.35);
        let paired = finite_line_correction_for_divergence(200.0, 100.0, 0.35, 100.0);
        assert!((plain - paired).abs() < 1e-12, "{plain} vs {paired}");
    }

    /// THE invariant the paired form buys (screening fix-pack C): splitting a
    /// segment the receiver sits PAST conserves received energy exactly, so a
    /// 250 m microsegment and its 33 sub-elements agree. The endpoint-distance
    /// form does not — it reads this segment +1.9 dB.
    #[test]
    fn off_end_split_conserves_energy() {
        let (d_perp, y0, y1) = (45.0_f64, 250.0_f64, 500.0_f64);
        let parent = line_element_energy(y1 - y0, d_perp, y0, y1);
        let n = 33;
        let sub = (y1 - y0) / n as f64;
        let split: f64 = (0..n)
            .map(|k| {
                let a = y0 + k as f64 * sub;
                line_element_energy(sub, d_perp, a, a + sub)
            })
            .sum();
        assert!(
            (parent / split - 1.0).abs() < 1e-9,
            "parent {parent:e} != split {split:e}"
        );
        // The old form applied to the same segment: endpoint distance in place
        // of the perpendicular one, fraction clamped to the near end.
        let d_end = (d_perp * d_perp + y0 * y0).sqrt();
        let legacy = 10f64.powf(finite_line_correction(y1 - y0, d_end, 0.0) / 10.0) / d_end;
        assert!(
            (10.0 * (legacy / parent).log10() - 1.94).abs() < 0.05,
            "legacy excess {:.2} dB",
            10.0 * (legacy / parent).log10()
        );
    }

    #[test]
    fn test_split_microsegment_preserves_subtended_angle() {
        let d: f64 = 500.0;
        let l_parent: f64 = 200.0;
        // Parent at fraction=0.5
        let theta_parent = 2.0 * (l_parent / (2.0 * d)).atan();
        // Each half at the SHARED endpoint sees fraction = 1.0 (left
        // half) or fraction = 0.0 (right half), with closest point
        // at the boundary between halves.
        let l_half: f64 = 100.0;
        let theta_left_half = (l_half / d).atan(); // d1=L, d2=0
        let theta_right_half = (l_half / d).atan(); // d1=0, d2=L
        let theta_halves_sum = theta_left_half + theta_right_half;
        assert!(
            (theta_halves_sum - theta_parent).abs() < 1e-6,
            "split halves angle {theta_halves_sum} != parent angle {theta_parent}"
        );
    }
}

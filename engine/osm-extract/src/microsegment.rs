//! Split linestrings into microsegments of max_length_m, **merging
//! consecutive near-collinear vertex pairs** so dense OSM polylines
//! (taxiway segmented per ramp boundary, etc.) emit one microsegment
//! per straight run instead of one per OSM vertex.
//!
//! Algorithm: cumulative-length walker (per Plan v2). From vertex `i`,
//! greedy-extend the chord endpoint `j` while
//!   - Σ length [i..j] ≤ `max_length_m` (hard cap, default 250 m)
//!   - every intermediate vertex `k ∈ (i, j)` has perpendicular
//!     distance ≤ `CHORD_EPS_M` (1.0 m) to the chord (i, j)
//!
//! On chord-tolerance violation, emit `(i, j-1)` and restart walker
//! from `j-1`. When a single vertex pair already exceeds `max_length`,
//! fall back to the legacy uniform interpolation for that pair.
//!
//! Acoustic invariance: aircraft / road / rail kernels apply
//! `+ 10·log10(θ / d_perp)` over the actual `length_m`, so merging two
//! collinear sub-segments into one is energy-conservative by Chasles
//! (`Σ θᵢ = θ_total`). Row count typically drops 30-50 % on dense
//! airport polylines (LKPR taxiway median was 5.3 m before merging).

/// Flat-earth distance in meters (accurate to <0.3% at <50km).
/// Handles antimeridian wrapping: lon 179.9→-179.9 = 0.2°, not 359.8°.
fn flat_dist(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let mid_lat = (lat1 + lat2) / 2.0;
    let cos_lat = mid_lat.to_radians().cos();
    let mut dlon = lon2 - lon1;
    if dlon > 180.0 {
        dlon -= 360.0;
    }
    if dlon < -180.0 {
        dlon += 360.0;
    }
    let dx = dlon * 111_320.0 * cos_lat;
    let dy = (lat2 - lat1) * 110_540.0;
    (dx * dx + dy * dy).sqrt()
}

/// Flat-earth bearing in degrees (0..360), 0 = North, 90 = East.
/// Scales longitude by cos(mid_lat) so LKPR's 60° runway reads 60°,
/// not 70° (raw `atan2(Δlon, Δlat)` overshoot at non-equator latitudes).
/// Antimeridian wrap consistent with `flat_dist`.
pub fn bearing_deg(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f32 {
    let mid_lat = (lat1 + lat2) / 2.0;
    let cos_lat = mid_lat.to_radians().cos();
    let mut dlon = lon2 - lon1;
    if dlon > 180.0 {
        dlon -= 360.0;
    }
    if dlon < -180.0 {
        dlon += 360.0;
    }
    let dx = dlon * cos_lat;
    let dy = lat2 - lat1;
    let bearing = dx.atan2(dy).to_degrees();
    let normalised = if bearing < 0.0 {
        bearing + 360.0
    } else {
        bearing
    };
    normalised as f32
}

/// Maximum perpendicular distance an intermediate vertex may stray from
/// the chord before the walker emits and restarts. 1 m matches OSM
/// surveyor digitisation noise and is well below the current base heatmap
/// pixel size (~19.1 m at the equator, ~12.3 m at 50° N).
const CHORD_EPS_M: f64 = 1.0;

/// Perpendicular distance from `p` to the chord `(a, b)` in metres.
/// Antimeridian-safe via the same `cos(mid_lat)` scaling used by
/// [`flat_dist`].
fn perp_distance_to_chord(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let mid_lat = (a[0] + b[0]) / 2.0;
    let cos_lat = mid_lat.to_radians().cos();
    let m_lon = 111_320.0 * cos_lat;
    let m_lat = 110_540.0;
    let mut bdlon = b[1] - a[1];
    if bdlon > 180.0 {
        bdlon -= 360.0;
    }
    if bdlon < -180.0 {
        bdlon += 360.0;
    }
    let mut pdlon = p[1] - a[1];
    if pdlon > 180.0 {
        pdlon -= 360.0;
    }
    if pdlon < -180.0 {
        pdlon += 360.0;
    }
    let bx = bdlon * m_lon;
    let by = (b[0] - a[0]) * m_lat;
    let px = pdlon * m_lon;
    let py = (p[0] - a[0]) * m_lat;
    let ab_len_sq = bx * bx + by * by;
    if ab_len_sq < 1e-9 {
        return (px * px + py * py).sqrt();
    }
    let t = (px * bx + py * by) / ab_len_sq;
    let foot_x = t * bx;
    let foot_y = t * by;
    ((px - foot_x).powi(2) + (py - foot_y).powi(2)).sqrt()
}

/// Split a linestring into microsegments, merging consecutive
/// near-collinear vertices. Returns `(start, end, length_m)` triples.
/// See the module docstring for the walker contract.
pub fn split(coords: &[[f64; 2]], max_length_m: f64) -> Vec<([f64; 2], [f64; 2], f32)> {
    let mut segments = Vec::new();
    if coords.len() < 2 {
        return segments;
    }
    let mut i = 0usize;
    while i + 1 < coords.len() {
        let first_hop = flat_dist(
            coords[i][0],
            coords[i][1],
            coords[i + 1][0],
            coords[i + 1][1],
        );

        if first_hop > max_length_m {
            // Single vertex pair already exceeds the cap — fall back
            // to uniform interpolation for this pair.
            interpolate_pair(
                coords[i],
                coords[i + 1],
                first_hop,
                max_length_m,
                &mut segments,
            );
            i += 1;
            continue;
        }

        // Walker: greedy-extend `j` while length stays under cap and
        // every intermediate vertex stays within `CHORD_EPS_M` of the
        // candidate chord `(i, j+1)`.
        let mut j = i + 1;
        let mut cum_len = first_hop;
        while j + 1 < coords.len() {
            let next = j + 1;
            let extra = flat_dist(coords[j][0], coords[j][1], coords[next][0], coords[next][1]);
            let candidate_len = cum_len + extra;
            if candidate_len > max_length_m {
                break;
            }
            let mut chord_ok = true;
            for k in (i + 1)..=j {
                if perp_distance_to_chord(coords[k], coords[i], coords[next]) > CHORD_EPS_M {
                    chord_ok = false;
                    break;
                }
            }
            if !chord_ok {
                break;
            }
            j = next;
            cum_len = candidate_len;
        }
        segments.push((coords[i], coords[j], cum_len as f32));
        i = j;
    }

    segments
}

/// Uniform sub-segments between `a` and `b` when their gap exceeds
/// `max_length_m`. Antimeridian-safe longitude delta.
fn interpolate_pair(
    a: [f64; 2],
    b: [f64; 2],
    dist: f64,
    max_length_m: f64,
    segments: &mut Vec<([f64; 2], [f64; 2], f32)>,
) {
    let n = (dist / max_length_m).ceil() as usize;
    let mut dlon = b[1] - a[1];
    if dlon > 180.0 {
        dlon -= 360.0;
    }
    if dlon < -180.0 {
        dlon += 360.0;
    }
    let seg_len = dist / n as f64;
    for j in 0..n {
        let t0 = j as f64 / n as f64;
        let t1 = (j + 1) as f64 / n as f64;
        let p0 = [a[0] + (b[0] - a[0]) * t0, a[1] + dlon * t0];
        let p1 = [a[0] + (b[0] - a[0]) * t1, a[1] + dlon * t1];
        segments.push((p0, p1, seg_len as f32));
    }
}

#[cfg(test)]
mod tests {
    use super::{bearing_deg, split};

    fn near(a: f32, b: f32) -> bool {
        let diff = (a - b).abs();
        diff < 0.5 || (360.0 - diff).abs() < 0.5
    }

    /// 100 m straight line discretised into 11 collinear OSM vertices at
    /// 10 m spacing — the merger should emit ONE microsegment.
    #[test]
    fn collinear_dense_polyline_merges_to_single_segment() {
        let mut coords = Vec::new();
        for i in 0..=10 {
            let t = i as f64 / 10.0;
            coords.push([
                50.0,
                14.0 + t * 100.0 / (111_320.0 * (50.0_f64.to_radians()).cos()),
            ]);
        }
        let segs = split(&coords, 250.0);
        assert_eq!(
            segs.len(),
            1,
            "dense collinear polyline should merge to 1, got {}",
            segs.len()
        );
        assert!(
            (segs[0].2 - 100.0).abs() < 1.0,
            "merged length ~100 m, got {}",
            segs[0].2
        );
    }

    /// 300 m straight line at 10 m spacing — should emit two segments
    /// at the 250 m max-length cap, not 30 segments.
    #[test]
    fn straight_line_past_max_emits_at_cap() {
        let cos_lat = (50.0_f64.to_radians()).cos();
        let mut coords = Vec::new();
        for i in 0..=30 {
            let t = i as f64 / 30.0;
            coords.push([50.0, 14.0 + t * 300.0 / (111_320.0 * cos_lat)]);
        }
        let segs = split(&coords, 250.0);
        assert_eq!(
            segs.len(),
            2,
            "300 m polyline at max=250 m → 2 segs, got {}",
            segs.len()
        );
        let total: f32 = segs.iter().map(|s| s.2).sum();
        assert!(
            (total - 300.0).abs() < 1.0,
            "total length ~300 m, got {total}"
        );
    }

    /// A sharp 90° corner mid-polyline breaks the chord tolerance so
    /// the walker emits two separate segments at the bend.
    #[test]
    fn sharp_bend_breaks_walker_at_corner() {
        let cos_lat = (50.0_f64.to_radians()).cos();
        let step_lon = 50.0 / (111_320.0 * cos_lat);
        let step_lat = 50.0 / 110_540.0;
        // Three collinear east-going then three collinear north-going.
        let coords = vec![
            [50.0, 14.0],
            [50.0, 14.0 + step_lon],
            [50.0, 14.0 + 2.0 * step_lon],
            [50.0, 14.0 + 3.0 * step_lon],
            [50.0 + step_lat, 14.0 + 3.0 * step_lon],
            [50.0 + 2.0 * step_lat, 14.0 + 3.0 * step_lon],
        ];
        let segs = split(&coords, 250.0);
        assert!(
            segs.len() >= 2,
            "sharp bend should emit at least 2 segments, got {}",
            segs.len()
        );
    }

    /// Long single hop past the max-length cap must still fall back to
    /// the legacy uniform interpolation (no intermediate vertices to
    /// merge over).
    #[test]
    fn long_single_hop_uses_uniform_interpolation_fallback() {
        let cos_lat = (50.0_f64.to_radians()).cos();
        let coords = vec![[50.0, 14.0], [50.0, 14.0 + 600.0 / (111_320.0 * cos_lat)]];
        let segs = split(&coords, 250.0);
        assert_eq!(
            segs.len(),
            3,
            "600 m / 250 m max → 3 interpolated segs, got {}",
            segs.len()
        );
        for s in &segs {
            assert!(
                s.2 <= 250.0 + 0.5,
                "interpolated seg length should be ≤ 250 m, got {}",
                s.2
            );
        }
    }

    #[test]
    fn due_north() {
        assert!(near(bearing_deg(50.0, 14.0, 51.0, 14.0), 0.0));
    }

    #[test]
    fn due_east() {
        assert!(near(bearing_deg(50.0, 14.0, 50.0, 15.0), 90.0));
    }

    #[test]
    fn due_south() {
        assert!(near(bearing_deg(50.0, 14.0, 49.0, 14.0), 180.0));
    }

    #[test]
    fn due_west() {
        assert!(near(bearing_deg(50.0, 14.0, 50.0, 13.0), 270.0));
    }

    #[test]
    fn lkpr_rwy06_designator_matches_geometry() {
        // LKPR RWY 06 threshold at (50.103, 14.236), other end at
        // (50.118, 14.286). Designator "06" → magnetic bearing ~060°.
        // Without cos(lat) scaling, raw atan2 would read ~70° at 50°N.
        let bearing = bearing_deg(50.103, 14.236, 50.118, 14.286);
        assert!(
            (bearing - 60.0).abs() < 5.0,
            "expected ~60°, got {bearing}°"
        );
    }
}

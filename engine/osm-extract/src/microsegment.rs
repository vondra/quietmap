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

use grid::geo::{
    flat_dist, m_per_deg_lon, normalize_longitude, wrapped_longitude_delta, M_PER_DEG_LAT,
};

/// Flat-earth bearing in degrees (0..360), 0 = North, 90 = East.
/// Scales longitude by cos(mid_lat) so LKPR's 60° runway reads 60°,
/// not 70° (raw `atan2(Δlon, Δlat)` overshoot at non-equator latitudes).
/// Antimeridian wrap consistent with `flat_dist`.
pub fn bearing_deg(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f32 {
    let mid_lat = (lat1 + lat2) / 2.0;
    let cos_lat = mid_lat.to_radians().cos();
    let dlon = wrapped_longitude_delta(lon1, lon2);
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
/// surveyor digitisation noise; shared source nodes are protected separately.
const CHORD_EPS_M: f64 = 1.0;

/// Perpendicular distance from `p` to the chord `(a, b)` in metres.
/// Antimeridian-safe via the same `cos(mid_lat)` scaling used by
/// [`flat_dist`].
fn perp_distance_to_chord(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let mid_lat = (a[0] + b[0]) / 2.0;
    let m_lon = m_per_deg_lon(mid_lat.to_radians());
    let bdlon = wrapped_longitude_delta(a[1], b[1]);
    let pdlon = wrapped_longitude_delta(a[1], p[1]);
    let bx = bdlon * m_lon;
    let by = (b[0] - a[0]) * M_PER_DEG_LAT;
    let px = pdlon * m_lon;
    let py = (p[0] - a[0]) * M_PER_DEG_LAT;
    let ab_len_sq = bx * bx + by * by;
    if ab_len_sq < 1e-9 {
        return (px * px + py * py).sqrt();
    }
    let t = (px * bx + py * by) / ab_len_sq;
    let foot_x = t * bx;
    let foot_y = t * by;
    ((px - foot_x).powi(2) + (py - foot_y).powi(2)).sqrt()
}

/// End a simplification range at each shared source node or missing coordinate.
pub fn split_at_junctions(
    nodes: impl IntoIterator<Item = (Option<[f64; 2]>, bool)>,
    max_length_m: f64,
) -> Vec<([f64; 2], [f64; 2], f32)> {
    let mut segments = Vec::new();
    let mut range = Vec::new();
    for (coords, junction) in nodes {
        if let Some(coords) = coords {
            if range.last() != Some(&coords) {
                range.push(coords);
            }
            if junction {
                segments.extend(split(&range, max_length_m));
                range.clear();
                range.push(coords);
            }
        } else {
            segments.extend(split(&range, max_length_m));
            range.clear();
        }
    }
    segments.extend(split(&range, max_length_m));
    segments
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
    let dlon = wrapped_longitude_delta(a[1], b[1]);
    let seg_len = dist / n as f64;
    for j in 0..n {
        let t0 = j as f64 / n as f64;
        let t1 = (j + 1) as f64 / n as f64;
        let p0 = if j == 0 {
            a
        } else {
            [
                a[0] + (b[0] - a[0]) * t0,
                normalize_longitude(a[1] + dlon * t0),
            ]
        };
        let p1 = if j + 1 == n {
            b
        } else {
            [
                a[0] + (b[0] - a[0]) * t1,
                normalize_longitude(a[1] + dlon * t1),
            ]
        };
        segments.push((p0, p1, seg_len as f32));
    }
}

#[cfg(test)]
mod tests {
    use super::{bearing_deg, split, split_at_junctions};
    use grid::geo::flat_dist;

    #[test]
    fn protected_collinear_source_node_remains_an_exact_segment_endpoint() {
        let coords = [[50.0, 14.0], [50.0, 14.0005], [50.0, 14.001]];
        let merged = split_at_junctions(coords.map(|point| (Some(point), false)), 250.0);
        assert_eq!(merged.len(), 1);
        let protected = split_at_junctions(
            coords
                .into_iter()
                .enumerate()
                .map(|(index, point)| (Some(point), index == 1)),
            250.0,
        );
        assert_eq!(protected.len(), 2);
        assert_eq!((protected[0].0, protected[0].1), (coords[0], coords[1]));
        assert_eq!((protected[1].0, protected[1].1), (coords[1], coords[2]));
    }

    #[test]
    fn repeated_adjacent_source_node_cannot_emit_a_graph_self_loop() {
        let a = [50.0, 14.0];
        let junction = [50.0, 14.0005];
        let b = [50.0, 14.001];
        let segments = split_at_junctions(
            [
                (Some(a), false),
                (Some(junction), true),
                (Some(junction), true),
                (Some(b), false),
            ],
            250.0,
        );
        assert_eq!(segments.len(), 2);
        assert_eq!((segments[0].0, segments[0].1), (a, junction));
        assert_eq!((segments[1].0, segments[1].1), (junction, b));
        assert!(segments.iter().all(|segment| segment.2 > 0.0));
    }

    #[test]
    fn missing_source_node_ends_the_connected_range_without_a_bridge() {
        let a = [50.0, 14.0];
        let b = [50.0002, 14.0];
        let c = [50.0004, 14.0];
        let d = [50.0006, 14.0];
        let segments = split_at_junctions(
            [Some(a), Some(b), None, Some(c), Some(d)].map(|point| (point, false)),
            250.0,
        );
        assert_eq!(
            segments
                .iter()
                .map(|segment| (segment.0, segment.1))
                .collect::<Vec<_>>(),
            vec![(a, b), (c, d)]
        );
    }

    #[test]
    fn interpolation_preserves_source_endpoint_bits_before_grid_quantization() {
        let coords = [[0.1234567, 179.99], [0.1245678, 180.0]];
        let segments = split(&coords, 250.0);
        assert!(segments.len() > 1);
        assert_eq!(segments.first().unwrap().0, coords[0]);
        assert_eq!(segments.last().unwrap().1, coords[1]);
    }

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
    fn antimeridian_interpolation_stays_short_and_canonical() {
        let segments = split(&[[0.0, 179.997], [0.0, -179.997]], 250.0);
        assert_eq!(segments.len(), 3);
        for (start, end, length_m) in segments {
            assert!((-180.0..180.0).contains(&start[1]), "lon={}", start[1]);
            assert!((-180.0..180.0).contains(&end[1]), "lon={}", end[1]);
            assert!(length_m <= 250.0);
            assert!(
                (flat_dist(start[0], start[1], end[0], end[1]) - f64::from(length_m)).abs() < 0.1
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

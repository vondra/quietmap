//! Distribute each ground leg over buffered airport microsegments without double-counting overlap.

use crate::geo::{flat_dist, M_PER_DEG_LAT, M_PER_DEG_LON_EQUATOR};
use grid::geo::wrapped_longitude_delta;

/// Perpendicular buffer used by [`project_leg_onto_airport_lines`] to
/// decide whether an ADS-B leg snapped onto an OSM aeroway. Stage 2C
/// projects every ground leg against the real OSM lines using this
/// value; Stage 1.5 (`stage_airport_discover_runner.rs`) inverts the
/// snap to find OSM-missing candidates and must use the SAME buffer
/// — a vertex Stage 2C would have snapped to must not feed DBSCAN.
pub(crate) const AIRPORT_LINE_SNAP_BUFFER_M: f32 = 50.0;

/// Minimal view into one airport_lines.arrow row — enough to do
/// projection geometry plus the per-row metadata the aggregator needs
/// without re-reading the source Arrow. Geometry coords drive
/// `clipped_overlap_m`; `length_m`/`aeroway_type` ride through for
/// the writer.
#[derive(Clone, Copy, Debug)]
pub struct AirportLineSegment {
    pub osm_id: u64,
    pub segment_idx: u16,
    pub start_lat: f32,
    pub start_lon: f32,
    pub end_lat: f32,
    pub end_lon: f32,
    pub grid: ((i32, i32), (i32, i32)),
    /// Carried through from airport_lines.arrow for downstream rows
    /// (`airport_traffic.length_m`). NOT consulted inside the kernel
    /// — `clipped_overlap_m` recomputes from coords so the local u/v
    /// basis stays consistent with the cached length.
    pub length_m: f32,
    /// OSM aeroway encoding (0=runway, 1=taxiway, 6=stopway, 7=airstrip,
    /// else apron/parking/heliport/gate). The aggregator uses it to
    /// derive `ops_kind` without re-reading the source Arrow row.
    pub aeroway_type: u8,
}

/// One ADS-B leg → one OSM microsegment overlap.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub struct LegIntersection {
    pub osm_id: u64,
    pub segment_idx: u16,
    /// Length of the leg's intersection with the segment's
    /// max-perp buffer rectangle, in meters. Bounded by `leg_len_m`.
    pub length_within_segment_m: f32,
}

/// Project one leg onto all candidate airport-line microsegments,
/// returning the per-segment overlap length. Candidates are typically
/// pre-filtered upstream by a measured z9 bounding-box index so this loop
/// runs over O(10-100) segments per leg, not the global airport list.
///
/// **Energy conservation:** when two OSM segments are within
/// `2 * max_perp_m` of each other (parallel taxiways close together),
/// the raw clipped overlap can exceed the leg length on both segments
/// simultaneously. That would double-count the aircraft's acoustic
/// contribution and bias Leq up by ~3 dB on the overlap patch. The
/// kernel resolves this by normalizing: if `Σ raw_overlap > leg_len`,
/// every overlap is scaled by `leg_len / Σ raw_overlap` so the total
/// matches the leg length exactly. Treats the situation as spatial
/// uncertainty between adjacent segments rather than two physically
/// independent sources.
pub fn project_leg_onto_airport_lines(
    leg_start_lat: f32,
    leg_start_lon: f32,
    leg_end_lat: f32,
    leg_end_lon: f32,
    candidates: &[AirportLineSegment],
    max_perp_m: f32,
) -> Vec<LegIntersection> {
    let leg_len_m = flat_dist(leg_start_lat, leg_start_lon, leg_end_lat, leg_end_lon);
    let mut out = Vec::with_capacity(candidates.len().min(8));
    let mut total: f32 = 0.0;
    for seg in candidates {
        let overlap = clipped_overlap_m(
            leg_start_lat,
            leg_start_lon,
            leg_end_lat,
            leg_end_lon,
            seg,
            max_perp_m,
        );
        if overlap > 0.0 {
            total += overlap;
            out.push(LegIntersection {
                osm_id: seg.osm_id,
                segment_idx: seg.segment_idx,
                length_within_segment_m: overlap,
            });
        }
    }
    if total > leg_len_m && total > 0.0 {
        let scale = leg_len_m / total;
        for h in out.iter_mut() {
            h.length_within_segment_m *= scale;
        }
    }
    out
}

/// Length of the leg `L1→L2` lying within the `max_perp_m` buffer of
/// segment `seg`. 0 when no overlap.
///
/// Algorithm:
/// 1. Build a local meter frame at the segment midpoint where the
///    segment is along the +x axis, length 2·s_half.
/// 2. Project L1, L2 into this frame.
/// 3. Liang-Barsky clip the leg parametric line against the rectangle
///    x ∈ [-s_half, +s_half], y ∈ [-max_perp_m, +max_perp_m].
/// 4. Return (t_max - t_min) · leg_len_m.
fn clipped_overlap_m(
    l1_lat: f32,
    l1_lon: f32,
    l2_lat: f32,
    l2_lon: f32,
    seg: &AirportLineSegment,
    max_perp_m: f32,
) -> f32 {
    let mid_lat = (seg.start_lat + seg.end_lat) * 0.5;
    let delta_lon = wrapped_longitude_delta(f64::from(seg.start_lon), f64::from(seg.end_lon));
    let mid_lon = f64::from(seg.start_lon) + delta_lon * 0.5;
    let cos_lat = (mid_lat as f64).to_radians().cos() as f32;
    let m_per_deg_lon = M_PER_DEG_LON_EQUATOR * cos_lat;

    // Segment vector in (east_m, north_m).
    let seg_dx_m = delta_lon as f32 * m_per_deg_lon;
    let seg_dy_m = (seg.end_lat - seg.start_lat) * M_PER_DEG_LAT;
    let seg_len_m = (seg_dx_m * seg_dx_m + seg_dy_m * seg_dy_m).sqrt();
    // Guard against NaN/inf coordinates and sub-millimeter degenerate
    // segments — both would propagate to garbage clipping results.
    if !seg_len_m.is_finite() || seg_len_m < 1e-3 {
        return 0.0;
    }
    let s_half = seg_len_m * 0.5;
    let inv_len = 1.0 / seg_len_m;
    // u = unit vector along the segment (east_m, north_m components).
    // v = perpendicular, 90° CCW from u in the east-north plane.
    let u_e = seg_dx_m * inv_len;
    let u_n = seg_dy_m * inv_len;
    let v_e = -u_n;
    let v_n = u_e;

    let to_local = |lat: f32, lon: f32| -> (f32, f32) {
        let dlon_m = wrapped_longitude_delta(mid_lon, f64::from(lon)) as f32 * m_per_deg_lon;
        let dlat_m = (lat - mid_lat) * M_PER_DEG_LAT;
        let x = dlon_m * u_e + dlat_m * u_n;
        let y = dlon_m * v_e + dlat_m * v_n;
        (x, y)
    };

    let (l1x, l1y) = to_local(l1_lat, l1_lon);
    let (l2x, l2y) = to_local(l2_lat, l2_lon);
    let leg_dx = l2x - l1x;
    let leg_dy = l2y - l1y;
    let leg_len_m = (leg_dx * leg_dx + leg_dy * leg_dy).sqrt();
    if !leg_len_m.is_finite() || leg_len_m < 1e-3 {
        return 0.0;
    }

    // Liang-Barsky parametric clip. t ∈ [t_min, t_max] is the slice
    // of the leg inside the rectangle. p · q encodes each edge:
    //   p < 0 → ray entering through this edge (update t_min)
    //   p > 0 → ray leaving through this edge (update t_max)
    //   p == 0 && q < 0 → fully outside parallel to this edge
    let mut t_min = 0.0f32;
    let mut t_max = 1.0f32;
    let pq = [
        (-leg_dx, l1x + s_half),     // left edge x = -s_half
        (leg_dx, s_half - l1x),      // right edge x = +s_half
        (-leg_dy, l1y + max_perp_m), // bottom edge y = -max_perp_m
        (leg_dy, max_perp_m - l1y),  // top edge y = +max_perp_m
    ];
    for (p, q) in pq {
        // 1e-6 m is below the noise floor of the (lat,lon)→meter
        // propagation at airport-scale input precision — a leg whose
        // along-this-axis component is below this is treated as
        // parallel to that edge.
        if p.abs() < 1e-6 {
            if q < 0.0 {
                return 0.0;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                if r > t_max {
                    return 0.0;
                }
                if r > t_min {
                    t_min = r;
                }
            } else {
                if r < t_min {
                    return 0.0;
                }
                if r < t_max {
                    t_max = r;
                }
            }
        }
    }
    if t_max <= t_min {
        return 0.0;
    }
    (t_max - t_min) * leg_len_m
}

#[cfg(test)]
#[path = "airport_traffic_tests.rs"]
mod tests;

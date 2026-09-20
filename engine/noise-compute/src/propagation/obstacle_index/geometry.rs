//! Shared exact segment geometry and conservative ray-cell bounds.

use super::ObstacleEdge;

/// Distance from the ORIGIN to the segment `(x0,y0)-(x1,y1)` (both already
/// origin-relative). The `near_m` of a [`super::SkylineArc`]: how far away the thing
/// standing in those directions actually is.
#[inline]
pub(crate) fn origin_to_segment_dist(x0: f64, y0: f64, x1: f64, y1: f64) -> f64 {
    let (ex, ey) = (x1 - x0, y1 - y0);
    let len2 = ex * ex + ey * ey;
    let t = if len2 > 0.0 {
        (-(x0 * ex + y0 * ey) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (px, py) = (x0 + t * ex, y0 + t * ey);
    (px * px + py * py).sqrt()
}

/// Closed, padded AABB of one DDA cell visit.
#[inline]
pub(super) fn ray_cell_aabb(
    sx: f64,
    sy: f64,
    dx: f64,
    dy: f64,
    t_lo: f64,
    t_hi: f64,
) -> ((f64, f64), (f64, f64)) {
    let ray_x0 = sx + dx * t_lo;
    let ray_x1 = sx + dx * t_hi;
    let ray_y0 = sy + dy * t_lo;
    let ray_y1 = sy + dy * t_hi;
    // DDA boundaries accumulate t_delta, whereas the exact intersection uses
    // independent cross products. This pad only covers their f64 last-ulp
    // disagreement; it cannot change the exact predicate's answer.
    let pad = 1e-9 * (1.0 + dx.abs() + dy.abs());
    (
        (ray_x0.min(ray_x1) - pad, ray_x0.max(ray_x1) + pad),
        (ray_y0.min(ray_y1) - pad, ray_y0.max(ray_y1) + pad),
    )
}

/// Conservative broad phase for one edge in one DDA cell visit.
///
/// The edge is binned by supercover and the ray DDA visits every crossed cell;
/// a closed, padded box therefore only rejects an edge whose crossing cannot
/// be in this cell. Boundary touches always reach the exact predicate below.
#[inline]
pub(super) fn ray_cell_aabb_may_overlap(
    ray_x: (f64, f64),
    ray_y: (f64, f64),
    edge: &ObstacleEdge,
) -> bool {
    let (edge_x_lo, edge_x_hi) = (edge.x0.min(edge.x1) as f64, edge.x0.max(edge.x1) as f64);
    let (edge_y_lo, edge_y_hi) = (edge.y0.min(edge.y1) as f64, edge.y0.max(edge.y1) as f64);

    !(ray_x.1 < edge_x_lo || edge_x_hi < ray_x.0 || ray_y.1 < edge_y_lo || edge_y_hi < ray_y.0)
}

/// Angle folded into `(−π, π]` — shared by the skyline walk and
/// [`crate::propagation::arc_screening`], which must agree on the unwrapping convention.
#[inline]
pub fn wrap_pi(a: f64) -> f64 {
    use std::f64::consts::{PI, TAU};
    let a = a % TAU;
    if a > PI {
        a - TAU
    } else if a <= -PI {
        a + TAU
    } else {
        a
    }
}

/// Chainage of the intersection of ray `(sx,sy)+t·(dx,dy)` with segment
/// `(x0,y0)–(x1,y1)`, if any, with `t` strictly inside `(0, 1)` and the hit
/// strictly inside the segment (`u ∈ [0, 1]`). Standard 2D cross-product
/// parametric form; collinear overlap returns `None` (a ray sliding along a
/// wall face grazes it, it does not cross it).
///
/// One primitive serves both kinds in the index — building ring edges and
/// noise-barrier polyline edges (`super::Builder::add_polyline`) — one rounding, one
/// set of edge cases.
#[inline]
pub(crate) fn segment_intersection_t(
    sx: f64,
    sy: f64,
    dx: f64,
    dy: f64,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
) -> Option<f64> {
    let ex = x1 - x0;
    let ey = y1 - y0;
    let denom = dx * ey - dy * ex;
    if denom == 0.0 {
        return None;
    }
    let wx = x0 - sx;
    let wy = y0 - sy;
    let t = (wx * ey - wy * ex) / denom;
    let u = (wx * dy - wy * dx) / denom;
    if t > 0.0 && t < 1.0 && (0.0..=1.0).contains(&u) {
        Some(t)
    } else {
        None
    }
}

impl super::ObstacleIndex {
    /// Reject rays outside the grid slab before walking any cells.
    #[inline]
    pub fn segment_may_hit(&self, src_lat: f64, src_lon: f64, rcv_lat: f64, rcv_lon: f64) -> bool {
        if self.edges.is_empty() {
            return false;
        }
        let (sx, sy) = self.to_local(src_lat, src_lon);
        let (rx, ry) = self.to_local(rcv_lat, rcv_lon);
        let max_x = self.min_x + self.cols as f64 * self.cell_m;
        let max_y = self.min_y + self.rows as f64 * self.cell_m;
        // Cheap AABB-vs-AABB reject first — it catches the common case (a ray
        // wholly on the far side of a neighbouring cell) without any division.
        if sx.max(rx) < self.min_x
            || sx.min(rx) > max_x
            || sy.max(ry) < self.min_y
            || sy.min(ry) > max_y
        {
            return false;
        }
        // Slab test for the diagonal cases the bbox overlap cannot decide.
        let (dx, dy) = (rx - sx, ry - sy);
        let mut lo = 0.0f64;
        let mut hi = 1.0f64;
        for (s0, d, b0, b1) in [(sx, dx, self.min_x, max_x), (sy, dy, self.min_y, max_y)] {
            if d.abs() < 1e-12 {
                if s0 < b0 || s0 > b1 {
                    return false;
                }
                continue;
            }
            let (mut a, mut b) = ((b0 - s0) / d, (b1 - s0) / d);
            if a > b {
                std::mem::swap(&mut a, &mut b);
            }
            lo = lo.max(a);
            hi = hi.min(b);
            if lo > hi {
                return false;
            }
        }
        true
    }
}

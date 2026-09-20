//! Intersect nearby buildings with a receiver-centred direction lattice.

use super::{
    origin_to_segment_dist, segment_intersection_t, wrap_pi, CrossingScratch, ObstacleIndex,
    ObstacleKind,
};
use grid::geo::{wrapped_longitude_delta, M_PER_DEG_LAT};

impl ObstacleIndex {
    /// Visit exact intersections between nearby building edges and a fixed
    /// receiver-centred direction lattice. Unlike running one DDA per ray,
    /// this scans the neighbourhood once and projects each edge only onto the
    /// sector centres inside its angular span. The final intersection uses the
    /// same cross-product primitive as [`Self::crossings`].
    #[allow(clippy::too_many_arguments)]
    pub(super) fn visit_building_sector_crossings<const SECTORS: usize>(
        &self,
        receiver_lat: f64,
        receiver_lon: f64,
        query_m_per_deg_lat: f64,
        query_m_per_deg_lon: f64,
        radius_m: f64,
        directions: &[(f64, f64); SECTORS],
        scratch: &mut CrossingScratch,
        visit: &mut impl FnMut(usize, f64, f32),
    ) {
        if self.edges.is_empty() {
            return;
        }
        let (origin_x, origin_y) = self.to_local(receiver_lat, receiver_lon);
        let x_radius = radius_m * self.m_per_deg_lon / query_m_per_deg_lon;
        let y_radius = radius_m * M_PER_DEG_LAT / query_m_per_deg_lat;
        let inv_cell = 1.0 / self.cell_m;
        let cell_range = |lo: f64, hi: f64, base: f64, n: usize| -> Option<(usize, usize)> {
            let c0 = ((lo - base) * inv_cell).floor();
            let c1 = ((hi - base) * inv_cell).floor();
            if c1 < 0.0 || c0 > (n - 1) as f64 {
                return None;
            }
            Some((c0.max(0.0) as usize, c1.min((n - 1) as f64) as usize))
        };
        let Some((cx0, cx1)) = cell_range(
            origin_x - x_radius,
            origin_x + x_radius,
            self.min_x,
            self.cols,
        ) else {
            return;
        };
        let Some((cy0, cy1)) = cell_range(
            origin_y - y_radius,
            origin_y + y_radius,
            self.min_y,
            self.rows,
        ) else {
            return;
        };

        let generation = scratch.begin_ray();
        let longitude_scale = query_m_per_deg_lon / self.m_per_deg_lon;
        let latitude_scale = query_m_per_deg_lat / M_PER_DEG_LAT;
        let origin_east_m =
            wrapped_longitude_delta(receiver_lon, self.origin_lon) * query_m_per_deg_lon;
        let origin_north_m = (self.origin_lat - receiver_lat) * query_m_per_deg_lat;
        let sector_width = std::f64::consts::TAU / SECTORS as f64;
        for cy in cy0..=cy1 {
            let row = cy * self.cols;
            for cx in cx0..=cx1 {
                let lo = self.cell_starts[row + cx] as usize;
                let hi = self.cell_starts[row + cx + 1] as usize;
                for &edge_ref in &self.edge_refs[lo..hi] {
                    let slot = edge_ref as usize & (scratch.recent.len() - 1);
                    let tag = (u64::from(generation) << 32) | u64::from(edge_ref);
                    if scratch.recent[slot] == tag {
                        continue;
                    }
                    scratch.recent[slot] = tag;
                    let edge = self.edges[edge_ref as usize];
                    if edge.kind() != ObstacleKind::Building {
                        continue;
                    }
                    let x0 = origin_east_m + f64::from(edge.x0) * longitude_scale;
                    let y0 = origin_north_m + f64::from(edge.y0) * latitude_scale;
                    let x1 = origin_east_m + f64::from(edge.x1) * longitude_scale;
                    let y1 = origin_north_m + f64::from(edge.y1) * latitude_scale;
                    if origin_to_segment_dist(x0, y0, x1, y1) > radius_m {
                        continue;
                    }
                    let angle0 = y0.atan2(x0);
                    let angle1 = angle0 + wrap_pi(y1.atan2(x1) - angle0);
                    let lo_angle = angle0.min(angle1);
                    let hi_angle = angle0.max(angle1);
                    let first = (lo_angle / sector_width - 0.5).ceil() as i64;
                    let last = (hi_angle / sector_width - 0.5).floor() as i64;
                    for unwrapped_sector in first..=last {
                        let sector = unwrapped_sector.rem_euclid(SECTORS as i64) as usize;
                        let (sin_angle, cos_angle) = directions[sector];
                        if let Some(t) = segment_intersection_t(
                            0.0,
                            0.0,
                            cos_angle * radius_m,
                            sin_angle * radius_m,
                            x0,
                            y0,
                            x1,
                            y1,
                        ) {
                            visit(sector, t * radius_m, edge.height_m);
                        }
                    }
                }
            }
        }
    }
}

//! Build obstacle edges from rings, WKB footprints and open barrier polylines.

use super::{Builder, ObstacleEdge, ObstacleIndex, ObstacleKind};
use crate::{constants::BUILDING_HEIGHT_MAX_M, envelope::EnvelopeClass};
use grid::geo::{m_per_deg_lon, wrapped_longitude_delta, M_PER_DEG_LAT};

impl ObstacleIndex {
    /// Build from closed rings given as `(lat, lon)` sequences (first point
    /// need not be repeated at the end; the closing edge is added). Open
    /// polylines (noise barriers) go through [`Builder::add_polyline`].
    pub fn builder(origin_lat: f64, origin_lon: f64) -> Builder {
        Builder {
            origin_lat,
            origin_lon,
            m_per_deg_lon: m_per_deg_lon(origin_lat.to_radians()),
            edges: Vec::new(),
            footprint_class: Vec::new(),
        }
    }
}

impl Builder {
    #[inline]
    fn to_local(&self, lat: f64, lon: f64) -> (f64, f64) {
        (
            wrapped_longitude_delta(self.origin_lon, lon) * self.m_per_deg_lon,
            (lat - self.origin_lat) * M_PER_DEG_LAT,
        )
    }

    /// Add a closed ring (footprint outer ring or hole — holes screen too:
    /// a courtyard wall is a wall). The closing edge back to the first
    /// point is added automatically.
    pub fn add_ring(&mut self, ring: &[(f64, f64)], height_m: f32, kind: ObstacleKind, id: u32) {
        // `is_finite` + `<= 0` together reject NaN heights; non-finite
        // coordinates would otherwise bin into cell (0,0) and panic the
        // t-sort downstream.
        if ring.len() < 3
            || !height_m.is_finite()
            || height_m <= 0.0
            || ring.iter().any(|(a, o)| !a.is_finite() || !o.is_finite())
        {
            return;
        }
        // This is the single formation site for building obstacle edges: both
        // WKB loaders route every outer ring and hole through it. Noise barriers
        // are a separate physical domain and retain their mapped height.
        let height_m = if kind == ObstacleKind::Building {
            height_m.min(BUILDING_HEIGHT_MAX_M as f32)
        } else {
            height_m
        };
        for i in 0..ring.len() {
            let (lat0, lon0) = ring[i];
            let (lat1, lon1) = ring[(i + 1) % ring.len()];
            if lat0 == lat1 && lon0 == lon1 {
                continue; // explicit closing repeat in the source data
            }
            let (x0, y0) = self.to_local(lat0, lon0);
            let (x1, y1) = self.to_local(lat1, lon1);
            self.edges.push(ObstacleEdge {
                x0: x0 as f32,
                y0: y0 as f32,
                x1: x1 as f32,
                y1: y1 as f32,
                height_m,
                id,
                kind: kind.code(),
            });
        }
    }

    /// Add every ring of a raw-WKB Polygon/MultiPolygon footprint (outer
    /// rings AND holes — a courtyard wall is a wall). Invalid or non-areal
    /// WKB adds nothing. This is the obstacle-store ingestion entry: the
    /// per-cell arrows carry Overture WKB bytes unencoded.
    pub fn add_polygon_wkb(
        &mut self,
        wkb: &[u8],
        height_m: f32,
        kind: ObstacleKind,
        id: u32,
        class: EnvelopeClass,
    ) {
        let slot = id as usize;
        if self.footprint_class.len() <= slot {
            self.footprint_class
                .resize(slot + 1, EnvelopeClass::Default as u8);
        }
        self.footprint_class[slot] = class as u8;
        for (outer, holes) in crate::wkb::parse_wkb_polygons_bytes(wkb) {
            self.add_ring(&outer, height_m, kind, id);
            for hole in &holes {
                self.add_ring(hole, height_m, kind, id);
            }
        }
    }

    /// Add an open polyline (noise barrier segment chain).
    pub fn add_polyline(&mut self, pts: &[(f64, f64)], height_m: f32, kind: ObstacleKind, id: u32) {
        if pts.len() < 2
            || !height_m.is_finite()
            || height_m <= 0.0
            || pts.iter().any(|(a, o)| !a.is_finite() || !o.is_finite())
        {
            return;
        }
        for w in pts.windows(2) {
            let (x0, y0) = self.to_local(w[0].0, w[0].1);
            let (x1, y1) = self.to_local(w[1].0, w[1].1);
            self.edges.push(ObstacleEdge {
                x0: x0 as f32,
                y0: y0 as f32,
                x1: x1 as f32,
                y1: y1 as f32,
                height_m,
                id,
                kind: kind.code(),
            });
        }
    }
}

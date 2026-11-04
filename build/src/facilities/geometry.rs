//! WGS84 facility geometry preserves the mapped integer grid and the registry's original precision.

use anyhow::{Result, ensure};
use geos::{Geom, Geometry, GeometryTypes};

pub type Ring = Vec<[f64; 2]>;
pub type Polygon = Vec<Ring>;

pub fn mapped_geometry(bytes: &[u8]) -> Result<Vec<u8>> {
    ensure!(bytes.len() >= 4, "truncated mapped facility geometry");
    let n = u32::from_le_bytes(bytes[..4].try_into()?) as usize;
    ensure!(
        bytes.len() == 4 + n * 8,
        "invalid mapped facility geometry length"
    );
    let mut wkb = vec![1];
    wkb.extend(2_u32.to_le_bytes());
    wkb.extend((n as u32).to_le_bytes());
    for point in bytes[4..].chunks_exact(8) {
        let x = i32::from_le_bytes(point[..4].try_into()?);
        let y = i32::from_le_bytes(point[4..].try_into()?);
        let (lat, lon) = crate::dev4::z30_corner_degrees(x, y);
        wkb.extend(lon.to_le_bytes());
        wkb.extend(lat.to_le_bytes());
    }
    Ok(wkb)
}

pub fn grid_point(longitude: f64, latitude: f64) -> (i32, i32) {
    let radius = tiles::geo::WGS84_A_M;
    let x = radius * longitude.to_radians();
    let y = radius
        * (std::f64::consts::FRAC_PI_4 + latitude.to_radians() / 2.0)
            .tan()
            .ln();
    let cell = |v: f64| (v / crate::dev4::Z30_QUANTUM_M).round() as i32 + (1 << 29);
    (cell(x), cell(y))
}

pub fn grid_ring(geometry: Option<&[u8]>) -> Result<Vec<(i32, i32)>> {
    let Some(bytes) = geometry else {
        return Ok(Vec::new());
    };
    Ok(rings(bytes)?
        .first()
        .and_then(|p| p.first())
        .into_iter()
        .flatten()
        .map(|p| grid_point(p[0], p[1]))
        .collect())
}

fn coordinates(geometry: &impl Geom) -> Result<Ring> {
    let sequence = geometry.get_coord_seq()?;
    (0..sequence.size()?)
        .map(|i| Ok([sequence.get_x(i)?, sequence.get_y(i)?]))
        .collect()
}

fn parts(geometry: &impl Geom) -> Result<Vec<Polygon>> {
    match geometry.geometry_type()? {
        GeometryTypes::Point | GeometryTypes::LineString | GeometryTypes::LinearRing => {
            Ok(vec![vec![coordinates(geometry)?]])
        }
        GeometryTypes::Polygon => {
            let mut rings = vec![coordinates(&geometry.get_exterior_ring()?)?];
            for i in 0..geometry.get_num_interior_rings()? {
                rings.push(coordinates(&geometry.get_interior_ring_n(i)?)?);
            }
            Ok(vec![rings])
        }
        GeometryTypes::MultiPolygon => {
            let mut result = Vec::new();
            for i in 0..geometry.get_num_geometries()? {
                result.extend(parts(&geometry.get_geometry_n(i)?)?);
            }
            Ok(result)
        }
        _ => anyhow::bail!("unsupported facility geometry"),
    }
}

pub fn rings(bytes: &[u8]) -> Result<Vec<Polygon>> {
    parts(&Geometry::new_from_wkb(bytes)?)
}

pub fn longitude_delta(value: f64) -> f64 {
    (value + 180.0).rem_euclid(360.0) - 180.0
}

pub fn contains(ring: &[[f64; 2]], point: [f64; 2]) -> bool {
    if ring.len() < 3 {
        return false;
    }
    let reference = ring[0][0];
    let x = reference + longitude_delta(point[0] - reference);
    let y = point[1];
    let mut inside = false;
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let xi = reference + longitude_delta(ring[i][0] - reference);
        let xj = reference + longitude_delta(ring[j][0] - reference);
        let (yi, yj) = (ring[i][1], ring[j][1]);
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

pub fn contains_parts(parts: &[Polygon], point: [f64; 2]) -> bool {
    parts.iter().any(|rings| {
        rings.first().is_some_and(|outer| contains(outer, point))
            && !rings[1..].iter().any(|hole| contains(hole, point))
    })
}

pub fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    let dx = longitude_delta(b[0] - a[0])
        * crate::osm::geometry::M_PER_DEG_LON_EQ
        * ((a[1] + b[1]) * 0.5).to_radians().cos();
    let dy = (b[1] - a[1]) * crate::osm::geometry::M_PER_DEG_LAT;
    dx.hypot(dy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapped_geometry_roundtrips_integer_grid_including_poles_and_dateline() {
        let points = [
            (0, 0),
            (1, 1),
            (1 << 29, 1 << 29),
            ((1 << 30) - 1, (1 << 30) - 1),
        ];
        let mut bytes = (points.len() as u32).to_le_bytes().to_vec();
        for (x, y) in points {
            let (latitude, longitude) = crate::dev4::z30_corner_degrees(x, y);
            assert_eq!(grid_point(longitude, latitude), (x, y));
            bytes.extend(x.to_le_bytes());
            bytes.extend(y.to_le_bytes());
        }
        assert_eq!(
            grid_ring(Some(&mapped_geometry(&bytes).unwrap())).unwrap(),
            points
        );
        assert!(mapped_geometry(&bytes[..bytes.len() - 1]).is_err());
    }

    #[test]
    fn a_registry_area_respects_holes_and_crosses_the_dateline() {
        let outer = vec![
            [179.0, -2.0],
            [-179.0, -2.0],
            [-179.0, 2.0],
            [179.0, 2.0],
            [179.0, -2.0],
        ];
        let hole = vec![
            [179.5, -1.0],
            [-179.5, -1.0],
            [-179.5, 1.0],
            [179.5, 1.0],
            [179.5, -1.0],
        ];
        let parts = vec![vec![outer, hole]];
        assert!(!contains_parts(&parts, [180.0, 0.0]));
        assert!(contains_parts(&parts, [-179.2, 0.0]));
        assert!(!contains_parts(&parts, [0.0, 0.0]));
    }
}

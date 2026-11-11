//! GEOS footprints in a common longitude frame, preserving the prepared geometry arithmetic.

use crate::osm::geometry::{self as grid, poly::encode_grid_poly};
use anyhow::{Result, ensure};
use geos::{CoordSeq, Geom, Geometry, GeometryTypes, STRtree, SpatialIndex};

pub const METRES_PER_DEGREE: f64 = 111_320.0;
pub const MATCH_IOU: f64 = 0.5;

// The frozen Python structures producer derives the quantum; OSM extraction's literal
// differs by 3.265e-13 m. Reusing that literal moves round-trip wall centroids by one cell.
const SCREENING_QUANTUM_M: f64 = grid::EARTH_CIRCUMFERENCE_M / (1u64 << 30) as f64;

pub fn grid_to_lonlat(x: i32, y: i32) -> (f64, f64) {
    grid::poly::meters_to_lonlat(
        (i64::from(x) - (1 << 29)) as f64 * SCREENING_QUANTUM_M,
        (i64::from(y) - (1 << 29)) as f64 * SCREENING_QUANTUM_M,
    )
}

pub fn lonlat_to_grid(longitude: f64, latitude: f64) -> (i32, i32) {
    let (x, y) = grid::lonlat_to_meters(longitude, latitude);
    (
        (x / SCREENING_QUANTUM_M).floor() as i32 + (1 << 29),
        (y / SCREENING_QUANTUM_M).floor() as i32 + (1 << 29),
    )
}

// Python's original wrap also rounds already-canonical values; centroids at cell edges depend on it.
pub fn normalize_longitude(longitude: f64) -> f64 {
    (longitude + 180.0).rem_euclid(360.0) - 180.0
}

pub fn wrapped_longitude_delta(from: f64, to: f64) -> f64 {
    normalize_longitude(to - from)
}

pub fn wrapped_longitude_midpoint(from: f64, to: f64) -> f64 {
    normalize_longitude(from + wrapped_longitude_delta(from, to) / 2.0)
}

pub fn coordinate(geometry: &impl Geom) -> Result<(f64, f64)> {
    let sequence = geometry.get_coord_seq()?;
    Ok((sequence.get_x(0)?, sequence.get_y(0)?))
}

pub fn first_coordinate(geometry: &impl Geom) -> Result<(f64, f64)> {
    match geometry.geometry_type()? {
        GeometryTypes::Polygon => coordinate(&geometry.get_exterior_ring()?),
        GeometryTypes::MultiPolygon => first_coordinate(&geometry.get_geometry_n(0)?),
        _ => coordinate(geometry),
    }
}

pub fn longitude_frame(geometry: &impl Geom, reference: f64) -> Result<Geometry> {
    if geometry.get_x_min()? >= reference - 180.0 && geometry.get_x_max()? < reference + 180.0 {
        return Ok(geometry.clone()?);
    }
    geometry.transform_xy(|x, y| {
        let delta = wrapped_longitude_delta(reference, x);
        let turns = ((x - reference - delta) / 360.0).round_ties_even();
        Ok::<_, anyhow::Error>((x - 360.0 * turns, y))
    })
}

pub fn centroid(geometry: &impl Geom) -> Result<(f64, f64)> {
    if geometry.get_x_max()? - geometry.get_x_min()? <= 180.0 {
        return coordinate(&geometry.get_centroid()?);
    }
    let local = longitude_frame(geometry, first_coordinate(geometry)?.0)?;
    let (longitude, latitude) = coordinate(&local.get_centroid()?)?;
    Ok((normalize_longitude(longitude), latitude))
}

pub fn area_m2(geometry: &impl Geom, latitude: f64) -> Result<f64> {
    Ok(
        longitude_frame(geometry, first_coordinate(geometry)?.0)?.area()?
            * METRES_PER_DEGREE.powi(2)
            * latitude.to_radians().cos(),
    )
}

pub fn ring_geometry(bytes: &[u8]) -> Result<Option<Geometry>> {
    let points = super::decode_wall(bytes).ok_or_else(|| anyhow::anyhow!("invalid OSM ring"))?;
    let first = points.first();
    let second = points.iter().find(|p| Some(*p) != first);
    if !points.iter().any(|p| Some(p) != first && Some(p) != second) {
        return Ok(None);
    }
    let mut coordinates: Vec<_> = points
        .into_iter()
        .map(|(x, y)| {
            let (lon, lat) = grid_to_lonlat(x, y);
            [lon, lat]
        })
        .collect();
    if coordinates.first() != coordinates.last() {
        coordinates.push(coordinates[0]);
    }
    Ok(Some(Geometry::create_polygon(
        CoordSeq::new_from_vec(&coordinates)?.create_linear_ring()?,
        vec![],
    )?))
}

fn snapped_ring(ring: &impl Geom) -> Result<Vec<u8>> {
    let coordinates = ring.get_coord_seq()?;
    let mut points = Vec::with_capacity(coordinates.size()?);
    for i in 0..coordinates.size()? {
        points.push(lonlat_to_grid(coordinates.get_x(i)?, coordinates.get_y(i)?));
    }
    Ok(encode_grid_poly(&points))
}

fn append_polygon(geometry: &impl Geom, bytes: &mut Vec<u8>) -> Result<()> {
    let holes = geometry.get_num_interior_rings()?;
    bytes.extend(((holes + 1) as u32).to_le_bytes());
    bytes.extend(snapped_ring(&geometry.get_exterior_ring()?)?);
    for i in 0..holes {
        bytes.extend(snapped_ring(&geometry.get_interior_ring_n(i)?)?);
    }
    Ok(())
}

pub fn snapped_parts(geometry: &impl Geom) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    if geometry.geometry_type()? == GeometryTypes::Polygon {
        bytes.extend(1u32.to_le_bytes());
        append_polygon(geometry, &mut bytes)?;
    } else {
        ensure!(
            geometry.geometry_type()? == GeometryTypes::MultiPolygon,
            "expected footprint polygon"
        );
        let parts = geometry.get_num_geometries()?;
        bytes.extend((parts as u32).to_le_bytes());
        for i in 0..parts {
            append_polygon(&geometry.get_geometry_n(i)?, &mut bytes)?;
        }
    }
    Ok(bytes)
}

fn repaired(geometry: &impl Geom) -> geos::GResult<Geometry> {
    if geometry.is_valid()? {
        geometry.clone()
    } else {
        geometry.buffer(0.0, 8)
    }
}

pub fn overlap(left: &impl Geom, right: &impl Geom) -> Result<(f64, bool)> {
    // GEOS 3.12 refuses both predicates on a self-crossing ring (13 such 3DBAG footprints in one
    // degree); the frozen producer's GEOS 3.14 answered `covers` directly. Both fall back to the
    // buffer-0 repair, and the union keeps the original areas.
    let centroid = left.get_centroid()?;
    let contains = right
        .covers(&centroid)
        .or_else(|_| repaired(right)?.covers(&centroid))?;
    let intersection = left
        .intersection(right)
        .or_else(|_| repaired(left)?.intersection(&repaired(right)?))?
        .area()?;
    let union = left.area()? + right.area()? - intersection;
    Ok((
        if intersection > 0.0 && union > 0.0 {
            intersection / union
        } else {
            0.0
        },
        contains,
    ))
}

pub struct FootprintIndex {
    // GEOS retains geometry pointers until the tree is destroyed.
    tree: STRtree<usize>,
    geometries: Vec<Geometry>,
    pub reference: f64,
}

impl FootprintIndex {
    pub fn new(geometries: Vec<Geometry>) -> Result<Self> {
        let reference = geometries
            .first()
            .map(first_coordinate)
            .transpose()?
            .map_or(0.0, |p| p.0);
        let geometries = geometries
            .iter()
            .map(|g| longitude_frame(g, reference))
            .collect::<Result<Vec<_>>>()?;
        let mut tree = STRtree::with_capacity(10)?;
        for (i, geometry) in geometries.iter().enumerate() {
            tree.insert(geometry, i);
        }
        Ok(Self {
            tree,
            geometries,
            reference,
        })
    }

    pub fn candidates(&self, geometry: &impl Geom) -> Vec<usize> {
        let mut candidates = Vec::new();
        self.tree.query(geometry, |&i| candidates.push(i));
        candidates
    }

    pub fn geometry(&self, index: usize) -> &Geometry {
        &self.geometries[index]
    }
}

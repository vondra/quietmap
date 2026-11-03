//! One wall stock: surveyed hops replace coincident OSM segments across square boundaries.

use super::{
    height_model::{HeightModel, screening_metres},
    observations::Observation,
    prepare_geometry::{self as geometry, FootprintIndex, METRES_PER_DEGREE},
    prepare_rows::{OsmWall, StructureRow},
};
use crate::osm::geometry::poly::encode_grid_poly;
use anyhow::{Result, ensure};
use geos::{CoordSeq, Geom, Geometry};

pub const SUPPORT_HALO_M: f64 = 375.0;
const REPLACE_DISTANCE_M: f64 = 5.0;

fn segment_length(start: (f64, f64), end: (f64, f64)) -> f64 {
    let dx = (end.0 - start.0) * METRES_PER_DEGREE * ((start.1 + end.1) / 2.0).to_radians().cos();
    let dy = (end.1 - start.1) * METRES_PER_DEGREE;
    dx.hypot(dy)
}

fn point_near(point: (f64, f64), index: &FootprintIndex) -> Result<bool> {
    let reach = REPLACE_DISTANCE_M / METRES_PER_DEGREE;
    let reach_lon = reach / point.1.clamp(-89.9, 89.9).to_radians().cos();
    let (left, right) = (point.0 - reach_lon - 1e-9, point.0 + reach_lon + 1e-9);
    let (bottom, top) = (point.1 - reach - 1e-9, point.1 + reach + 1e-9);
    let bbox = Geometry::create_polygon(
        CoordSeq::new_from_vec(&[
            [left, bottom],
            [right, bottom],
            [right, top],
            [left, top],
            [left, bottom],
        ])?
        .create_linear_ring()?,
        vec![],
    )?;
    let origin = CoordSeq::new_from_vec(&[[point.0, point.1]])?.create_point()?;
    for i in index.candidates(&bbox) {
        let nearest = origin.nearest_points(index.geometry(i))?;
        if segment_length(
            (nearest.get_x(0)?, nearest.get_y(0)?),
            (nearest.get_x(1)?, nearest.get_y(1)?),
        ) <= REPLACE_DISTANCE_M
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn replaced(start: (f64, f64), end: (f64, f64), index: &FootprintIndex) -> Result<bool> {
    let reference = index.reference;
    let middle = (
        reference
            + (geometry::wrapped_longitude_delta(reference, start.0)
                + geometry::wrapped_longitude_delta(reference, end.0))
                / 2.0,
        (start.1 + end.1) / 2.0,
    );
    let start = (
        reference + geometry::wrapped_longitude_delta(reference, start.0),
        start.1,
    );
    let end = (
        reference + geometry::wrapped_longitude_delta(reference, end.0),
        end.1,
    );
    Ok(point_near(middle, index)? && point_near(start, index)? && point_near(end, index)?)
}

fn append(
    rows: &mut Vec<StructureRow>,
    start: (i32, i32),
    end: (i32, i32),
    height: f64,
    source: u8,
    id: Option<i64>,
    ordinal: &mut u32,
) -> Result<()> {
    let (a, b) = (
        geometry::grid_to_lonlat(start.0, start.1),
        geometry::grid_to_lonlat(end.0, end.1),
    );
    rows.push(StructureRow {
        kind: 1,
        geometry: Some(encode_grid_poly(&[start, end])),
        height_m: screening_metres(height)?,
        height_source: source,
        envelope: 0,
        centroid: geometry::lonlat_to_grid(
            geometry::wrapped_longitude_midpoint(a.0, b.0),
            (a.1 + b.1) / 2.0,
        ),
        osm_id: id,
        ordinal: Some(*ordinal),
        ..Default::default()
    });
    *ordinal = ordinal
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("too many screening structures"))?;
    Ok(())
}

pub fn append_walls(
    rows: &mut Vec<StructureRow>,
    osm: &[OsmWall],
    official: &[Observation],
    model: &HeightModel,
    mut ordinal: u32,
) -> Result<()> {
    let index = FootprintIndex::new(
        official
            .iter()
            .map(Observation::geometry)
            .collect::<Result<_>>()?,
    )?;
    for wall in osm {
        let start = geometry::grid_to_lonlat(wall.start.0, wall.start.1);
        let end = geometry::grid_to_lonlat(wall.end.0, wall.end.1);
        if replaced(start, end, &index)? {
            continue;
        }
        let (height, source) = model.wall_height(wall.height, wall.mapped, wall.country);
        append(
            rows,
            wall.start,
            wall.end,
            height,
            source,
            Some(wall.osm_id),
            &mut ordinal,
        )?;
    }
    for wall in official.iter().filter(|r| r.owned) {
        let geometry = wall.geometry()?;
        let coordinates = geometry.get_coord_seq()?;
        ensure!(
            coordinates.size()? == 2,
            "normalized wall must be one survey hop"
        );
        let start = (coordinates.get_x(0)?, coordinates.get_y(0)?);
        let end = (coordinates.get_x(1)?, coordinates.get_y(1)?);
        let span = geometry::wrapped_longitude_delta(start.0, end.0);
        let length = segment_length(start, (start.0 + span, end.1));
        let hops = (length / 250.0).ceil().max(1.0) as usize;
        for i in 0..hops {
            let point = |part: usize| {
                let fraction = part as f64 / hops as f64;
                (
                    geometry::normalize_longitude(start.0 + span * fraction),
                    start.1 + (end.1 - start.1) * fraction,
                )
            };
            let (a, b) = if hops == 1 {
                (start, end)
            } else {
                (point(i), point(i + 1))
            };
            append(
                rows,
                geometry::lonlat_to_grid(a.0, a.1),
                geometry::lonlat_to_grid(b.0, b.1),
                wall.height_m
                    .ok_or_else(|| anyhow::anyhow!("wall has no height"))?,
                9,
                None,
                &mut ordinal,
            )?;
        }
    }
    Ok(())
}

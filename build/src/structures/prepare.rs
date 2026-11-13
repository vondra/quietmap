//! Merge admitted OSM and survey evidence into the single structures table consumed by the map.

use super::{
    height_model::{HeightModel, screening_metres},
    height_raster::HeightRaster,
    national_attributes::NationalAttributes,
    observations::{self, Observation},
    prepare_geometry::{self as geometry, FootprintIndex, MATCH_IOU},
    prepare_rows::{self, OsmBuilding, StructureRow},
    prepare_walls,
};
use crate::{
    dev4::Square,
    year::{Kind, YearInputs},
};
use anyhow::{Result, anyhow, ensure};
use geos::{Geom, Geometry};
use rayon::prelude::*;
use std::{collections::HashMap, path::Path};

type FootprintMatches = (Vec<Option<Geometry>>, HashMap<usize, usize>);

pub(super) fn match_footprints(
    osm: &[OsmBuilding],
    footprints: &[Observation],
) -> Result<FootprintMatches> {
    let mut geometries = Vec::with_capacity(osm.len());
    let mut indexed = Vec::new();
    let mut original = Vec::new();
    for (i, building) in osm.iter().enumerate() {
        let geometry = if building.ground_activity {
            None
        } else {
            building
                .row
                .emission_geometry
                .as_deref()
                .map(geometry::ring_geometry)
                .transpose()?
                .flatten()
        };
        if let Some(g) = &geometry {
            indexed.push(Geom::clone(g)?);
            original.push(i);
        }
        geometries.push(geometry);
    }
    let index = FootprintIndex::new(indexed)?;
    let mut edges = Vec::new();
    for (j, footprint) in footprints.iter().enumerate() {
        let g = geometry::longitude_frame(&footprint.geometry()?, index.reference)?;
        for k in index.candidates(&g) {
            let other = index.geometry(k);
            if !g.intersects(other)? {
                continue;
            }
            let (iou, contains) = geometry::overlap(&g, other)?;
            if contains || iou >= MATCH_IOU {
                edges.push((iou, contains, j, original[k]));
            }
        }
    }
    edges.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then(b.1.cmp(&a.1))
            .then(a.2.cmp(&b.2))
            .then(a.3.cmp(&b.3))
    });
    let mut pairs = HashMap::new();
    let mut used = vec![false; footprints.len()];
    for (_, _, j, i) in edges {
        if !used[j] && !pairs.contains_key(&i) {
            pairs.insert(i, j);
            used[j] = true;
        }
    }
    Ok((geometries, pairs))
}

fn measured_height(
    candidate: &Geometry,
    index: &FootprintIndex,
    rows: &[Observation],
) -> Result<Option<f64>> {
    if rows.is_empty() {
        return Ok(None);
    }
    let framed = geometry::longitude_frame(candidate, index.reference)?;
    let mut best_iou = -1.0;
    let mut best = None;
    for i in index.candidates(&framed) {
        let target = index.geometry(i);
        if !framed.intersects(target)? {
            continue;
        }
        let (iou, contains) = geometry::overlap(&framed, target)?;
        if (contains || iou >= MATCH_IOU) && iou > best_iou {
            best_iou = iou;
            best = rows[i].height_m;
        }
    }
    Ok(best)
}

/// A building's surveyed roof: a raster's mean, replaced by a matched footprint survey's height.
pub(super) fn surveyed_roof(
    geometry: &Geometry,
    longitude: f64,
    latitude: f64,
    rasters: &[HeightRaster],
    index: &FootprintIndex,
    rows: &[Observation],
) -> Result<Option<f64>> {
    let mut measured = None;
    for raster in rasters {
        if let Some(height) = raster.sample(geometry, longitude, latitude)? {
            measured = Some(height);
        }
    }
    if let Some(height) = measured_height(geometry, index, rows)? {
        measured = Some(height);
    }
    Ok(measured)
}

pub(super) struct HeightEvidence<'a> {
    pub(super) measured: &'a [Observation],
    pub(super) measured_index: FootprintIndex,
    pub(super) raster: &'a [HeightRaster],
    pub(super) model: &'a HeightModel,
}

impl HeightEvidence<'_> {
    pub(super) fn building(
        &self,
        mut row: StructureRow,
        osm_geometry: Option<&Geometry>,
        building_use: u8,
        ground: bool,
        footprint: Option<&Observation>,
        ordinal: Option<u32>,
    ) -> Result<StructureRow> {
        let footprint_geometry = footprint.map(Observation::geometry).transpose()?;
        let geometry = footprint_geometry.as_ref().or(osm_geometry);
        let (longitude, latitude) = footprint.map_or_else(
            || geometry::grid_to_lonlat(row.centroid.0, row.centroid.1),
            |f| (f.longitude, f.latitude),
        );
        let floors = row
            .floors
            .filter(|f| *f > 0)
            .unwrap_or_else(|| footprint.map_or(0, |f| f.floors));
        let open = building_use == 3 || footprint.is_some_and(|f| f.open_roof);
        let (height, source) = if ground || open {
            (0.0, if ground { 7 } else { 6 })
        } else {
            let measured = match geometry {
                Some(geometry) => surveyed_roof(
                    geometry,
                    longitude,
                    latitude,
                    self.raster,
                    &self.measured_index,
                    self.measured,
                )?,
                None => None,
            };
            let area = geometry
                .map(|g| geometry::area_m2(g, latitude))
                .transpose()?
                .unwrap_or_else(|| f64::from(row.area_m2.unwrap_or(0.0)));
            self.model.height(
                measured,
                row.height.map(f64::from),
                floors,
                footprint.and_then(|f| f.height_m),
                area,
            )
        };
        row.height_m = screening_metres(height)?;
        row.height_source = source;
        row.storeys = Some(
            self.model
                .storeys(floors, (!(ground || open)).then_some(height)),
        );
        row.ordinal = ordinal;
        row.envelope = match building_use {
            0 => 1,
            1 => 2,
            2 => 3,
            3 => 0,
            _ => 5,
        };
        if let Some(footprint) = footprint {
            row.geometry = Some(geometry::snapped_parts(
                footprint_geometry.as_ref().unwrap(),
            )?);
            if row.osm_id.is_some() {
                row.emission_centroid = Some(row.centroid);
            }
            row.centroid = geometry::lonlat_to_grid(longitude, latitude);
            row.envelope = if building_use == 3 {
                0
            } else {
                footprint.envelope
            };
        } else if osm_geometry.is_some() {
            let mut bytes = Vec::new();
            bytes.extend(1u32.to_le_bytes());
            bytes.extend(1u32.to_le_bytes());
            bytes.extend(row.emission_geometry.as_ref().unwrap());
            row.geometry = Some(bytes);
        }
        Ok(row)
    }
}

pub(super) fn square(
    osm_root: &Path,
    input_root: &Path,
    square: Square,
    model: &HeightModel,
    rasters: &[HeightRaster],
    year: &str,
) -> Result<Vec<StructureRow>> {
    let name = format!("z9/{}/{}", square.x, square.y);
    let osm_directory = osm_root.join(&name);
    ensure!(
        osm_directory.join("square-country-city.bin").is_file(),
        "{name}: missing OSM square, cannot infer empty buildings"
    );
    let observations = observations::read(
        &input_root.join(&name).join("structure-observations.arrow"),
        year,
    )?;
    let mut families: [Vec<Observation>; 4] = std::array::from_fn(|_| vec![]);
    for row in observations {
        families[row.kind as usize].push(row);
    }
    let [footprints, measured, walls, attributes] = families;
    let mut osm = prepare_rows::read_osm(&osm_directory.join("buildings.arrow"))?;
    let attributes = NationalAttributes::new(&attributes);
    for building in &mut osm {
        attributes.apply(building);
    }
    let (geometries, pairs) = match_footprints(&osm, &footprints)?;
    let evidence = HeightEvidence {
        measured_index: FootprintIndex::new(
            measured
                .iter()
                .map(Observation::geometry)
                .collect::<Result<_>>()?,
        )?,
        measured: &measured,
        raster: rasters,
        model,
    };
    let mut rows = Vec::with_capacity(osm.len() + footprints.len());
    let mut used = vec![false; footprints.len()];
    let mut ordinal = u32::try_from(footprints.len())?;
    for (i, building) in osm.into_iter().enumerate() {
        let footprint = pairs.get(&i).copied();
        let row_ordinal = if let Some(j) = footprint {
            used[j] = true;
            Some(j as u32)
        } else if geometries[i].is_some() {
            let next = ordinal;
            ordinal = ordinal
                .checked_add(1)
                .ok_or_else(|| anyhow!("too many structures"))?;
            Some(next)
        } else {
            None
        };
        rows.push(evidence.building(
            building.row,
            geometries[i].as_ref(),
            building.building_use,
            building.ground_activity,
            footprint.map(|j| &footprints[j]),
            row_ordinal,
        )?);
    }
    for (j, footprint) in footprints.iter().enumerate().filter(|(j, _)| !used[*j]) {
        rows.push(evidence.building(
            StructureRow::default(),
            None,
            u8::MAX,
            false,
            Some(footprint),
            Some(j as u32),
        )?);
    }
    prepare_walls::append_walls(
        &mut rows,
        &prepare_rows::read_walls(&osm_directory.join("barriers.arrow"))?,
        &walls,
        model,
        ordinal,
    )?;
    Ok(rows)
}

pub fn build(inputs: &YearInputs, squares: &[Square], output: &Path) -> Result<()> {
    ensure!(!output.exists(), "structures output must be new");
    ensure!(
        !squares.is_empty() && squares.iter().all(|s| s.x < 512 && s.y < 512),
        "invalid structure square selection"
    );
    let source = inputs
        .required(Kind::OsmGeography)
        .map_err(anyhow::Error::msg)?;
    let observations = inputs
        .required(Kind::StructureObservations)
        .map_err(anyhow::Error::msg)?;
    let model = HeightModel::read(
        inputs
            .required(Kind::StructureHeightModel)
            .map_err(anyhow::Error::msg)?,
    )?;
    let raster_paths = inputs
        .structure_height_rasters()
        .map_err(anyhow::Error::msg)?;
    let open_rasters = || {
        raster_paths
            .iter()
            .map(|p| HeightRaster::open(p))
            .collect::<Result<Vec<_>>>()
    };
    let mut squares = squares.to_vec();
    squares.sort_unstable();
    squares.dedup();
    // Each worker opens its own rasters: a GDAL dataset serves one thread.
    squares
        .par_iter()
        .try_for_each_init(open_rasters, |rasters, &selected| {
            let rasters = rasters.as_ref().map_err(|e| anyhow!("{e:#}"))?;
            let rows = square(
                source,
                observations,
                selected,
                &model,
                rasters,
                &inputs.manifest.year,
            )?;
            let name = format!("z9/{}/{}", selected.x, selected.y);
            prepare_rows::write(&output.join(&name).join("structures.arrow"), &rows)?;
            eprintln!("structures: {name}: {} rows", rows.len());
            Ok(())
        })
}

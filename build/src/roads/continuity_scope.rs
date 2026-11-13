//! Bound the chain results needed by selected road geometries and their physical carriageway neighbors.

use super::{
    allocation::MAX_CARRIAGEWAY_SEPARATION_M,
    input::{self, Road},
};
use crate::{dev4::Square, osm::geometry};
use anyhow::Result;
use rayon::prelude::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub struct Scope(rstar::RTree<rstar::primitives::Rectangle<[f64; 2]>>);

impl Scope {
    pub fn read(paths: &BTreeMap<Square, PathBuf>, selected: &BTreeSet<Square>) -> Result<Self> {
        let selected_paths = selected
            .iter()
            .filter_map(|square| paths.get(square))
            .collect::<Vec<_>>();
        let boxes = selected_paths
            .par_iter()
            .map(|path| reach_box(path))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self(rstar::RTree::bulk_load(
            boxes
                .into_iter()
                .flatten()
                .map(|b| rstar::primitives::Rectangle::from_corners([b[0], b[1]], [b[2], b[3]]))
                .collect(),
        )))
    }

    pub fn needs(&self, road: &Road) -> bool {
        [
            -geometry::EARTH_CIRCUMFERENCE_M,
            0.0,
            geometry::EARTH_CIRCUMFERENCE_M,
        ]
        .into_iter()
        .any(|shift| {
            let bounds = rstar::AABB::from_corners(
                [
                    road.start.0.min(road.end.0) + shift,
                    road.start.1.min(road.end.1),
                ],
                [
                    road.start.0.max(road.end.0) + shift,
                    road.start.1.max(road.end.1),
                ],
            );
            self.0
                .locate_in_envelope_intersecting(&bounds)
                .next()
                .is_some()
        })
    }
}

/// A square's road bounds widened by the farthest physical carriageway separation at its latitude.
fn reach_box(path: &Path) -> Result<Option<[f64; 4]>> {
    let mut bounds = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for batch in input::load(path)? {
        for road in input::roads(&batch)? {
            for (x, y) in [road.start, road.end] {
                bounds[0] = bounds[0].min(x);
                bounds[1] = bounds[1].min(y);
                bounds[2] = bounds[2].max(x);
                bounds[3] = bounds[3].max(y);
            }
        }
    }
    if !bounds.iter().all(|v| v.is_finite()) {
        return Ok(None);
    }
    let latitude_y = bounds[1].abs().max(bounds[3].abs());
    let reach =
        MAX_CARRIAGEWAY_SEPARATION_M * (latitude_y / geometry::WEB_MERCATOR_RADIUS_M).cosh();
    Ok(Some([
        bounds[0] - reach,
        bounds[1] - reach,
        bounds[2] + reach,
        bounds[3] + reach,
    ]))
}

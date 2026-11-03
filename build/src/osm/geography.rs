//! Indexed country containment with holes, polar polygons and unambiguous coastal attribution.

use super::geometry::{M_PER_DEG_LAT, M_PER_DEG_LON_EQ as M_PER_DEG_LON};
use super::region_input::RegionFile;
use anyhow::{Result, ensure};
use geo::{
    BoundingRect, Contains, Coord, LineString, MultiPolygon, Orient, Polygon,
    indexed::IntervalTreeMultiPolygon, orient::Direction,
};
use rstar::{AABB, RTree, RTreeObject};
use std::{collections::BTreeSet, path::Path};

// The frozen geographic attribution rule: one coast within two kilometres, in local metres.
const COASTAL_BUFFER_M: f64 = 2000.0;

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Identity {
    pub country: u16,
    pub continent: u8,
}

struct Part {
    polygon: IntervalTreeMultiPolygon<f64>,
    region: usize,
    polar: Option<bool>,
    latitude_range: [f64; 2],
}

struct Coast {
    start: [f64; 2],
    end: [f64; 2],
    region: usize,
}

struct Indexed {
    envelope: AABB<[f64; 2]>,
    index: usize,
}

impl RTreeObject for Indexed {
    type Envelope = AABB<[f64; 2]>;
    fn envelope(&self) -> Self::Envelope {
        self.envelope
    }
}

pub struct Regions {
    identities: Vec<Identity>,
    disputed: Vec<bool>,
    parts: Vec<Part>,
    land_index: RTree<Indexed>,
    coasts: Vec<Coast>,
    coastal_index: RTree<Indexed>,
}

fn polar([longitude, latitude]: [f64; 2], south: bool) -> [f64; 2] {
    let radius = if south {
        90.0 + latitude
    } else {
        90.0 - latitude
    };
    [
        radius * longitude.to_radians().cos(),
        radius * longitude.to_radians().sin(),
    ]
}

fn envelope(start: [f64; 2], end: [f64; 2]) -> AABB<[f64; 2]> {
    AABB::from_corners(
        [start[0].min(end[0]), start[1].min(end[1])],
        [start[0].max(end[0]), start[1].max(end[1])],
    )
}

impl Regions {
    pub fn read(path: &Path) -> Result<Self> {
        let input = serde_json::from_slice(&std::fs::read(path)?)?;
        Self::from_input(input)
    }

    fn from_input(input: RegionFile) -> Result<Self> {
        let mut parts = Vec::new();
        let mut land = Vec::new();
        let mut coasts = Vec::new();
        let mut identities = Vec::new();
        let mut disputed = Vec::new();
        for (region, source) in input.regions.into_iter().enumerate() {
            let country = source
                .country
                .map(|iso| {
                    ensure!(
                        iso.len() == 2 && iso.bytes().all(|byte| byte.is_ascii_uppercase()),
                        "invalid ISO country {iso}"
                    );
                    Ok::<u16, anyhow::Error>(u16::from_le_bytes(iso.as_bytes().try_into().unwrap()))
                })
                .transpose()?
                .unwrap_or(0);
            identities.push(Identity {
                country,
                continent: source.continent,
            });
            disputed.push(source.disputed);
            for rings in source.polygons {
                ensure!(
                    !rings.is_empty()
                        && rings.iter().all(|ring| ring.len() >= 4
                            && ring.first() == ring.last()
                            && ring.iter().flatten().all(|value| value.is_finite())),
                    "invalid country polygon"
                );
                let outer = &rings[0];
                let polar_projection = outer
                    .windows(2)
                    .any(|edge| (edge[0][0] - edge[1][0]).abs() > 180.0)
                    .then(|| {
                        outer.iter().map(|point| point[1]).sum::<f64>() / (outer.len() as f64) < 0.0
                    });
                let latitude_range = outer.iter().fold([90.0_f64, -90.0_f64], |range, point| {
                    [range[0].min(point[1]), range[1].max(point[1])]
                });
                for edge in outer
                    .windows(2)
                    .filter(|edge| (edge[0][0] - edge[1][0]).abs() <= 180.0)
                {
                    coasts.push(Coast {
                        start: edge[0],
                        end: edge[1],
                        region,
                    });
                }
                let mut projected = rings.into_iter().map(|ring| {
                    LineString::from(
                        ring.into_iter()
                            .map(|point| {
                                let point =
                                    polar_projection.map_or(point, |south| polar(point, south));
                                (point[0], point[1])
                            })
                            .collect::<Vec<_>>(),
                    )
                });
                let polygon = Polygon::new(projected.next().unwrap(), projected.collect())
                    .orient(Direction::Default);
                if polar_projection.is_none() {
                    let bounds = polygon.bounding_rect().unwrap();
                    land.push(Indexed {
                        envelope: envelope(bounds.min().into(), bounds.max().into()),
                        index: parts.len(),
                    });
                }
                parts.push(Part {
                    polygon: IntervalTreeMultiPolygon::new(&MultiPolygon(vec![polygon])),
                    region,
                    polar: polar_projection,
                    latitude_range,
                });
            }
        }
        ensure!(!land.is_empty(), "empty country polygons");
        let coastal_index = RTree::bulk_load(
            coasts
                .iter()
                .enumerate()
                .map(|(index, edge)| Indexed {
                    envelope: envelope(edge.start, edge.end),
                    index,
                })
                .collect(),
        );
        Ok(Self {
            identities,
            disputed,
            parts,
            land_index: RTree::bulk_load(land),
            coasts,
            coastal_index,
        })
    }

    pub fn resolve(&self, latitude: f64, longitude: f64, coastal: bool) -> Identity {
        if !latitude.is_finite() || !longitude.is_finite() || latitude.abs() > 90.0 {
            return Identity::default();
        }
        let longitude = (longitude + 180.0).rem_euclid(360.0) - 180.0;
        let point = Coord {
            x: longitude,
            y: latitude,
        };
        let mut containing = self
            .land_index
            .locate_in_envelope_intersecting(&AABB::from_point([longitude, latitude]))
            .filter(|entry| self.parts[entry.index].polygon.contains(&point))
            .map(|entry| self.parts[entry.index].region);
        let first = containing.next();
        if let Some(first) = first
            && containing.any(|other| {
                self.identities[other] != self.identities[first]
                    || (!coastal && self.disputed[other])
            })
        {
            // Overlapping boundary interiors cannot acquire authority from tree traversal order.
            return Identity::default();
        }
        let region = first.or_else(|| {
            self.parts.iter().find_map(|part| {
                let south = part.polar?;
                if !(part.latitude_range[0]..=part.latitude_range[1]).contains(&latitude) {
                    return None;
                }
                let projected = polar([longitude, latitude], south);
                part.polygon
                    .contains(&Coord::from(projected))
                    .then_some(part.region)
            })
        });
        if let Some(region) = region {
            return if !coastal && self.disputed[region] {
                Identity::default()
            } else {
                self.identities[region]
            };
        }
        if !coastal {
            return Identity::default();
        }
        let scale_x = M_PER_DEG_LON * latitude.to_radians().cos();
        let reach_x = COASTAL_BUFFER_M / scale_x.abs().max(1.0);
        let reach_y = COASTAL_BUFFER_M / M_PER_DEG_LAT;
        let mut found = BTreeSet::new();
        for shift in [0.0, -360.0, 360.0] {
            let bounds = AABB::from_corners(
                [longitude + shift - reach_x, latitude - reach_y],
                [longitude + shift + reach_x, latitude + reach_y],
            );
            for candidate in self.coastal_index.locate_in_envelope_intersecting(&bounds) {
                let edge = &self.coasts[candidate.index];
                let delta = |from: f64, to: f64| (to - from + 180.0_f64).rem_euclid(360.0) - 180.0;
                let (px, py) = (
                    delta(edge.start[0], longitude) * scale_x,
                    (latitude - edge.start[1]) * M_PER_DEG_LAT,
                );
                let (bx, by) = (
                    delta(edge.start[0], edge.end[0]) * scale_x,
                    (edge.end[1] - edge.start[1]) * M_PER_DEG_LAT,
                );
                let length = bx * bx + by * by;
                let fraction = if length > 0.0 {
                    ((px * bx + py * by) / length).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                if (px - fraction * bx).hypot(py - fraction * by) <= COASTAL_BUFFER_M {
                    found.insert(edge.region);
                }
            }
        }
        if found.len() == 1 {
            self.identities[*found.first().unwrap()]
        } else {
            Identity::default()
        }
    }
}

pub use super::geography_output::bake;

#[cfg(test)]
#[path = "geography_tests.rs"]
mod tests;

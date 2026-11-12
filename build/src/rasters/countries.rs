//! The year's country polygons on a node window: the country whose land holds each node centre.
use crate::osm::region_input::RegionFile;
use std::{collections::BTreeMap, path::Path};
use tiles::terrain::{NODES_PER_DEGREE, Window};

/// One country's rings (outer rings and holes alike) in degrees, with their bounds.
struct Country {
    code: String,
    rings: Vec<Vec<[f64; 2]>>,
    bounds: [f64; 4],
}

/// Countries in code order; a node's label is its country's index plus one, 0 at sea and on
/// disputed land, which belong to no country.
#[derive(Default)]
pub struct Countries {
    countries: Vec<Country>,
}

impl Countries {
    pub fn read(path: &Path) -> Result<Self, String> {
        let file: RegionFile = serde_json::from_slice(
            &std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?,
        )
        .map_err(|error| format!("{}: {error}", path.display()))?;
        let mut rings = BTreeMap::<String, Vec<Vec<[f64; 2]>>>::new();
        for region in file.regions {
            if let Some(code) = region.country {
                rings
                    .entry(code)
                    .or_default()
                    .extend(region.polygons.into_iter().flatten());
            }
        }
        Ok(Self::new(rings))
    }

    pub(super) fn new(rings: BTreeMap<String, Vec<Vec<[f64; 2]>>>) -> Self {
        let countries = rings
            .into_iter()
            .map(|(code, rings)| {
                let mut bounds = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
                for &[longitude, latitude] in rings.iter().flatten() {
                    bounds = [
                        bounds[0].min(longitude),
                        bounds[1].min(latitude),
                        bounds[2].max(longitude),
                        bounds[3].max(latitude),
                    ];
                }
                Country {
                    code,
                    rings,
                    bounds,
                }
            })
            .collect();
        Self { countries }
    }

    /// The label of a region's country; `None` for a region without a polygon, such as the world.
    pub fn label(&self, region: &str) -> Option<u16> {
        self.countries
            .iter()
            .position(|country| country.code == region)
            .map(|index| index as u16 + 1)
    }

    /// Each node's label, by even-odd crossings of its row: a node holds a country's label when it
    /// lies on or right of an odd crossing, so a row on a vertex counts each edge once.
    pub fn labels(&self, window: Window) -> Vec<u16> {
        let (rows, columns) = (window.rows as usize, window.columns as usize);
        let density = f64::from(NODES_PER_DEGREE);
        let [west, south, east, north] = super::warp::bounds(window);
        let mut labels = vec![0; rows * columns];
        let mut crossings = vec![Vec::new(); rows];
        for (index, country) in self.countries.iter().enumerate() {
            // Polygons stop at ±180°; a window past the antimeridian reads their copies 360° away.
            for shift in [-360.0, 0.0, 360.0] {
                let [left, bottom, right, top] = country.bounds;
                if bottom > north || top < south || left + shift > east || right + shift < west {
                    continue;
                }
                for ring in &country.rings {
                    for (vertex, &[x0, y0]) in ring.iter().enumerate() {
                        let [x1, y1] = ring[(vertex + 1) % ring.len()];
                        // The rows whose node latitude lies in [lower, upper) of this edge.
                        let (lower, upper) = (y0.min(y1) * density, y0.max(y1) * density);
                        let first = (f64::from(window.north_node) - upper).floor() as i64 + 1;
                        let last = (f64::from(window.north_node) - lower).floor() as i64;
                        for row in first.max(0)..=last.min(rows as i64 - 1) {
                            let latitude = (f64::from(window.north_node) - row as f64) / density;
                            let longitude = x0 + (latitude - y0) * (x1 - x0) / (y1 - y0) + shift;
                            crossings[row as usize]
                                .push(longitude * density - f64::from(window.west_node));
                        }
                    }
                }
            }
            for (row, line) in crossings.iter_mut().enumerate() {
                line.sort_by(f64::total_cmp);
                for pair in line.chunks_exact(2) {
                    let [from, to] =
                        [pair[0], pair[1]].map(|x| x.ceil().clamp(0.0, columns as f64));
                    labels[row * columns + from as usize..row * columns + to as usize]
                        .fill(index as u16 + 1);
                }
                line.clear();
            }
        }
        labels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(west: f64, south: f64, east: f64, north: f64) -> Vec<[f64; 2]> {
        vec![
            [west, south],
            [east, south],
            [east, north],
            [west, north],
            [west, south],
        ]
    }

    #[test]
    fn node_centres_take_their_country_with_holes_edges_once_and_copies_across_the_antimeridian() {
        let degree = 1.0 / f64::from(NODES_PER_DEGREE);
        // Country A: nodes 0..4 east and 0..4 north, less a hole over node (2, 2); country B east of
        // it shares A's edge at node 4, and C sits on the far side of the antimeridian.
        let a = vec![
            square(0.0, 0.0, 4.0 * degree, 4.0 * degree),
            square(1.5 * degree, 1.5 * degree, 2.5 * degree, 2.5 * degree),
        ];
        let b = vec![square(4.0 * degree, 0.0, 6.0 * degree, 4.0 * degree)];
        let c = vec![square(-180.0, 0.0, -180.0 + 2.0 * degree, 4.0 * degree)];
        let countries = Countries::new(BTreeMap::from([
            ("A".into(), a),
            ("B".into(), b),
            ("C".into(), c),
        ]));
        assert_eq!(
            (countries.label("B"), countries.label("world")),
            (Some(2), None)
        );
        let window = Window {
            north_node: 4,
            west_node: -1,
            rows: 6,
            columns: 8,
        };
        let labels = countries.labels(window);
        let row = |r: usize| labels[r * 8..(r + 1) * 8].to_vec();
        // Latitude 4 is A's and B's northern edge: half-open, it belongs to the row above.
        assert_eq!(row(0), [0; 8]);
        assert_eq!(row(1), [0, 1, 1, 1, 1, 2, 2, 0]);
        assert_eq!(row(2), [0, 1, 1, 0, 1, 2, 2, 0]);
        assert_eq!(row(4), [0, 1, 1, 1, 1, 2, 2, 0]);
        assert_eq!(row(5), [0; 8]);
        let across = Window {
            north_node: 2,
            west_node: 180 * NODES_PER_DEGREE - 1,
            rows: 1,
            columns: 4,
        };
        assert_eq!(countries.labels(across), [0, 3, 3, 0]);
    }
}

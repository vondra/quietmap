//! The tiles a click has read so far, in the click's metre frame: the ground under any point and
//! the profile of a ray, a vertex wherever it crosses a line of the terrain lattice.

use physics::profile::Profile;
use tiles::geo::{LocalFrame, Mercator, TILES_PER_AXIS, TileId};
use tiles::terrain::{
    GroundSample, HEIGHT_MISSING, NODES_PER_DEGREE, Node, OCEAN, PERCENT_MAX, Terrain,
    height_m_of_code,
};

/// Lattice columns around the globe.
const COLUMNS_AROUND: i64 = 360 * NODES_PER_DEGREE as i64;

/// Terrain of every read tile within `radius` rings of the clicked tile, by offset.
pub struct Ground<'a> {
    pub frame: LocalFrame,
    centre: TileId,
    radius: i64,
    /// `None`: not read (an error to sample); `Some(None)`: read, absent (ocean).
    tiles: Vec<Option<Option<Terrain<'a>>>>,
    /// Web Mercator y of every lattice row over the scene's tiles, the northernmost first: row
    /// `north_row - k` (its latitude times 3600) at index k.
    row_y: Vec<f64>,
    north_row: i64,
}

impl<'a> Ground<'a> {
    pub fn new(frame: LocalFrame, centre: TileId, radius: u32) -> Self {
        let side = 2 * radius as usize + 1;
        let latitude_at = |tile_y: i64| {
            let y = tile_y.clamp(0, i64::from(TILES_PER_AXIS)) as f64;
            Mercator { x: 0.0, y }.to_degrees().0
        };
        let nodes = f64::from(NODES_PER_DEGREE);
        let (top, bottom) = (
            i64::from(centre.y) - i64::from(radius),
            i64::from(centre.y) + i64::from(radius) + 1,
        );
        let north_row = (latitude_at(top) * nodes).ceil() as i64;
        let south_row = (latitude_at(bottom) * nodes).floor() as i64;
        let row_y = (south_row..=north_row)
            .rev()
            .map(|row| Mercator::from_degrees(row as f64 / nodes, 0.0).y)
            .collect();
        Ground {
            frame,
            centre,
            radius: i64::from(radius),
            tiles: (0..side * side).map(|_| None).collect(),
            row_y,
            north_row,
        }
    }

    fn slot(&self, tile: TileId) -> Option<usize> {
        let n = i64::from(TILES_PER_AXIS);
        let dx = (i64::from(tile.x) - i64::from(self.centre.x) + n / 2).rem_euclid(n) - n / 2;
        let dy = i64::from(tile.y) - i64::from(self.centre.y);
        if dx.abs() > self.radius || dy.abs() > self.radius {
            return None;
        }
        Some(((dy + self.radius) * (2 * self.radius + 1) + dx + self.radius) as usize)
    }

    /// Records a read tile's terrain (`None` for an absent file: ocean).
    pub fn insert(&mut self, tile: TileId, terrain: Option<Terrain<'a>>) {
        let slot = self
            .slot(tile)
            .expect("a read tile lies within the scene radius");
        self.tiles[slot] = Some(terrain);
    }

    /// The tile a walk's vertex reads: the one containing it, or, for a vertex exactly on its north
    /// or west edge (the equator, every 32nd tile meridian: lattice lines), the land tile across
    /// that edge when the containing one is ocean; the land tile holds the seam's nodes.
    fn land_tile_at(&self, position: Mercator) -> Result<TileId, String> {
        let id = TileId::containing(position);
        if self.terrain_of(id)?.is_some() {
            return Ok(id);
        }
        let across = [
            (position.y == f64::from(id.y) && id.y > 0).then(|| TileId { y: id.y - 1, ..id }),
            (position.x.rem_euclid(f64::from(TILES_PER_AXIS)) == f64::from(id.x)).then(|| TileId {
                x: (id.x + TILES_PER_AXIS - 1) % TILES_PER_AXIS,
                ..id
            }),
        ];
        Ok(across
            .into_iter()
            .flatten()
            .find(|tile| matches!(self.terrain_of(*tile), Ok(Some(_))))
            .unwrap_or(id))
    }

    /// The terrain of a read tile: `Some(None)` for ocean, an error for a tile not read.
    fn terrain_of(&self, tile: TileId) -> Result<Option<&Terrain<'a>>, String> {
        self.slot(tile)
            .and_then(|slot| self.tiles[slot].as_ref())
            .map(Option::as_ref)
            .ok_or_else(|| format!("terrain of tile {tile:?} not read"))
    }

    /// The ground at [east, north] metres; an error outside the read tiles or at a node without
    /// data (the click fails instead of answering quieter).
    pub fn at(&self, metres: [f64; 2]) -> Result<GroundSample, String> {
        let position = self.frame.to_mercator(metres).snapped_to_lattice();
        let Some(terrain) = self.terrain_of(TileId::containing(position))? else {
            return Ok(OCEAN);
        };
        terrain.sample(position).ok_or_else(|| {
            let (lat, lon) = position.to_degrees();
            format!("no terrain data at {lat:.6},{lon:.6}")
        })
    }

    /// The terrain height at `metres`, `None` where no tile is read yet or a node has no data
    /// (the aircraft horizons skip such samples; a ray never meets them).
    pub fn read_height_m(&self, metres: [f64; 2]) -> Option<f64> {
        let position = self.frame.to_mercator(metres).snapped_to_lattice();
        let terrain = self
            .slot(TileId::containing(position))
            .and_then(|slot| self.tiles[slot].as_ref())?;
        match terrain {
            Some(terrain) => terrain.sample(position).map(|sample| sample.height_m),
            None => Some(OCEAN.height_m),
        }
    }

    /// The ground under the ray from `source` to `receiver` (click metres) at both ends and
    /// wherever the ray crosses a row or a column of the lattice, in order from the source. A
    /// vertex on a line reads that line's two nodes either side of it.
    pub fn fill_profile(
        &self,
        source: [f64; 2],
        receiver: [f64; 2],
        profile: &mut Profile,
    ) -> Result<(), String> {
        let offset = [source[0] - receiver[0], source[1] - receiver[1]];
        profile.clear(offset[0].hypot(offset[1]).max(1.0));
        let start = self.at(source)?;
        profile.push(0.0, start.height_m, start.ground_factor);
        let (a, b) = (
            self.frame.to_mercator(source),
            self.frame.to_mercator(receiver),
        );
        let columns_per_unit = COLUMNS_AROUND as f64 / f64::from(TILES_PER_AXIS);
        let column_of = |x: f64| x * columns_per_unit - (COLUMNS_AROUND / 2) as f64;
        let (column_a, column_b) = (column_of(a.x), column_of(b.x));
        let (columns, rise) = (column_b - column_a, b.y - a.y);
        let rows = &self.row_y;
        // The next column line strictly ahead, in the direction of travel.
        let (mut next_column, column_step) = if columns > 0.0 {
            (column_a.floor() + 1.0, 1.0)
        } else {
            (column_a.ceil() - 1.0, -1.0)
        };
        // Row lines ascend southward in `rows`; `north` is the index of the row at or north of
        // the point, `next_row` the next row line strictly ahead.
        // A start exactly on a row line belongs to the stretch the ray heads into: the row is
        // north of it heading south, south of it heading north.
        let north_of_start = if rise > 0.0 {
            rows.partition_point(|&y| y <= a.y)
        } else {
            rows.partition_point(|&y| y < a.y)
        };
        let mut north = north_of_start
            .checked_sub(1)
            .ok_or("the ray starts north of the scene")?;
        let mut next_row = if rise > 0.0 {
            Some(north_of_start)
        } else {
            north_of_start.checked_sub(1)
        };
        let mut tile: Option<(TileId, Option<&Terrain<'a>>)> = None;
        loop {
            let t_column = if columns != 0.0 {
                (next_column - column_a) / columns
            } else {
                f64::INFINITY
            };
            let t_row = match next_row.and_then(|k| rows.get(k)) {
                Some(&y) if rise != 0.0 => (y - a.y) / rise,
                _ => f64::INFINITY,
            };
            let t = t_column.min(t_row);
            if t >= 1.0 {
                break;
            }
            let position = Mercator {
                x: a.x + t * (b.x - a.x),
                y: a.y + t * rise,
            };
            let id = self.land_tile_at(position)?;
            if tile.is_none_or(|(cached, _)| cached != id) {
                tile = Some((id, self.terrain_of(id)?));
            }
            let terrain = tile.and_then(|(_, terrain)| terrain);
            let sample = match terrain {
                None => Some(OCEAN),
                Some(terrain) if t_row <= t_column => {
                    let k = next_row.expect("a row line ahead");
                    let column = column_of(position.x);
                    let west = column.floor();
                    let row = self.north_row - k as i64;
                    edge_sample(
                        node(terrain, row, west as i64),
                        node(terrain, row, west as i64 + 1),
                        column - west,
                    )
                }
                Some(terrain) => {
                    let (y0, y1) = (
                        rows[north],
                        *rows
                            .get(north + 1)
                            .ok_or("the ray leaves the scene southward")?,
                    );
                    let row = self.north_row - north as i64;
                    edge_sample(
                        node(terrain, row, next_column as i64),
                        node(terrain, row - 1, next_column as i64),
                        ((position.y - y0) / (y1 - y0)).clamp(0.0, 1.0),
                    )
                }
            }
            .ok_or_else(|| {
                let (lat, lon) = position.to_degrees();
                format!("no terrain data at {lat:.6},{lon:.6}")
            })?;
            profile.push(t, sample.height_m, sample.ground_factor);
            if t_row <= t_column {
                let k = next_row.expect("a row line ahead");
                if rise > 0.0 {
                    north = k;
                    next_row = Some(k + 1);
                } else {
                    north = k
                        .checked_sub(1)
                        .ok_or("the ray leaves the scene northward")?;
                    next_row = k.checked_sub(1);
                }
            }
            if t_column <= t_row {
                next_column += column_step;
            }
        }
        let end = self.at(receiver)?;
        profile.push(1.0, end.height_m, end.ground_factor);
        Ok(())
    }
}

/// The node of `terrain` at global lattice `row` (latitude times 3600) and `column` (longitude
/// times 3600, any turn of the globe), or `None` outside its window.
fn node(terrain: &Terrain, row: i64, column: i64) -> Option<Node> {
    let window = terrain.window();
    let local_row = i64::from(window.north_node) - row;
    let local_column = (column - i64::from(window.west_node)).rem_euclid(COLUMNS_AROUND);
    (local_row >= 0
        && local_row < i64::from(window.rows)
        && local_column < i64::from(window.columns))
    .then(|| terrain.node(local_row as u32, local_column as u32))
}

/// The ground a fraction `f` of the way from node `a` to node `b`: height and ground factor
/// linear, forest cover from the nearer node; `None` where a node it weighs is missing or has no
/// data. A node of no weight is not read: on a node or a tile seam the vertex may lie in the tile
/// that lacks its zero-weight neighbour.
fn edge_sample(a: Option<Node>, b: Option<Node>, f: f64) -> Option<GroundSample> {
    let (a, b) = match (a, b) {
        (Some(a), Some(b)) => (a, b),
        (Some(a), None) if f <= 0.0 => (a, a),
        (None, Some(b)) if f >= 1.0 => (b, b),
        _ => return None,
    };
    if [a, b]
        .iter()
        .any(|node| node.height_code == HEIGHT_MISSING || node.impervious_percent > PERCENT_MAX)
    {
        return None;
    }
    let lerp = |p: f64, q: f64| p + f * (q - p);
    let nearer = if f < 0.5 { a } else { b };
    Some(GroundSample {
        height_m: lerp(
            height_m_of_code(a.height_code),
            height_m_of_code(b.height_code),
        ),
        ground_factor: 1.0
            - lerp(
                f64::from(a.impervious_percent),
                f64::from(b.impervious_percent),
            ) / 100.0,
        forest_cover: f64::from(nearer.forest_percent.min(PERCENT_MAX)) / 100.0,
    })
}

#[cfg(test)]
#[path = "scene_tests.rs"]
mod tests;

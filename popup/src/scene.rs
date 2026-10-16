//! The tiles a click has read so far, in the click's metre frame: the ground under any point and
//! the profile of a ray, a vertex wherever it crosses a line of the terrain lattice: the 1/6"
//! lattice within `FINE_ENDS_M` of either end where a tile under that stretch has fine terrain
//! (its detail read when first needed, added to the 1" heights), the one arc-second lattice
//! elsewhere.

use physics::profile::{FINE_ENDS_M, Profile};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use tiles::Kind;
use tiles::geo::{LocalFrame, Mercator, TILES_PER_AXIS, TileId};
use tiles::square::read_tile;
use tiles::terrain::{
    GroundSample, HEIGHT_MISSING, NODES_PER_DEGREE, Node, NodePosition, OCEAN, PERCENT_MAX,
    Terrain, height_m_of_code,
};
use tiles::terrain_fine::{FINE_PER_NODE, FineTerrain, fine_window};

/// Lattice columns around the globe.
const COLUMNS_AROUND: i64 = 360 * NODES_PER_DEGREE as i64;

/// Terrain of every read tile within `radius` rings of the clicked tile, by offset.
pub struct Ground {
    pub frame: LocalFrame,
    centre: TileId,
    radius: i64,
    /// `None`: not read (an error to sample); `Some(None)`: read, absent (ocean).
    tiles: Vec<Option<Option<Terrain>>>,
    /// The year root the fine terrain is read from when a ray first needs a tile's (`None`: the
    /// one arc-second lattice alone).
    fine_root: Option<PathBuf>,
    /// Each slot's fine terrain once read, `None` inside for a tile without one.
    fine: Vec<OnceLock<Result<Option<FineTerrain>, String>>>,
    /// Fine terrain files and bytes read.
    fine_read: [AtomicU64; 2],
    /// Web Mercator y of every lattice row over the scene's tiles, the northernmost first: row
    /// `north_row - k` (its latitude times 3600) at index k.
    row_y: Vec<f64>,
    north_row: i64,
}

impl Ground {
    pub fn new(frame: LocalFrame, centre: TileId, radius: u32, fine_root: Option<PathBuf>) -> Self {
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
            fine_root,
            fine: (0..side * side).map(|_| OnceLock::new()).collect(),
            fine_read: [AtomicU64::new(0), AtomicU64::new(0)],
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
    pub fn insert(&mut self, tile: TileId, terrain: Option<Terrain>) {
        let slot = self
            .slot(tile)
            .expect("a read tile lies within the scene radius");
        self.tiles[slot] = Some(terrain);
    }

    /// The fine terrain tiles and bytes read so far.
    pub fn fine_read(&self) -> (u64, u64) {
        let [files, bytes] = &self.fine_read;
        (files.load(Ordering::Relaxed), bytes.load(Ordering::Relaxed))
    }

    /// The fine terrain of a tile of the scene, read whole on first use; `None` where it has none.
    /// Its window must be its `terrain` tile's, six times as fine.
    pub fn fine_of(&self, tile: TileId) -> Result<Option<&FineTerrain>, String> {
        let Some(root) = &self.fine_root else {
            return Ok(None);
        };
        let slot = self
            .slot(tile)
            .ok_or_else(|| format!("fine terrain of tile {tile:?} outside the scene"))?;
        self.fine[slot]
            .get_or_init(|| {
                let Some(bytes) = read_tile(root, tile, Kind::TerrainFine)? else {
                    return Ok(None);
                };
                self.fine_read[0].fetch_add(1, Ordering::Relaxed);
                self.fine_read[1].fetch_add(bytes.len() as u64, Ordering::Relaxed);
                let fine = FineTerrain::parse(bytes)
                    .map_err(|error| format!("fine terrain of {tile:?}: {error}"))?;
                let coarse = self
                    .terrain_of(tile)?
                    .ok_or_else(|| format!("fine terrain of {tile:?} over the ocean"))?;
                if fine.window() != fine_window(coarse.window()) {
                    return Err(format!(
                        "fine terrain of {tile:?}: not its terrain tile's window"
                    ));
                }
                Ok(Some(fine))
            })
            .as_ref()
            .map(Option::as_ref)
            .map_err(Clone::clone)
    }

    /// The tile a walk's vertex reads: the one containing it, or, for a vertex exactly on its north
    /// or west edge (the equator; a tile meridian, every 32nd a 1" column, every 16th a 1/6" one),
    /// the land tile across that edge when the containing one is ocean; the land tile holds the
    /// seam's nodes.
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
    fn terrain_of(&self, tile: TileId) -> Result<Option<&Terrain>, String> {
        self.slot(tile)
            .and_then(|slot| self.tiles[slot].as_ref())
            .map(Option::as_ref)
            .ok_or_else(|| format!("terrain of tile {tile:?} not read"))
    }

    /// The ground at [east, north] metres as a ray's end reads it, the tile's fine detail added to
    /// its height where it has one; an error outside the read tiles or at a node without data (the
    /// click fails instead of answering quieter).
    pub fn at(&self, metres: [f64; 2]) -> Result<GroundSample, String> {
        let position = self.frame.to_mercator(metres).snapped_to_lattice();
        let tile = TileId::containing(position);
        let Some(terrain) = self.terrain_of(tile)? else {
            return Ok(OCEAN);
        };
        let place = terrain
            .locate(position)
            .ok_or_else(|| no_data_at(position))?;
        let mut sample = terrain
            .sample_at(place)
            .ok_or_else(|| no_data_at(position))?;
        sample.height_m += self.detail_at(tile, place)?;
        Ok(sample)
    }

    /// The fine detail (m) of `tile` at a place among its 1" nodes, 0 where it has none.
    fn detail_at(&self, tile: TileId, place: NodePosition) -> Result<f64, String> {
        let Some(fine) = self.fine_of(tile)? else {
            return Ok(0.0);
        };
        let per = f64::from(FINE_PER_NODE);
        fine.at(
            per * (f64::from(place.row) + place.row_fraction),
            per * (f64::from(place.column) + place.column_fraction),
        )
        .map_err(|error| error.to_string())
    }

    /// The one arc-second ground at [east, north] metres, its fine detail left out: what the walk
    /// reads away from a ray's ends.
    #[cfg(test)]
    pub fn coarse_at(&self, metres: [f64; 2]) -> Result<GroundSample, String> {
        let position = self.frame.to_mercator(metres).snapped_to_lattice();
        let Some(terrain) = self.terrain_of(TileId::containing(position))? else {
            return Ok(OCEAN);
        };
        terrain.sample(position).ok_or_else(|| no_data_at(position))
    }

    /// The terrain height at `metres`, its fine detail added, `None` where no tile is read yet or a
    /// node has no data (the aircraft horizons skip such samples; a ray never meets them); an error
    /// where a fine file fails.
    pub fn read_height_m(&self, metres: [f64; 2]) -> Result<Option<f64>, String> {
        let position = self.frame.to_mercator(metres).snapped_to_lattice();
        let tile = TileId::containing(position);
        let Some(terrain) = self.slot(tile).and_then(|slot| self.tiles[slot].as_ref()) else {
            return Ok(None);
        };
        let Some(terrain) = terrain else {
            return Ok(Some(OCEAN.height_m));
        };
        let Some((place, sample)) = terrain
            .locate(position)
            .and_then(|place| Some((place, terrain.sample_at(place)?)))
        else {
            return Ok(None);
        };
        Ok(Some(sample.height_m + self.detail_at(tile, place)?))
    }

    /// Whether a tile under the stretch from `a` to `b` has fine terrain: the tiles of its box's
    /// corners, as a stretch of at most 200 m lies in two by two tiles at most. A tile without a
    /// file under a fine stretch adds no detail, its 1" surface read on the fine lattice.
    fn fine_near(&self, a: Mercator, b: Mercator) -> Result<bool, String> {
        for x in [a.x.min(b.x), a.x.max(b.x)] {
            for y in [a.y.min(b.y), a.y.max(b.y)] {
                if self
                    .fine_of(TileId::containing(Mercator { x, y }))?
                    .is_some()
                {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// The ground under the ray from `source` to `receiver` (click metres) at both ends and
    /// wherever the ray crosses a row or a column of the lattice, in order from the source: the
    /// 1/6" lattice within `FINE_ENDS_M` of either end where a tile under that stretch has fine
    /// terrain, with a vertex where the lattice changes (no line of the coarser one), the one
    /// arc-second lattice elsewhere. A vertex on a line reads that line's two nodes either side.
    pub fn fill_profile(
        &self,
        source: [f64; 2],
        receiver: [f64; 2],
        profile: &mut Profile,
    ) -> Result<(), String> {
        let offset = [source[0] - receiver[0], source[1] - receiver[1]];
        let length_m = offset[0].hypot(offset[1]);
        profile.clear(length_m.max(1.0));
        let start = self.at(source)?;
        push_sample(profile, 0.0, &start);
        let (a, b) = (
            self.frame.to_mercator(source),
            self.frame.to_mercator(receiver),
        );
        let along = |t: f64| Mercator {
            x: a.x + t * (b.x - a.x),
            y: a.y + t * (b.y - a.y),
        };
        let scale_of = |fine: bool| if fine { FINE_PER_NODE as usize } else { 1 };
        // The stretches as (from, to, lattice scale), neighbours on one lattice joined.
        let mut stretches = [(0.0, 1.0, 1); 3];
        let count = if self.fine_root.is_none() {
            1
        } else if length_m <= 2.0 * FINE_ENDS_M {
            stretches[0].2 = scale_of(self.fine_near(a, b)?);
            1
        } else {
            let end = FINE_ENDS_M / length_m;
            let mut count = 0;
            for (from, to, scale) in [
                (0.0, end, scale_of(self.fine_near(a, along(end))?)),
                (end, 1.0 - end, 1),
                (
                    1.0 - end,
                    1.0,
                    scale_of(self.fine_near(along(1.0 - end), b)?),
                ),
            ] {
                if count > 0 && stretches[count - 1].2 == scale {
                    stretches[count - 1].1 = to;
                } else {
                    stretches[count] = (from, to, scale);
                    count += 1;
                }
            }
            count
        };
        for &(from, to, scale) in &stretches[..count] {
            self.walk(scale, a, b, from, to, profile)?;
            if to < 1.0 {
                let switch = self.at([source[0] - to * offset[0], source[1] - to * offset[1]])?;
                push_sample(profile, to, &switch);
            }
        }
        let end = self.at(receiver)?;
        push_sample(profile, 1.0, &end);
        Ok(())
    }

    /// Mercator y of row line `k` of the lattice `scale` times as fine as the one arc-second, the
    /// northernmost 0: on the 1" rows, evenly in y between them (latitude is linear in y there to
    /// a millionth of the spacing).
    fn row_line_y(&self, scale: usize, k: usize) -> f64 {
        let (row, sub) = (k / scale, k % scale);
        if sub == 0 {
            return self.row_y[row];
        }
        let (y0, y1) = (self.row_y[row], self.row_y[row + 1]);
        y0 + (y1 - y0) * sub as f64 / scale as f64
    }

    /// The number of row lines of the lattice `scale` north of `y`, those on it too when `or_on`.
    fn rows_north_of(&self, scale: usize, y: f64, or_on: bool) -> usize {
        let (mut low, mut high) = (0, scale * (self.row_y.len() - 1) + 1);
        while low < high {
            let middle = (low + high) / 2;
            let line_y = self.row_line_y(scale, middle);
            if line_y < y || (or_on && line_y == y) {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        low
    }

    /// Appends the ground wherever the ray a -> b crosses a line of the lattice `scale` times as
    /// fine as the one arc-second, strictly between fractions `from` and `to`, in order.
    fn walk(
        &self,
        scale: usize,
        a: Mercator,
        b: Mercator,
        from: f64,
        to: f64,
        profile: &mut Profile,
    ) -> Result<(), String> {
        let around = scale as i64 * COLUMNS_AROUND;
        let columns_per_unit = around as f64 / f64::from(TILES_PER_AXIS);
        let column_of = |x: f64| x * columns_per_unit - (around / 2) as f64;
        let (column_a, column_b) = (column_of(a.x), column_of(b.x));
        let (columns, rise) = (column_b - column_a, b.y - a.y);
        let lines = scale * (self.row_y.len() - 1) + 1;
        // The next column line strictly ahead, in the direction of travel.
        let column_from = column_a + from * columns;
        let (mut next_column, column_step) = if columns > 0.0 {
            (column_from.floor() + 1.0, 1.0)
        } else {
            (column_from.ceil() - 1.0, -1.0)
        };
        // Row lines ascend southward; `north` is the index of the row at or north of the point,
        // `next_row` the next row line strictly ahead. A start exactly on a row line belongs to
        // the stretch the ray heads into: the row is north of it heading south, south of it
        // heading north.
        let north_of_start = self.rows_north_of(scale, a.y + from * rise, rise > 0.0);
        let mut north = north_of_start
            .checked_sub(1)
            .ok_or("the ray starts north of the scene")?;
        let mut next_row = if rise > 0.0 {
            Some(north_of_start)
        } else {
            north_of_start.checked_sub(1)
        };
        let mut tile: Option<(TileId, Option<&Terrain>)> = None;
        loop {
            let t_column = if columns != 0.0 {
                (next_column - column_a) / columns
            } else {
                f64::INFINITY
            };
            let t_row = match next_row.filter(|&k| k < lines) {
                Some(k) if rise != 0.0 => (self.row_line_y(scale, k) - a.y) / rise,
                _ => f64::INFINITY,
            };
            let t = t_column.min(t_row);
            if t >= to {
                break;
            }
            let position = Mercator {
                x: a.x + t * (b.x - a.x),
                y: a.y + t * rise,
            };
            // The vertex's line: its first node (global row and column on this lattice), whether
            // the second lies a row south (`down`) or a column east, and the fraction between.
            let (row, column, down, fraction) = if t_row <= t_column {
                let k = next_row.expect("a row line ahead");
                let west = column_of(position.x).floor();
                let row = scale as i64 * self.north_row - k as i64;
                (row, west as i64, false, column_of(position.x) - west)
            } else {
                if north + 1 >= lines {
                    return Err("the ray leaves the scene southward".into());
                }
                let (y0, y1) = (
                    self.row_line_y(scale, north),
                    self.row_line_y(scale, north + 1),
                );
                let row = scale as i64 * self.north_row - north as i64;
                let fraction = ((position.y - y0) / (y1 - y0)).clamp(0.0, 1.0);
                (row, next_column as i64, true, fraction)
            };
            let sample = if scale == 1 {
                let id = self.land_tile_at(position)?;
                if tile.is_none_or(|(cached, _)| cached != id) {
                    tile = Some((id, self.terrain_of(id)?));
                }
                match tile.and_then(|(_, terrain)| terrain) {
                    None => Some(OCEAN),
                    Some(terrain) => {
                        let second = if down {
                            node(terrain, row - 1, column)
                        } else {
                            node(terrain, row, column + 1)
                        };
                        edge_sample(node(terrain, row, column), second, fraction)
                    }
                }
            } else {
                self.fine_sample(position, row, column, down, fraction)?
            }
            .ok_or_else(|| no_data_at(position))?;
            push_sample(profile, t, &sample);
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
        Ok(())
    }

    /// The ground at a vertex of the fine walk (global 1/6" `row` and `column` of its line's first
    /// node, the second a row south when `down`, else a column east, `fraction` the way between):
    /// the 1" tile's bilinear height, ground factor and trees at the vertex's place in its 1"
    /// cell, taken from its fine indices, plus the fine detail on the line (a node of no weight
    /// not read); `None` where a 1" node it weighs has no data. The fine window's corner is the 1"
    /// window's, so a fine index over six is a 1" one.
    fn fine_sample(
        &self,
        position: Mercator,
        row: i64,
        column: i64,
        down: bool,
        fraction: f64,
    ) -> Result<Option<GroundSample>, String> {
        let tile = self.land_tile_at(position)?;
        let Some(terrain) = self.terrain_of(tile)? else {
            return Ok(Some(OCEAN));
        };
        let window = fine_window(terrain.window());
        let around = i64::from(FINE_PER_NODE) * COLUMNS_AROUND;
        let local_row = i64::from(window.north_node) - row;
        let local_column = (column - i64::from(window.west_node)).rem_euclid(around);
        if local_row < 0
            || local_row >= i64::from(window.rows)
            || local_column >= i64::from(window.columns)
        {
            return Err(format!("a fine vertex outside tile {tile:?}"));
        }
        let (local_row, local_column) = (local_row as u32, local_column as u32);
        // The vertex's place in its 1" cell: on its own line exactly, along it by the fraction (so
        // a vertex on a cell's midline takes the same trees here and on the card).
        let (fine_row, fine_column) = if down {
            (f64::from(local_row) + fraction, f64::from(local_column))
        } else {
            (f64::from(local_row), f64::from(local_column) + fraction)
        };
        let per = f64::from(FINE_PER_NODE);
        let (row_position, column_position) = (fine_row / per, fine_column / per);
        let place = NodePosition {
            row: row_position.floor() as u32,
            row_fraction: row_position - row_position.floor(),
            column: column_position.floor() as u32,
            column_fraction: column_position - column_position.floor(),
        };
        let Some(mut sample) = terrain.sample_at(place) else {
            return Ok(None);
        };
        if let Some(fine) = self.fine_of(tile)? {
            sample.height_m += fine
                .on_line(local_row, local_column, down, fraction)
                .map_err(|error| error.to_string())?;
        }
        Ok(Some(sample))
    }
}

fn no_data_at(position: Mercator) -> String {
    let (lat, lon) = position.to_degrees();
    format!("no terrain data at {lat:.6},{lon:.6}")
}

fn push_sample(profile: &mut Profile, t: f64, sample: &GroundSample) {
    profile.push(
        t,
        sample.height_m,
        sample.ground_factor,
        sample.forest_cover,
        sample.canopy_m,
    );
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
/// linear, the trees (cover and crown height) from the nearer node; `None` where a node it weighs
/// is missing or has no data, or the nearer one has no forest data (as `Terrain::sample` refuses).
/// A node of no weight is not read: on a node or a tile seam the vertex may lie in the tile that
/// lacks its zero-weight neighbour.
fn edge_sample(a: Option<Node>, b: Option<Node>, f: f64) -> Option<GroundSample> {
    let (a, b) = match (a, b) {
        (Some(a), Some(b)) => (a, b),
        (Some(a), None) if f <= 0.0 => (a, a),
        (None, Some(b)) if f >= 1.0 => (b, b),
        _ => return None,
    };
    let nearer = if f < 0.5 { a } else { b };
    if nearer.forest_percent > PERCENT_MAX
        || [a, b]
            .iter()
            .any(|node| node.height_code == HEIGHT_MISSING || node.impervious_percent > PERCENT_MAX)
    {
        return None;
    }
    let lerp = |p: f64, q: f64| p + f * (q - p);
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
        forest_cover: f64::from(nearer.forest_percent) / 100.0,
        canopy_m: f64::from(nearer.canopy_m),
    })
}

#[cfg(test)]
#[path = "scene_tests.rs"]
mod tests;

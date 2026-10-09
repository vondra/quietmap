//! One z12 square's neighbourhood, read and parsed once for all its pixels: the ground and the
//! obstacles to the ground reach of its farthest pixel, every ground source that can reach one of
//! its pixels, indexed by place, and the aircraft boxes to the aircraft reach.

use physics::bands::{PERIODS, lden_energy};
use physics::bound::receiver_bound;
use physics::line::{LINE_PERPENDICULAR_FLOOR_M, POINT_DIVERGENCE_LINEAR};
use physics::weather::{PlaceWeather, WeatherTable};
use popup::aircraft::FINE_BOXES_WITHIN_M;
use popup::aircraft::boxes::AIRCRAFT_REACH_M;
use popup::candidates::{Attributes, Candidate, GROUND_REACH_M, collect};
use popup::obstacles::Scene;
use popup::release::{Release, RingFiles};
use popup::scene::Ground;
use tiles::Kind;
use tiles::aircraft::Aircraft;
use tiles::geo::{LocalFrame, Mercator, TileId};
use tiles::obstacles::Obstacles;
use tiles::sources::Sources;
use tiles::terrain::Terrain;

/// The largest gain over free field a ray can have (FAVOURABLE_GAIN_BOUND_DB, linear).
const GAIN_BOUND: f64 = 63.095_734_448_019_33;
/// Side of the index's cells (m).
const INDEX_CELL_M: f64 = 100.0;

/// The files of a square's neighbourhood.
pub struct Files {
    rings: RingFiles,
    ground_rings: u32,
}

/// The fewest rings around `tile` whose block covers `reach_m` beyond the tile's every edge.
fn rings_for(frame: &LocalFrame, reach_m: f64) -> u32 {
    (reach_m / frame.east_m_per_unit.min(frame.north_m_per_unit)).ceil() as u32
}

/// Distance (m) from `position` (frame metres) to the rectangle of `tile`.
pub fn gap_to_tile(frame: &LocalFrame, tile: TileId, position: [f64; 2]) -> f64 {
    let corner = |dx: f64, dy: f64| {
        frame.to_metres(Mercator {
            x: f64::from(tile.x) + dx,
            y: f64::from(tile.y) + dy,
        })
    };
    let (a, b) = (corner(0.0, 0.0), corner(1.0, 1.0));
    let gap = |value: f64, low: f64, high: f64| {
        let (low, high) = (low.min(high), low.max(high));
        (low - value).max(value - high).max(0.0)
    };
    gap(position[0], a[0], b[0]).hypot(gap(position[1], a[1], b[1]))
}

impl Files {
    /// Reads the ground kinds to the ground reach of the square's every pixel and the aircraft
    /// boxes to theirs: the fine boxes of tiles within their distance of the square, the far ones
    /// of every tile.
    pub fn read(release: &Release, tile: TileId) -> Result<Self, String> {
        let frame = LocalFrame::at(tile.centre());
        let ground_rings = rings_for(&frame, GROUND_REACH_M);
        let aircraft_rings = rings_for(&frame, AIRCRAFT_REACH_M);
        let tiles: Vec<TileId> = (0..=aircraft_rings).flat_map(|k| tile.ring(k)).collect();
        let ring_of = |other: TileId| {
            let n = i64::from(tiles::geo::TILES_PER_AXIS);
            let dx = (i64::from(other.x) - i64::from(tile.x) + n / 2).rem_euclid(n) - n / 2;
            let dy = i64::from(other.y) - i64::from(tile.y);
            dx.unsigned_abs().max(dy.unsigned_abs()) as u32
        };
        let square_gap = |other: TileId| {
            // The tile's distance from the square, across the antimeridian too.
            let n = i64::from(tiles::geo::TILES_PER_AXIS);
            let dx = (i64::from(other.x) - i64::from(tile.x) + n / 2).rem_euclid(n) - n / 2;
            let dy = i64::from(other.y) - i64::from(tile.y);
            let gap = |d: i64| (d.unsigned_abs() as f64 - 1.0).max(0.0);
            (gap(dx) * frame.east_m_per_unit).hypot(gap(dy) * frame.north_m_per_unit)
        };
        let rings = RingFiles::read(
            release,
            tiles,
            vec![
                Kind::Terrain,
                Kind::Obstacles,
                Kind::Sources,
                Kind::Aircraft,
                Kind::AircraftFar,
            ],
            |other, kind| match kind {
                Kind::Aircraft => square_gap(other) <= FINE_BOXES_WITHIN_M,
                Kind::AircraftFar => true,
                _ => ring_of(other) <= ground_rings,
            },
        )?;
        Ok(Files {
            rings,
            ground_rings,
        })
    }

    pub fn bytes(&self) -> u64 {
        self.rings.bytes
    }
}

/// One tile's aircraft boxes, fine and far.
pub struct AircraftTile<'a> {
    pub tile: TileId,
    pub fine: Option<Aircraft<'a>>,
    pub far: Option<Aircraft<'a>>,
}

/// The candidates by place: short pieces by the cell of their middle, long ones always.
pub struct Index {
    origin: [f64; 2],
    columns: usize,
    rows: usize,
    starts: Vec<u32>,
    items: Vec<u32>,
    long: Vec<u32>,
}

impl Index {
    fn new(candidates: &[Candidate]) -> Self {
        let middle = |c: &Candidate| {
            let [a, b] = c.ends_m;
            [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
        };
        let is_long = |c: &Candidate| {
            let [a, b] = c.ends_m;
            (b[0] - a[0]).hypot(b[1] - a[1]) > 2.0 * INDEX_CELL_M
        };
        let (mut low, mut high) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for c in candidates.iter().filter(|c| !is_long(c)) {
            let m = middle(c);
            for axis in 0..2 {
                low[axis] = low[axis].min(m[axis]);
                high[axis] = high[axis].max(m[axis]);
            }
        }
        if low[0] > high[0] {
            (low, high) = ([0.0; 2], [0.0; 2]);
        }
        let columns = ((high[0] - low[0]) / INDEX_CELL_M) as usize + 1;
        let rows = ((high[1] - low[1]) / INDEX_CELL_M) as usize + 1;
        let cell = |m: [f64; 2]| {
            let column = ((m[0] - low[0]) / INDEX_CELL_M) as usize;
            let row = ((m[1] - low[1]) / INDEX_CELL_M) as usize;
            row.min(rows - 1) * columns + column.min(columns - 1)
        };
        let mut counts = vec![0u32; columns * rows + 1];
        let mut long = Vec::new();
        for (index, c) in candidates.iter().enumerate() {
            if is_long(c) {
                long.push(index as u32);
            } else {
                counts[cell(middle(c)) + 1] += 1;
            }
        }
        for i in 1..counts.len() {
            counts[i] += counts[i - 1];
        }
        let mut next = counts.clone();
        let mut items = vec![0u32; candidates.len() - long.len()];
        for (index, c) in candidates.iter().enumerate() {
            if !is_long(c) {
                let slot = &mut next[cell(middle(c))];
                items[*slot as usize] = index as u32;
                *slot += 1;
            }
        }
        Index {
            origin: low,
            columns,
            rows,
            starts: counts,
            items,
            long,
        }
    }

    /// Every candidate that may lie within `radius_m` of the rectangle `low`-`high` (frame
    /// metres), and some farther.
    pub fn within(&self, low: [f64; 2], high: [f64; 2], radius_m: f64, out: &mut Vec<u32>) {
        out.clear();
        out.extend_from_slice(&self.long);
        // A short piece reaches at most a cell from its middle.
        let reach = radius_m + INDEX_CELL_M;
        let span = |value: f64, origin: f64, limit: usize| {
            ((value - origin) / INDEX_CELL_M)
                .floor()
                .clamp(0.0, (limit - 1) as f64) as usize
        };
        if high[0] + reach < self.origin[0] || high[1] + reach < self.origin[1] {
            return;
        }
        let (c0, c1) = (
            span(low[0] - reach, self.origin[0], self.columns),
            span(high[0] + reach, self.origin[0], self.columns),
        );
        let (r0, r1) = (
            span(low[1] - reach, self.origin[1], self.rows),
            span(high[1] + reach, self.origin[1], self.rows),
        );
        for row in r0..=r1 {
            let first = row * self.columns;
            let (from, to) = (
                self.starts[first + c0] as usize,
                self.starts[first + c1 + 1] as usize,
            );
            out.extend_from_slice(&self.items[from..to]);
        }
    }
}

/// The square read for painting.
pub struct Square<'a> {
    pub tile: TileId,
    pub frame: LocalFrame,
    pub weather: &'a WeatherTable,
    pub ground: Ground<'a>,
    pub obstacles: Scene<'a>,
    pub attributes: Attributes,
    pub candidates: Vec<Candidate>,
    /// Per candidate its bound's factor: the Lden energy it emits (per metre for a line) times
    /// the largest gain, so that times the divergence at a distance bounds what it delivers.
    pub bound_factor: Vec<f64>,
    /// Per candidate its 3D length (m; 0 for a point).
    pub length_m: Vec<f64>,
    pub index: Index,
    pub aircraft: Vec<AircraftTile<'a>>,
}

/// The divergence of a piece of `length_m` (0: a point) at horizontal distance `distance_m`, as
/// the popup's bound takes it (`physics::bound::Spread`), linear.
pub fn divergence(length_m: f64, distance_m: f64) -> f64 {
    if length_m > 0.0 {
        let d = distance_m.max(LINE_PERPENDICULAR_FLOOR_M);
        (std::f64::consts::PI / d).min(length_m / (d * d)) / POINT_DIVERGENCE_LINEAR
    } else {
        let d = distance_m.max(1.0);
        1.0 / (POINT_DIVERGENCE_LINEAR * d * d)
    }
}

impl<'a> Square<'a> {
    pub fn new(weather: &'a WeatherTable, tile: TileId, files: &'a Files) -> Result<Self, String> {
        let frame = LocalFrame::at(tile.centre());
        let read = &files.rings;
        let mut ground = Ground::new(frame, tile, files.ground_rings);
        let mut obstacle_tiles = Vec::new();
        let mut sources: Vec<(TileId, Sources<'a>)> = Vec::new();
        let mut aircraft = Vec::new();
        for (index, &other) in read.tiles.iter().enumerate() {
            if read.kinds.contains(&Kind::Terrain) && ground_slot(tile, other, files.ground_rings) {
                let terrain = read
                    .file(index, Kind::Terrain)
                    .map(Terrain::parse)
                    .transpose()
                    .map_err(|e| e.to_string())?;
                ground.insert(other, terrain);
                let obstacles = read
                    .file(index, Kind::Obstacles)
                    .map(Obstacles::parse)
                    .transpose()
                    .map_err(|e| e.to_string())?;
                obstacle_tiles.push((other, obstacles));
                if let Some(bytes) = read.file(index, Kind::Sources) {
                    sources.push((other, Sources::parse(bytes).map_err(|e| e.to_string())?));
                }
            }
            let parse = |kind| {
                read.file(index, kind)
                    .map(Aircraft::parse)
                    .transpose()
                    .map_err(|e| e.to_string())
            };
            let (fine, far) = (parse(Kind::Aircraft)?, parse(Kind::AircraftFar)?);
            if fine.is_some() || far.is_some() {
                aircraft.push(AircraftTile {
                    tile: other,
                    fine,
                    far,
                });
            }
        }
        let mut obstacles = Scene::new(frame);
        obstacles.insert_all(obstacle_tiles);
        // Every piece within the ground reach of some pixel: of the square's centre, its reach
        // and half its diagonal.
        let half_diagonal = frame.east_m_per_unit.hypot(frame.north_m_per_unit) / 2.0;
        let (lat, lon) = tile.centre().to_degrees();
        let centre: PlaceWeather = weather.place(lat, lon);
        let gain = receiver_bound(centre.favourable.maximum(), 0.0, centre.alpha_db_per_km);
        let mut attributes = Attributes::default();
        let mut candidates = Vec::new();
        for (index, (other, file)) in sources.iter().enumerate() {
            let (list, mut found) = collect(
                file,
                *other,
                (0, index as u16),
                &ground,
                [0.0, 0.0],
                GROUND_REACH_M + half_diagonal,
                &gain,
            )?;
            let list = attributes.push(list);
            for candidate in &mut found {
                candidate.attribute.list = list;
            }
            candidates.extend(found);
        }
        let (bound_factor, length_m) = candidates
            .iter()
            .map(|c| {
                let source = &attributes[c.attribute];
                let periods: [f64; PERIODS] =
                    std::array::from_fn(|p| source.energy[p].iter().sum());
                let length = if c.line {
                    let [a, b] = c.ends_m;
                    (b[0] - a[0])
                        .hypot(b[1] - a[1])
                        .hypot(c.ground_m[1] - c.ground_m[0])
                } else {
                    0.0
                };
                (lden_energy(&periods) * GAIN_BOUND, length)
            })
            .unzip();
        let index = Index::new(&candidates);
        Ok(Square {
            tile,
            frame,
            weather,
            ground,
            obstacles,
            attributes,
            candidates,
            bound_factor,
            length_m,
            index,
            aircraft,
        })
    }

    /// The bound of candidate `index` at horizontal distance `distance_m`: its Lden energy as
    /// the popup's bound takes it, without the air absorption (so larger).
    pub fn bound(&self, index: usize, distance_m: f64) -> f64 {
        self.bound_factor[index] * divergence(self.length_m[index], distance_m)
    }
}

/// Whether `other` lies within `rings` of `tile` (its ground was read).
fn ground_slot(tile: TileId, other: TileId, rings: u32) -> bool {
    let n = i64::from(tiles::geo::TILES_PER_AXIS);
    let dx = (i64::from(other.x) - i64::from(tile.x) + n / 2).rem_euclid(n) - n / 2;
    let dy = i64::from(other.y) - i64::from(tile.y);
    dx.unsigned_abs().max(dy.unsigned_abs()) <= u64::from(rings)
}

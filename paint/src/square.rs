//! One z12 square's neighbourhood, read and parsed once for all its pixels: the ground and the
//! obstacles to the ground reach of its farthest pixel, every ground source whose reach covers
//! one of its pixels, indexed by place and reach, and the aircraft boxes to the aircraft reach.

use physics::bound::{ReceiverBound, receiver_bound};
use physics::weather::{PlaceWeather, WeatherTable};
use popup::aircraft::FINE_BOXES_WITHIN_M;
use popup::aircraft::boxes::AIRCRAFT_REACH_M;
use popup::candidates::{Attributes, Candidate, GROUND_REACH_M, collect, loud};
use popup::obstacles::Scene;
use popup::release::{Release, RingFiles};
use popup::scene::Ground;
use tiles::Kind;
use tiles::aircraft::Aircraft;
use tiles::geo::{LocalFrame, Mercator, TileId};
use tiles::obstacles::Obstacles;
use tiles::sources::Sources;
use tiles::terrain::Terrain;

/// Side of the index's cells (m).
const INDEX_CELL_M: f64 = 100.0;
/// The index's reach classes (m): a source sits in the first that covers its reach.
const REACH_CLASSES_M: [f64; 7] = [
    250.0,
    500.0,
    1_000.0,
    2_000.0,
    4_000.0,
    8_000.0,
    GROUND_REACH_M,
];

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

    /// The files read, for a device copy of the square.
    pub fn rings(&self) -> &RingFiles {
        &self.rings
    }

    /// The rings around the square read for the ground kinds.
    pub fn ground_rings(&self) -> u32 {
        self.ground_rings
    }
}

/// One tile's aircraft boxes, fine and far.
pub struct AircraftTile<'a> {
    pub tile: TileId,
    pub fine: Option<Aircraft<'a>>,
    pub far: Option<Aircraft<'a>>,
}

/// The candidates by place and reach: per reach class a grid, a point asking each class within
/// that class's reach.
pub struct Index {
    classes: Vec<(f64, Grid)>,
}

impl Index {
    fn new(candidates: &[Candidate], reaches: &[f64]) -> Self {
        let mut members = vec![Vec::new(); REACH_CLASSES_M.len()];
        for (index, &reach) in reaches.iter().enumerate() {
            let class = REACH_CLASSES_M
                .iter()
                .position(|&class| reach <= class)
                .unwrap_or(REACH_CLASSES_M.len() - 1);
            members[class].push(index as u32);
        }
        Index {
            classes: REACH_CLASSES_M
                .iter()
                .zip(members)
                .map(|(&reach, members)| (reach, Grid::new(candidates, members)))
                .collect(),
        }
    }

    /// Every candidate whose reach may cover a point of the rectangle `low`-`high` (frame
    /// metres): every one loud there, and some not.
    pub fn reaching(&self, low: [f64; 2], high: [f64; 2], out: &mut Vec<u32>) {
        out.clear();
        for (reach, grid) in &self.classes {
            grid.within(low, high, *reach, out);
        }
    }

    /// Every candidate that may lie within `radius_m` of the rectangle, and some farther.
    pub fn within(&self, low: [f64; 2], high: [f64; 2], radius_m: f64, out: &mut Vec<u32>) {
        out.clear();
        for (_, grid) in &self.classes {
            grid.within(low, high, radius_m, out);
        }
    }
}

/// One reach class's candidates by place: short pieces by the cell of their middle, long ones
/// always.
struct Grid {
    origin: [f64; 2],
    columns: usize,
    rows: usize,
    starts: Vec<u32>,
    items: Vec<u32>,
    long: Vec<u32>,
}

impl Grid {
    fn new(candidates: &[Candidate], members: Vec<u32>) -> Self {
        let middle = |index: u32| {
            let [a, b] = candidates[index as usize].ends_m;
            [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
        };
        let is_long = |index: u32| {
            let [a, b] = candidates[index as usize].ends_m;
            (b[0] - a[0]).hypot(b[1] - a[1]) > 2.0 * INDEX_CELL_M
        };
        let (long, short): (Vec<u32>, Vec<u32>) =
            members.into_iter().partition(|&index| is_long(index));
        let (mut low, mut high) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for &index in &short {
            let m = middle(index);
            for axis in 0..2 {
                low[axis] = low[axis].min(m[axis]);
                high[axis] = high[axis].max(m[axis]);
            }
        }
        if short.is_empty() {
            (low, high) = ([0.0; 2], [0.0; 2]);
        }
        let columns = ((high[0] - low[0]) / INDEX_CELL_M) as usize + 1;
        let rows = ((high[1] - low[1]) / INDEX_CELL_M) as usize + 1;
        let cell = |m: [f64; 2]| {
            let column = ((m[0] - low[0]) / INDEX_CELL_M) as usize;
            let row = ((m[1] - low[1]) / INDEX_CELL_M) as usize;
            row.min(rows - 1) * columns + column.min(columns - 1)
        };
        let mut starts = vec![0u32; columns * rows + 1];
        for &index in &short {
            starts[cell(middle(index)) + 1] += 1;
        }
        for i in 1..starts.len() {
            starts[i] += starts[i - 1];
        }
        let mut next = starts.clone();
        let mut items = vec![0u32; short.len()];
        for &index in &short {
            let slot = &mut next[cell(middle(index))];
            items[*slot as usize] = index;
            *slot += 1;
        }
        Grid {
            origin: low,
            columns,
            rows,
            starts,
            items,
            long,
        }
    }

    /// Appends every member that may lie within `radius_m` of the rectangle `low`-`high`, and
    /// some farther.
    fn within(&self, low: [f64; 2], high: [f64; 2], radius_m: f64, out: &mut Vec<u32>) {
        out.extend_from_slice(&self.long);
        // A short piece reaches at most a cell from its middle.
        let reach = radius_m + INDEX_CELL_M;
        let span = |value: f64, origin: f64, limit: usize| {
            ((value - origin) / INDEX_CELL_M)
                .floor()
                .clamp(0.0, (limit - 1) as f64) as usize
        };
        if self.items.is_empty()
            || high[0] + reach < self.origin[0]
            || high[1] + reach < self.origin[1]
        {
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
    pub index: Index,
    pub aircraft: Vec<AircraftTile<'a>>,
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
        let reaches: Vec<f64> = (candidates.iter())
            .map(|candidate| candidate.reach_m(&attributes[candidate.attribute]))
            .collect();
        let index = Index::new(&candidates, &reaches);
        Ok(Square {
            tile,
            frame,
            weather,
            ground,
            obstacles,
            attributes,
            candidates,
            index,
            aircraft,
        })
    }

    /// Whether candidate `index` is loud at `position` (frame metres) for a receiver bounded by
    /// `receiver`, as the popup decides it: there it is evaluated exactly.
    pub fn loud(&self, index: u32, position: [f64; 2], receiver: &ReceiverBound) -> bool {
        let candidate = &self.candidates[index as usize];
        let distance = candidate.distance_from(position);
        let source = &self.attributes[candidate.attribute];
        distance <= GROUND_REACH_M && loud(&candidate.bound_at_distance(distance, source, receiver))
    }
}

/// Whether `other` lies within `rings` of `tile` (its ground was read).
fn ground_slot(tile: TileId, other: TileId, rings: u32) -> bool {
    let n = i64::from(tiles::geo::TILES_PER_AXIS);
    let dx = (i64::from(other.x) - i64::from(tile.x) + n / 2).rem_euclid(n) - n / 2;
    let dy = i64::from(other.y) - i64::from(tile.y);
    dx.unsigned_abs().max(dy.unsigned_abs()) <= u64::from(rings)
}

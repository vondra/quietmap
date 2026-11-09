//! One z12 square's neighbourhood, read and parsed once for all its pixels: the ground and the
//! obstacles to the ground reach of its farthest pixel, every ground source whose reach covers
//! one of its pixels, indexed by place and reach, the aircraft boxes to the aircraft reach and the
//! runways' rolls to theirs.

#[path = "source_index.rs"]
mod source_index;
use source_index::Index;

use crate::exact::Point;
use crate::inputs::{KINDS, Neighbourhood};
use physics::bound::{ReceiverBound, Spread, received_energy_bound, receiver_bound};
use physics::doc29::atmosphere::PlaceAtmosphere;
use physics::weather::{PlaceWeather, WeatherTable};
use popup::aircraft::rolls::{HeardPath, prepare as prepare_rolls};
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

/// The files of a square's neighbourhood.
pub struct Files {
    rings: RingFiles,
    ground_rings: u32,
    /// The release's year root, where the rays' ends read the fine terrain when they need it.
    year_root: std::path::PathBuf,
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
    /// of every tile; the rolls of the tiles within their reach.
    pub fn read(release: &Release, tile: TileId) -> Result<Self, String> {
        let neighbourhood = Neighbourhood::new(tile);
        let rings = RingFiles::read(
            release,
            neighbourhood.tiles().collect(),
            // Fine terrain stays lazy on the CPU; the device upload reads it from Ground.
            KINDS
                .into_iter()
                .filter(|&kind| kind != Kind::TerrainFine)
                .collect(),
            |other, kind| neighbourhood.contains(other, kind),
        )?;
        Ok(Files {
            rings,
            ground_rings: neighbourhood.ground_rings,
            year_root: release.year_root.clone(),
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

/// The square read for painting.
pub struct Square<'a> {
    pub tile: TileId,
    pub frame: LocalFrame,
    pub weather: &'a WeatherTable,
    pub ground: Ground,
    pub obstacles: Scene<'a>,
    pub attributes: Attributes,
    pub candidates: Vec<Candidate>,
    pub index: Index,
    reaches_squared: Vec<f64>,
    spreads: Vec<Spread>,
    pub aircraft: Vec<AircraftTile<'a>>,
    /// The rolls of the runways within their reach of the square, by path.
    pub rolls: Vec<HeardPath>,
    /// The source tiles the candidates came from (a candidate's `display.tile`), where its display
    /// record is read.
    pub sources: Vec<Sources<'a>>,
}

impl<'a> Square<'a> {
    /// Whether either aircraft resolution contains a box: empty files carry no flight sound.
    pub(crate) fn has_aircraft_boxes(&self) -> bool {
        self.aircraft.iter().any(|tile| {
            tile.fine
                .iter()
                .chain(&tile.far)
                .any(|boxes| boxes.box_count() > 0)
        })
    }

    pub fn new(weather: &'a WeatherTable, tile: TileId, files: &'a Files) -> Result<Self, String> {
        let frame = LocalFrame::at(tile.centre());
        let read = &files.rings;
        let mut ground = Ground::new(
            frame,
            tile,
            files.ground_rings,
            Some(files.year_root.clone()),
        );
        let mut obstacle_tiles = Vec::new();
        let mut sources: Vec<(TileId, Sources<'a>)> = Vec::new();
        let mut aircraft = Vec::new();
        let mut rolls = Vec::new();
        // The yearly atmosphere of each square the rolls start in, as the boxes take theirs.
        let mut roll_atmospheres: std::collections::HashMap<
            tiles::square::Square,
            PlaceAtmosphere,
        > = std::collections::HashMap::new();
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
            if let Some(bytes) = read.file(index, Kind::AircraftRolls) {
                let parsed = tiles::aircraft_rolls::parse(bytes).map_err(|e| e.to_string())?;
                let place = (roll_atmospheres.entry(tiles::square::Square::of(other)))
                    .or_insert_with_key(|square| {
                        let (lat, lon) = square.middle_tile().centre().to_degrees();
                        PlaceAtmosphere::new(&weather.place(lat, lon).alpha_db_per_km)
                    });
                rolls.extend(prepare_rolls(other, &parsed, place));
            }
            if fine.is_some() || far.is_some() {
                aircraft.push(AircraftTile {
                    tile: other,
                    fine,
                    far,
                });
            }
        }
        let mut obstacles = Scene::new(frame);
        let half = [frame.east_m_per_unit / 2.0, frame.north_m_per_unit / 2.0];
        let (mut receiver_low, mut receiver_high) = (half.map(|value| -value), half);
        let facade_footprints: std::collections::HashSet<u64> = obstacle_tiles
            .iter()
            .filter(|(other, _)| *other == tile)
            .filter_map(|(_, outlines)| outlines.as_ref())
            .flat_map(|outlines| (0..outlines.outline_count()).map(|index| outlines.outline(index)))
            .filter(|outline| outline.kind.is_building() && outline.envelope.is_enclosed())
            .map(|outline| outline.footprint_id)
            .collect();
        // A footprint can have a detached wing stored only in a neighbouring tile.
        for (other, outlines) in &obstacle_tiles {
            let Some(outlines) = outlines else { continue };
            for index in 0..outlines.outline_count() {
                let outline = outlines.outline(index);
                if !facade_footprints.contains(&outline.footprint_id) {
                    continue;
                }
                for vertex in outline.first_vertex..outline.first_vertex + outline.vertex_count {
                    let position =
                        frame.to_metres(other.global(outlines.vertex(vertex)).to_mercator());
                    for axis in 0..2 {
                        let margin = popup::obstacles::FACADE_RECEIVER_OFFSET_M;
                        receiver_low[axis] = receiver_low[axis].min(position[axis] - margin);
                        receiver_high[axis] = receiver_high[axis].max(position[axis] + margin);
                    }
                }
            }
        }
        obstacles.insert_all(obstacle_tiles);
        obstacles.prepare_reflection_coverage(receiver_low, receiver_high)?;
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
                &ground.frame,
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
        // Leave roundoff slack at the conservative reach before the full receiver bound.
        let reaches_squared = reaches
            .iter()
            .map(|reach| reach * reach * (1.0 + 64.0 * f64::EPSILON))
            .collect();
        let spreads = candidates.iter().map(Candidate::spread).collect();
        Ok(Square {
            tile,
            frame,
            weather,
            ground,
            obstacles,
            attributes,
            candidates,
            index,
            reaches_squared,
            spreads,
            aircraft,
            rolls,
            sources: sources.into_iter().map(|(_, file)| file).collect(),
        })
    }

    /// The distance from `point`'s receiver to candidate `index`'s emitters, as a click's
    /// contributor takes it (`popup::evaluate::Received::slant_distance_m`).
    pub fn slant_distance_m(&self, index: u32, point: &Point) -> f64 {
        let candidate = &self.candidates[index as usize];
        let height_m = self.attributes[candidate.attribute].height_m;
        candidate.slant_distance_from(point.position, point.altitude_m, height_m)
    }

    /// Whether candidate `index` is loud at `position` (frame metres) for a receiver bounded by
    /// `receiver`, as the popup decides it: there it is evaluated exactly.
    pub fn loud(&self, index: u32, position: [f64; 2], receiver: &ReceiverBound) -> bool {
        let candidate = &self.candidates[index as usize];
        let [x, y] = candidate.offset_from(position);
        if x * x + y * y > self.reaches_squared[index as usize] {
            return false;
        }
        let distance = x.hypot(y);
        let source = &self.attributes[candidate.attribute];
        distance <= GROUND_REACH_M
            && loud(&received_energy_bound(
                &source.energy,
                self.spreads[index as usize],
                distance,
                receiver,
            ))
    }
}

/// Whether `other` lies within `rings` of `tile` (its ground was read).
fn ground_slot(tile: TileId, other: TileId, rings: u32) -> bool {
    other.ring_distance(tile) <= rings
}

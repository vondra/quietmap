//! The traffic pass's rolls as `aircraft-rolls` square files (`tiles::aircraft_rolls`): each roll
//! in the z12 tile of its start, on its runway's elevation, the mean of the terrain tiles' ground
//! under its two ends (the ground the popup stands its receivers on).

use super::runways::{RollKey, Runways};
use physics::bands::PERIODS;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use tiles::aircraft_rolls::{Roll, encode};
use tiles::geo::{LocalFrame, Mercator, TileId};
use tiles::terrain::Terrain;

/// The terrain tiles of a prepared year root, each read once.
struct Ground<'a> {
    root: &'a Path,
    tiles: HashMap<TileId, Option<Terrain>>,
}

impl Ground<'_> {
    /// The ground's height at a point (m); a point without a terrain tile is verified ocean.
    fn height_m(&mut self, at: Mercator) -> Result<f64, String> {
        let tile = TileId::containing(at);
        if !self.tiles.contains_key(&tile) {
            let read = tiles::square::read_tile(self.root, tile, tiles::Kind::Terrain)?;
            let parsed = read
                .map(|bytes| Terrain::parse(&bytes).map_err(|error| error.to_string()))
                .transpose()?;
            self.tiles.insert(tile, parsed);
        }
        match &self.tiles[&tile] {
            None => Ok(0.0),
            Some(terrain) => terrain
                .sample(at)
                .map(|ground| ground.height_m)
                .ok_or_else(|| format!("no terrain height at {:?}", at.to_degrees())),
        }
    }
}

/// Writes the rolls with movements as square files under `out`; returns how many.
pub fn write(
    rolls: &BTreeMap<RollKey, [f64; PERIODS]>,
    runways: &Runways,
    terrain_root: &Path,
    out: &Path,
) -> Result<usize, String> {
    let mut ground = Ground {
        root: terrain_root,
        tiles: HashMap::new(),
    };
    let mut by_tile: BTreeMap<TileId, Vec<Roll>> = BTreeMap::new();
    for (&key, &movements_per_day) in rolls {
        let Some(path) = runways.path(key) else {
            continue;
        };
        let [start, end, ahead] =
            [path.start, path.end, path.ahead].map(|[lat, lon]| Mercator::from_degrees(lat, lon));
        let tile = TileId::containing(start);
        let local = tile
            .to_local(start)
            .ok_or_else(|| format!("{:?} outside its own tile", path.start))?;
        // Toward the runway's end, not the class's own: every class of a runway direction keeps one
        // heading, the identity of the stream its passes make (`popup::aircraft::rolls`).
        let [east, north] = LocalFrame::at(start).to_metres(ahead);
        let field_m = 0.5 * (ground.height_m(start)? + ground.height_m(end)?);
        by_tile.entry(tile).or_default().push(Roll {
            start: local,
            heading_rad: north.atan2(east),
            length_m: path.length_m,
            speed_ms: path.speed_ms,
            field_m,
            class: key.class,
            landing: key.landing,
            movements_per_day,
        });
    }
    let written = by_tile.values().map(Vec::len).sum();
    let tiles: Vec<(TileId, Vec<u8>)> = by_tile
        .into_iter()
        .map(|(tile, rolls)| (tile, encode(&rolls)))
        .collect();
    tiles::square::write_tiles(out, tiles::Kind::AircraftRolls, &tiles)?;
    Ok(written)
}

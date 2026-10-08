//! `qm-raster --prepared DIR --year YYYY --layer LAYER --z Z --x X --y Y`: one 256-pixel map tile
//! (standard XYZ) of the data under the computation, as a PNG on stdout: the terrain's height,
//! forest cover or hard ground per lattice node, or the buildings by height or the noise
//! barriers of the obstacles. Where the release has nothing to show the tile is transparent.

mod ground;
mod outlines;
mod png;

use std::io::Write;
use std::path::PathBuf;
use tiles::geo::{Mercator, TILES_PER_AXIS, TileId, ZOOM};
use tiles::read::read_all;
use tiles::{COMPLETION_MARKER, Kind, tile_path};

/// Pixels per side of a map tile.
const PIXELS: usize = 256;
/// The map's deepest zoom: deeper tiles are the browser's enlargement of these.
const MAX_ZOOM: u32 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layer {
    Elevation,
    Forest,
    Hard,
    Buildings,
    Barriers,
}

impl Layer {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "elevation" => Layer::Elevation,
            "forest" => Layer::Forest,
            "hard" => Layer::Hard,
            "buildings" => Layer::Buildings,
            "barriers" => Layer::Barriers,
            _ => return None,
        })
    }

    /// The terrain layers read 16 files a tile at zoom 10; the obstacles lie in one file from
    /// zoom 13 on, where a building covers pixels.
    fn min_zoom(self) -> u32 {
        match self {
            Layer::Elevation | Layer::Forest | Layer::Hard => 10,
            Layer::Buildings | Layer::Barriers => 13,
        }
    }

    /// Pixels beyond the map tile whose data reach into it: half a barrier's line.
    fn margin_px(self) -> f64 {
        match self {
            Layer::Barriers => outlines::BARRIER_WIDTH_PX / 2.0,
            _ => 0.0,
        }
    }

    fn kind(self) -> Kind {
        match self {
            Layer::Elevation | Layer::Forest | Layer::Hard => Kind::Terrain,
            Layer::Buildings | Layer::Barriers => Kind::Obstacles,
        }
    }
}

/// The colour of `value` between the stops (ascending), the end stops' beyond them.
fn ramp(stops: &[(f64, [f64; 3])], value: f64) -> [f64; 3] {
    let upper = stops.partition_point(|&(stop, _)| stop <= value);
    if upper == 0 || upper == stops.len() {
        return stops[upper.saturating_sub(1)].1;
    }
    let ((low, from), (high, to)) = (stops[upper - 1], stops[upper]);
    let share = (value - low) / (high - low);
    std::array::from_fn(|channel| from[channel] + share * (to[channel] - from[channel]))
}

/// A map tile and where its pixels lie, in z12 Mercator units.
#[derive(Clone, Copy, Debug, PartialEq)]
struct MapTile {
    west: f64,
    north: f64,
    /// z12 units per pixel.
    pixel: f64,
}

impl MapTile {
    fn new(z: u32, x: u32, y: u32) -> Self {
        let span = 2f64.powi(ZOOM as i32 - z as i32);
        MapTile {
            west: f64::from(x) * span,
            north: f64::from(y) * span,
            pixel: span / PIXELS as f64,
        }
    }

    fn pixel_centre(self, column: usize, row: usize) -> Mercator {
        Mercator {
            x: self.west + (column as f64 + 0.5) * self.pixel,
            y: self.north + (row as f64 + 0.5) * self.pixel,
        }
    }

    /// The z12 tiles under the map tile widened by `margin_px` pixels on every side (across the
    /// antimeridian, within the Mercator limit).
    fn z12_tiles(self, margin_px: f64) -> Vec<TileId> {
        let (margin, span) = (margin_px * self.pixel, self.pixel * PIXELS as f64);
        let range = |low: f64| (low - margin).floor() as i64..(low + span + margin).ceil() as i64;
        let axis = i64::from(TILES_PER_AXIS);
        range(self.north)
            .filter(|y| (0..axis).contains(y))
            .flat_map(|y| {
                range(self.west).map(move |x| TileId {
                    x: x.rem_euclid(axis) as u32,
                    y: y as u32,
                })
            })
            .collect()
    }
}

fn value<'a>(arguments: &'a [String], key: &str) -> Option<&'a str> {
    arguments
        .windows(2)
        .find(|pair| pair[0] == format!("--{key}"))
        .map(|pair| pair[1].as_str())
}

fn run(arguments: &[String]) -> Result<Vec<u8>, String> {
    let required = |key: &str| value(arguments, key).ok_or_else(|| format!("missing --{key}"));
    let integer = |key: &str| {
        required(key)?
            .parse::<u32>()
            .map_err(|_| format!("--{key} is not a tile number"))
    };
    let layer = Layer::parse(required("layer")?).ok_or("--layer is no layer")?;
    let (z, x, y) = (integer("z")?, integer("x")?, integer("y")?);
    if !(layer.min_zoom()..=MAX_ZOOM).contains(&z) || x >> z != 0 || y >> z != 0 {
        return Err(format!("no {layer:?} tile {z}/{x}/{y}"));
    }
    let year_root = PathBuf::from(required("prepared")?).join(required("year")?);
    if !year_root.join(COMPLETION_MARKER).exists() {
        return Err(format!("{} is not a complete release", year_root.display()));
    }
    let map_tile = MapTile::new(z, x, y);
    let tiles = map_tile.z12_tiles(layer.margin_px());
    let paths: Vec<PathBuf> = tiles
        .iter()
        .map(|&tile| tile_path(&year_root, tile, layer.kind()))
        .collect();
    let files = read_all(&paths)?;
    let pixels = match layer {
        Layer::Elevation | Layer::Forest | Layer::Hard => {
            ground::render(layer, map_tile, &tiles, &files)?
        }
        Layer::Buildings | Layer::Barriers => outlines::render(layer, map_tile, &tiles, &files)?,
    };
    Ok(png::encode(PIXELS, PIXELS, &pixels))
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match run(&arguments) {
        Ok(png) => std::io::stdout().write_all(&png).unwrap_or_else(|error| {
            eprintln!("qm-raster: {error}");
            std::process::exit(1)
        }),
        Err(error) => {
            eprintln!("qm-raster: {error}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A zoom-10 tile lies over 16 z12 tiles, a zoom-14 tile within one, and its pixels run from
    /// its north-west corner.
    #[test]
    fn map_tiles_cover_their_z12_tiles() {
        let coarse = MapTile::new(10, 553, 346);
        assert_eq!(coarse.z12_tiles(0.0).len(), 16);
        assert_eq!(coarse.z12_tiles(0.0)[0], TileId { x: 2212, y: 1384 });
        assert_eq!(coarse.z12_tiles(0.0)[15], TileId { x: 2215, y: 1387 });
        let fine = MapTile::new(14, 8849, 5549);
        assert_eq!(fine.z12_tiles(0.0), vec![TileId { x: 2212, y: 1387 }]);
        // A barrier's margin reaches the neighbours a tile touches, across the antimeridian: a
        // zoom-13 tile touches its z12 tile's north and west edges here.
        let touching = |x, y| TileId { x, y };
        assert_eq!(
            MapTile::new(13, 0, 2 * 1387).z12_tiles(1.5),
            vec![
                touching(4095, 1386),
                touching(0, 1386),
                touching(4095, 1387),
                touching(0, 1387)
            ]
        );
        let corner = fine.pixel_centre(0, 0);
        assert_eq!(corner.x, 8849.0 / 4.0 + 0.5 / 1024.0);
        assert_eq!(corner.y, 5549.0 / 4.0 + 0.5 / 1024.0);
    }

    #[test]
    fn tiles_beyond_a_layers_zooms_or_the_world_are_refused() {
        let arguments = |layer: &str, z: &str, x: &str| -> Vec<String> {
            [
                "--prepared",
                "/nowhere",
                "--year",
                "2026",
                "--layer",
                layer,
                "--z",
                z,
                "--x",
                x,
            ]
            .into_iter()
            .chain(["--y", "0"])
            .map(String::from)
            .collect()
        };
        for (layer, z, x) in [
            ("buildings", "12", "0"),
            ("elevation", "9", "0"),
            ("forest", "17", "0"),
            ("hard", "10", "1024"),
        ] {
            assert!(
                run(&arguments(layer, z, x)).unwrap_err().starts_with("no "),
                "{layer} {z} {x}"
            );
        }
        assert!(run(&arguments("roads", "14", "0")).is_err());
        assert!(
            run(&arguments("barriers", "13", "0"))
                .unwrap_err()
                .contains("not a complete release")
        );
    }
}

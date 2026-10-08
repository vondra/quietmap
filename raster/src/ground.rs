//! The terrain layers, the data as the release holds it: the lattice cell under every pixel in
//! the colour of its mean height, shaded by its slope; or the nearest node's forest cover or
//! acoustically hard share of the ground (paved, built over or water; CNOSSOS's G = 1 - hard).

use crate::{Layer, MapTile, PIXELS, ramp};
use tiles::geo::TileId;
use tiles::terrain::{
    HEIGHT_MISSING, NODES_PER_DEGREE, Node, PERCENT_MAX, Terrain, height_m_of_code,
};

const TRANSPARENT: [u8; 4] = [0; 4];
/// Metres of one arc-second of latitude (mean Earth radius).
const ARC_SECOND_M: f64 = 6_371_008.8 * std::f64::consts::PI / (180.0 * 3600.0);
/// The relief is shaded as if twice as steep: a city's gentle slopes show.
const RELIEF_EXAGGERATION: f64 = 2.0;
/// Light from the north-west, 45 degrees up, in (east, north, up).
const LIGHT: [f64; 3] = [-0.5, 0.5, std::f64::consts::FRAC_1_SQRT_2];
/// Height colours: the sea (0 m) pale blue, land below it teal, then dense where most people
/// live, so a city's hills and valleys differ.
const HEIGHT_STOPS: [(f64, [f64; 3]); 14] = [
    (-100.0, [20.0, 70.0, 90.0]),
    (-5.0, [60.0, 150.0, 140.0]),
    (0.0, [160.0, 195.0, 210.0]),
    (1.0, [20.0, 110.0, 60.0]),
    (100.0, [70.0, 160.0, 60.0]),
    (200.0, [170.0, 205.0, 80.0]),
    (300.0, [245.0, 220.0, 90.0]),
    (400.0, [245.0, 170.0, 70.0]),
    (550.0, [225.0, 115.0, 55.0]),
    (800.0, [190.0, 70.0, 55.0]),
    (1200.0, [140.0, 55.0, 80.0]),
    (1800.0, [105.0, 80.0, 140.0]),
    (2600.0, [160.0, 160.0, 200.0]),
    (4000.0, [235.0, 235.0, 245.0]),
];
const FOREST: [u8; 3] = [21, 128, 61];
const HARD_LOW: [f64; 3] = [250.0, 204.0, 21.0];
const HARD_HIGH: [f64; 3] = [220.0, 38.0, 38.0];

/// The brightness of ground rising `eastward` and `southward` (metres per metre) under the light.
fn shade(eastward: f64, southward: f64) -> f64 {
    let normal = [
        -RELIEF_EXAGGERATION * eastward,
        RELIEF_EXAGGERATION * southward,
        1.0,
    ];
    let length = normal.iter().map(|value| value * value).sum::<f64>().sqrt();
    (normal.iter().zip(LIGHT).map(|(n, l)| n * l).sum::<f64>() / length).max(0.0)
}

/// The lattice cell's colour (the cell between nodes `row`..`row + 1` and `column`..`column + 1`):
/// its mean height, shaded by its slope. A cell lies whole in the window of every tile it
/// touches, so a seam's two sides agree; a cell touching a node without data is left out.
fn elevation(terrain: &Terrain, row: u32, column: u32, latitude_deg: f64) -> [u8; 4] {
    let height_at = |row: u32, column: u32| {
        let code = terrain.node(row, column).height_code;
        (code != HEIGHT_MISSING).then(|| height_m_of_code(code))
    };
    let (Some(north_west), Some(north_east), Some(south_west), Some(south_east)) = (
        height_at(row, column),
        height_at(row, column + 1),
        height_at(row + 1, column),
        height_at(row + 1, column + 1),
    ) else {
        return TRANSPARENT;
    };
    let eastward = (north_east - north_west + south_east - south_west)
        / (2.0 * ARC_SECOND_M * latitude_deg.to_radians().cos());
    let southward = (south_west - north_west + south_east - north_east) / (2.0 * ARC_SECOND_M);
    let mean = (north_west + north_east + south_west + south_east) / 4.0;
    let factor = 0.55 + 0.6 * shade(eastward, southward);
    let [r, g, b] =
        ramp(&HEIGHT_STOPS, mean).map(|value| (value * factor).round().clamp(0.0, 255.0) as u8);
    [r, g, b, 255]
}

fn percent_colour(layer: Layer, node: Node) -> [u8; 4] {
    match layer {
        Layer::Forest if (1..=PERCENT_MAX).contains(&node.forest_percent) => {
            let alpha = (229.5 * f64::from(node.forest_percent) / 100.0).round() as u8;
            [FOREST[0], FOREST[1], FOREST[2], alpha]
        }
        Layer::Hard if (1..=PERCENT_MAX).contains(&node.impervious_percent) => {
            let share = f64::from(node.impervious_percent) / 100.0;
            let [r, g, b] = std::array::from_fn(|channel| {
                (HARD_LOW[channel] + share * (HARD_HIGH[channel] - HARD_LOW[channel])).round() as u8
            });
            [r, g, b, (255.0 * (0.3 + 0.6 * share)).round() as u8]
        }
        _ => TRANSPARENT,
    }
}

/// The map tile's pixels from the terrain files of `tiles` (absent: the sea, left transparent).
pub fn render(
    layer: Layer,
    map_tile: MapTile,
    tiles: &[TileId],
    files: &[Option<Vec<u8>>],
) -> Result<Vec<[u8; 4]>, String> {
    let terrains = files
        .iter()
        .map(|file| file.as_deref().map(Terrain::parse).transpose())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let latitudes: Vec<f64> = (0..PIXELS)
        .map(|row| map_tile.pixel_centre(0, row).to_degrees().0)
        .collect();
    let longitudes: Vec<f64> = (0..PIXELS)
        .map(|column| map_tile.pixel_centre(column, 0).to_degrees().1)
        .collect();
    let nodes_per_degree = f64::from(NODES_PER_DEGREE);
    let mut pixels = vec![TRANSPARENT; PIXELS * PIXELS];
    for (row, &latitude) in latitudes.iter().enumerate() {
        for (column, &longitude) in longitudes.iter().enumerate() {
            let tile = TileId::containing(map_tile.pixel_centre(column, row));
            let Some(Some(terrain)) = tiles
                .iter()
                .position(|&each| each == tile)
                .map(|at| &terrains[at])
            else {
                continue;
            };
            // Fractional lattice position: rows run south from the window's north node.
            let window = terrain.window();
            let row_position = f64::from(window.north_node) - latitude * nodes_per_degree;
            let column_position = longitude * nodes_per_degree - f64::from(window.west_node);
            let (rows, columns) = (f64::from(window.rows), f64::from(window.columns));
            pixels[row * PIXELS + column] = match layer {
                // The cell holding the pixel: its corners lie in the window, which brackets the tile.
                Layer::Elevation => elevation(
                    terrain,
                    row_position.floor().clamp(0.0, rows - 2.0) as u32,
                    column_position.floor().clamp(0.0, columns - 2.0) as u32,
                    latitude,
                ),
                _ => {
                    let (node_row, node_column) = (row_position.round(), column_position.round());
                    if !(0.0..rows).contains(&node_row) || !(0.0..columns).contains(&node_column) {
                        continue;
                    }
                    percent_colour(layer, terrain.node(node_row as u32, node_column as u32))
                }
            };
        }
    }
    Ok(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiles::geo::Mercator;
    use tiles::terrain::{Window, encode};

    /// The tile's window: flat at 1,000 m, a forest and a hard square in its north-west corner.
    fn terrain_file(tile: TileId) -> Vec<u8> {
        let window = Window::of_tile(tile);
        let nodes: Vec<Node> = (0..window.rows)
            .flat_map(|row| {
                (0..window.columns).map(move |column| {
                    let corner = row < 10 && column < 10;
                    Node {
                        height_code: 7500,
                        impervious_percent: if corner { 80 } else { 0 },
                        forest_percent: if corner { 50 } else { 101 },
                    }
                })
            })
            .collect();
        encode(window, &nodes)
    }

    #[test]
    fn heights_take_their_stops_colour_and_slopes_their_shade() {
        assert_eq!(ramp(&HEIGHT_STOPS, 300.0), [245.0, 220.0, 90.0]);
        assert_eq!(ramp(&HEIGHT_STOPS, -500.0), HEIGHT_STOPS[0].1);
        assert_eq!(ramp(&HEIGHT_STOPS, 9000.0), HEIGHT_STOPS[13].1);
        assert_eq!(ramp(&HEIGHT_STOPS, 150.0), [120.0, 182.5, 70.0]);
        // Ground facing the north-west light is brighter than flat ground, facing away darker.
        let flat = shade(0.0, 0.0);
        assert!((flat - LIGHT[2]).abs() < 1e-12);
        assert!(shade(0.1, 0.0) > flat && shade(0.0, 0.1) > flat);
        assert!(shade(-0.1, 0.0) < flat && shade(0.0, -0.1) < flat);
        let tile = TileId::containing(Mercator::from_degrees(50.07553, 14.43781));
        let bytes = terrain_file(tile);
        let terrain = Terrain::parse(&bytes).unwrap();
        let [r, g, b] =
            ramp(&HEIGHT_STOPS, 1000.0).map(|value| (value * (0.55 + 0.6 * flat)).round() as u8);
        assert_eq!(elevation(&terrain, 50, 50, 50.0), [r, g, b, 255]);
    }

    /// A zoom-14 tile in the tile's north-west corner shows the forest and the hard square where
    /// they are, and nothing where the forest has no data or the ground is soft.
    #[test]
    fn forest_and_hard_ground_show_their_nodes() {
        let tile = TileId { x: 2212, y: 1387 };
        let files = vec![Some(terrain_file(tile))];
        let map_tile = MapTile::new(14, 4 * 2212, 4 * 1387);
        let forest = render(Layer::Forest, map_tile, &[tile], &files).unwrap();
        let hard = render(Layer::Hard, map_tile, &[tile], &files).unwrap();
        assert_eq!(forest[0], [21, 128, 61, 115]);
        assert_eq!(hard[0], [226, 71, 35, 199]);
        let far = 200 * PIXELS + 200;
        assert_eq!(forest[far], TRANSPARENT);
        assert_eq!(hard[far], TRANSPARENT);
        let sea = render(Layer::Elevation, map_tile, &[tile], &[None]).unwrap();
        assert!(sea.iter().all(|&pixel| pixel == TRANSPARENT));
    }
}

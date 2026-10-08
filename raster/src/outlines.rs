//! The obstacles layers: every building filled in the colour of its height above the ground (its
//! rings filled even-odd, so courtyards stay open; one too small to cover a pixel's centre still
//! marks the pixel of its first corner; the taller over the lower), and every noise barrier drawn
//! as a line.

use crate::{Layer, MapTile, PIXELS, ramp};
use tiles::geo::{STEPS_PER_TILE, TileId};
use tiles::obstacles::{Obstacles, OutlineKind};

/// Building heights (m) and their colours, light for low houses and dark for towers.
const HEIGHT_STOPS: [(f64, [f64; 3]); 6] = [
    (3.0, [255.0, 237.0, 160.0]),
    (8.0, [254.0, 178.0, 76.0]),
    (15.0, [240.0, 59.0, 32.0]),
    (25.0, [189.0, 0.0, 38.0]),
    (50.0, [110.0, 1.0, 107.0]),
    (100.0, [40.0, 0.0, 70.0]),
];
/// Cyan: apart from the heatmap's purples and yellows and from the basemap.
const BARRIER: [u8; 4] = [0, 190, 255, 255];
/// A barrier's line, pixels across.
pub const BARRIER_WIDTH_PX: f64 = 3.0;

fn building_colour(height_m: f64) -> [u8; 4] {
    let [r, g, b] = ramp(&HEIGHT_STOPS, height_m).map(|value| value.round() as u8);
    [r, g, b, 255]
}

/// Fills the pixels whose centres lie inside an odd number of the rings (pixel coordinates).
fn fill_even_odd(rings: &[Vec<[f64; 2]>], colour: [u8; 4], pixels: &mut [[u8; 4]]) {
    let (top, bottom) = rings
        .iter()
        .flatten()
        .fold((f64::MAX, f64::MIN), |(top, bottom), point| {
            (top.min(point[1]), bottom.max(point[1]))
        });
    let first_row = (top - 0.5).ceil().max(0.0) as usize;
    let end_row = ((bottom - 0.5).floor() + 1.0).clamp(0.0, PIXELS as f64) as usize;
    let mut crossings = Vec::new();
    let mut filled = false;
    for row in first_row..end_row {
        let y = row as f64 + 0.5;
        crossings.clear();
        for ring in rings {
            for edge in ring.windows(2) {
                let ([x0, y0], [x1, y1]) = (edge[0], edge[1]);
                if (y0 <= y) != (y1 <= y) {
                    crossings.push(x0 + (y - y0) * (x1 - x0) / (y1 - y0));
                }
            }
        }
        crossings.sort_by(f64::total_cmp);
        for span in crossings.chunks_exact(2) {
            let first = (span[0] - 0.5).ceil().max(0.0) as usize;
            let end = ((span[1] - 0.5).ceil()).clamp(0.0, PIXELS as f64) as usize;
            for pixel in &mut pixels[row * PIXELS..(row + 1) * PIXELS][first.min(end)..end] {
                *pixel = colour;
                filled = true;
            }
        }
    }
    let [x, y] = rings[0][0];
    if !filled && (0.0..PIXELS as f64).contains(&x) && (0.0..PIXELS as f64).contains(&y) {
        pixels[y as usize * PIXELS + x as usize] = colour;
    }
}

/// Draws the pixels whose centres lie closer to the polyline than half its width.
pub(crate) fn draw_line(
    points: &[[f64; 2]],
    width_px: f64,
    colour: [u8; 4],
    pixels: &mut [[u8; 4]],
) {
    let radius = width_px / 2.0;
    for edge in points.windows(2) {
        let ([x0, y0], [x1, y1]) = (edge[0], edge[1]);
        let span = |a: f64, b: f64| {
            let first = (a.min(b) - radius - 0.5).ceil().max(0.0) as usize;
            let end = ((a.max(b) + radius - 0.5).floor() + 1.0).clamp(0.0, PIXELS as f64) as usize;
            first..end
        };
        let (dx, dy) = (x1 - x0, y1 - y0);
        let length_squared = (dx * dx + dy * dy).max(f64::MIN_POSITIVE);
        for row in span(y0, y1) {
            for column in span(x0, x1) {
                let (px, py) = (column as f64 + 0.5, row as f64 + 0.5);
                let along = (((px - x0) * dx + (py - y0) * dy) / length_squared).clamp(0.0, 1.0);
                let (ex, ey) = (px - (x0 + along * dx), py - (y0 + along * dy));
                if ex * ex + ey * ey < radius * radius {
                    pixels[row * PIXELS + column] = colour;
                }
            }
        }
    }
}

/// Whether points span a box that meets the tile widened by `margin` pixels.
pub(crate) fn near(points: &[[f64; 2]], margin: f64) -> bool {
    let (low, high) = points
        .iter()
        .fold(([f64::MAX; 2], [f64::MIN; 2]), |(low, high), &[x, y]| {
            (
                [low[0].min(x), low[1].min(y)],
                [high[0].max(x), high[1].max(y)],
            )
        });
    let (first, end) = (-margin, PIXELS as f64 + margin);
    low[0] < end && low[1] < end && high[0] > first && high[1] > first
}

/// The map tile's pixels from the obstacles files of `tiles` (absent: nothing there): for the
/// buildings the z12 tile holding the map tile, which stores every outline crossing it; for the
/// barriers also those within half a line of it, whose lines reach in.
pub fn render(
    layer: Layer,
    map_tile: MapTile,
    tiles: &[TileId],
    files: &[Option<Vec<u8>>],
) -> Result<Vec<[u8; 4]>, String> {
    let mut pixels = vec![[0; 4]; PIXELS * PIXELS];
    // A building's rings with its height; drawn lowest first, so where two overlap the taller
    // shows, as the computation takes the tallest building enclosing a point.
    let mut buildings: Vec<(f64, Vec<Vec<[f64; 2]>>)> = Vec::new();
    for (&tile, file) in tiles.iter().zip(files) {
        let Some(bytes) = file else { continue };
        let obstacles = Obstacles::parse(bytes).map_err(|error| error.to_string())?;
        let centre = map_tile.centre_of(tile);
        let points = |index: usize| -> Vec<[f64; 2]> {
            let record = obstacles.outline(index);
            (record.first_vertex..record.first_vertex + record.vertex_count)
                .map(|vertex| {
                    let local = obstacles.vertex(vertex);
                    [
                        (centre.x + f64::from(local[0]) / STEPS_PER_TILE - map_tile.west)
                            / map_tile.pixel,
                        (centre.y + f64::from(local[1]) / STEPS_PER_TILE - map_tile.north)
                            / map_tile.pixel,
                    ]
                })
                .collect()
        };
        let mut index = 0;
        while index < obstacles.outline_count() {
            let record = obstacles.outline(index);
            if record.kind == OutlineKind::Wall {
                if layer == Layer::Barriers {
                    let line = points(index);
                    if near(&line, BARRIER_WIDTH_PX) {
                        draw_line(&line, BARRIER_WIDTH_PX, BARRIER, &mut pixels);
                    }
                }
                index += 1;
                continue;
            }
            // A footprint's rings are consecutive: its exteriors and holes fill together.
            let first = index;
            while index < obstacles.outline_count()
                && obstacles.outline(index).footprint_id == record.footprint_id
                && obstacles.outline(index).kind.is_building()
            {
                index += 1;
            }
            if layer == Layer::Buildings {
                let rings: Vec<Vec<[f64; 2]>> = (first..index).map(points).collect();
                if rings.iter().any(|ring| near(ring, 0.0)) {
                    buildings.push((record.height_m, rings));
                }
            }
        }
    }
    buildings.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (height_m, rings) in &buildings {
        fill_even_odd(rings, building_colour(*height_m), &mut pixels);
    }
    Ok(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiles::obstacles::{EnvelopeClass, Outline, encode};

    fn ring(corners: &[[i16; 2]]) -> Vec<[i16; 2]> {
        corners.iter().chain(&corners[..1]).copied().collect()
    }

    /// A 10 m house with a courtyard and a barrier in the north-west quarter of a z12 tile, read
    /// as its zoom-13 tile: the walls filled, the courtyard open, the barrier a 3-pixel line.
    #[test]
    fn buildings_fill_around_their_courtyards_and_barriers_are_lines() {
        let tile = TileId { x: 2212, y: 1387 };
        // One z13 pixel is 64 steps; the house spans pixels 16-32 east and south of the corner.
        let corner = |px: i16, py: i16| [-16_384 + 64 * px, -16_384 + 64 * py];
        let outline = |kind, vertices| Outline {
            footprint_id: 7,
            kind,
            envelope: EnvelopeClass::Residential,
            height_m: 10.0,
            vertices,
        };
        let outlines = vec![
            outline(
                OutlineKind::Exterior,
                ring(&[
                    corner(16, 16),
                    corner(32, 16),
                    corner(32, 32),
                    corner(16, 32),
                ]),
            ),
            outline(
                OutlineKind::Hole,
                ring(&[
                    corner(20, 20),
                    corner(28, 20),
                    corner(28, 28),
                    corner(20, 28),
                ]),
            ),
            Outline {
                footprint_id: 9,
                kind: OutlineKind::Wall,
                envelope: EnvelopeClass::Outdoor,
                height_m: 4.0,
                // Along the centres of pixel row 50.
                vertices: vec![
                    [-16_384 + 6400, -16_384 + 3232],
                    [-16_384 + 12_800, -16_384 + 3232],
                ],
            },
        ];
        let bytes = encode(&outlines);
        let map_tile = MapTile::new(13, 2 * 2212, 2 * 1387);
        let buildings =
            render(Layer::Buildings, map_tile, &[tile], &[Some(bytes.clone())]).unwrap();
        let at = |pixels: &[[u8; 4]], column: usize, row: usize| pixels[row * PIXELS + column];
        let house = building_colour(10.0);
        assert_eq!(at(&buildings, 17, 17), house);
        assert_eq!(at(&buildings, 24, 24), [0; 4], "the courtyard");
        assert_eq!(at(&buildings, 15, 17), [0; 4]);
        assert_eq!(
            at(&buildings, 150, 50),
            [0; 4],
            "no barriers among the buildings"
        );
        let barriers = render(Layer::Barriers, map_tile, &[tile], &[Some(bytes.clone())]).unwrap();
        for row in [49, 50, 51] {
            assert_eq!(at(&barriers, 150, row), BARRIER);
        }
        assert_eq!(at(&barriers, 150, 48), [0; 4]);
        assert_eq!(at(&barriers, 150, 52), [0; 4]);
        assert_eq!(at(&barriers, 17, 17), [0; 4]);
        assert!(
            render(Layer::Buildings, map_tile, &[tile], &[None])
                .unwrap()
                .iter()
                .all(|&pixel| pixel == [0; 4])
        );
    }

    /// A barrier half a zoom-16 pixel west of a z12 tile's east edge reaches into the first column
    /// of the map tile east of it, which reads it from its western neighbour's file; where two
    /// buildings overlap the taller shows, whatever their order in the file.
    #[test]
    fn barriers_reach_across_tile_edges_and_taller_buildings_show() {
        let west = TileId { x: 2047, y: 2048 };
        let wall = Outline {
            footprint_id: 3,
            kind: OutlineKind::Wall,
            envelope: EnvelopeClass::Outdoor,
            height_m: 4.0,
            vertices: vec![[16_380, 0], [16_380, 64]],
        };
        let map_tile = MapTile::new(16, 32_768, 32_776);
        let tiles = map_tile.z12_tiles(BARRIER_WIDTH_PX / 2.0);
        assert_eq!(tiles, vec![west, TileId { x: 2048, y: 2048 }]);
        let files = vec![Some(encode(&[wall])), None];
        let barriers = render(Layer::Barriers, map_tile, &tiles, &files).unwrap();
        assert_eq!(barriers[3 * PIXELS], BARRIER);
        assert_eq!(barriers[3 * PIXELS + 2], [0; 4]);

        let square = |id, height_m, low: i16, high: i16| Outline {
            footprint_id: id,
            kind: OutlineKind::Exterior,
            envelope: EnvelopeClass::Commercial,
            height_m,
            vertices: ring(&[[low, low], [high, low], [high, high], [low, high]]),
        };
        let tile = TileId { x: 2212, y: 1387 };
        let map_tile = MapTile::new(12, 2212, 1387);
        let bytes = encode(&[square(7, 100.0, -2048, 2048), square(9, 3.0, -1024, 1024)]);
        let buildings = render(Layer::Buildings, map_tile, &[tile], &[Some(bytes)]).unwrap();
        assert_eq!(buildings[128 * PIXELS + 128], building_colour(100.0));
    }

    #[test]
    fn a_building_below_a_pixel_marks_its_corners_pixel() {
        let mut pixels = vec![[0; 4]; PIXELS * PIXELS];
        fill_even_odd(
            &[vec![[10.6, 10.6], [10.9, 10.6], [10.9, 10.9], [10.6, 10.6]]],
            [1, 2, 3, 255],
            &mut pixels,
        );
        assert_eq!(pixels[10 * PIXELS + 10], [1, 2, 3, 255]);
        assert_eq!(pixels.iter().filter(|&&pixel| pixel != [0; 4]).count(), 1);
    }

    #[test]
    fn heights_take_their_stops_colour() {
        assert_eq!(building_colour(1.0), [255, 237, 160, 255]);
        assert_eq!(building_colour(15.0), [240, 59, 32, 255]);
        assert_eq!(building_colour(20.0), [215, 30, 35, 255]);
        assert_eq!(building_colour(400.0), [40, 0, 70, 255]);
    }
}

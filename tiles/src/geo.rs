//! Web Mercator z12 tiles (XYZ, y grows south), int16 tile-local coordinates and a click's metre frame.

use std::f64::consts::PI;

/// Zoom of the prepared tiles.
pub const ZOOM: u32 = 12;
/// Tiles per axis at [`ZOOM`].
pub const TILES_PER_AXIS: u32 = 1 << ZOOM;
/// int16 steps per tile width: the tile spans +-16,384 steps around its centre, so half a tile of
/// margin fits on every side (30 cm steps at the equator, 19 cm in Prague).
pub const STEPS_PER_TILE: f64 = 32_768.0;
const STEPS_PER_TILE_I64: i64 = 32_768;
const WORLD_STEPS: i64 = STEPS_PER_TILE_I64 << ZOOM;
/// WGS84 semi-major axis in metres; also the Web Mercator (EPSG:3857) sphere radius.
pub const WGS84_A_M: f64 = 6_378_137.0;
/// WGS84 first eccentricity squared.
pub const WGS84_E2: f64 = 6.694_379_990_141_316e-3;
/// Web Mercator latitude limit in degrees (the square world).
pub const MAX_LATITUDE_DEG: f64 = 85.051_128_779_806_59;

/// A Web Mercator position in z12 tile units: `x` east in [0, 4096), `y` south in [0, 4096).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mercator {
    pub x: f64,
    pub y: f64,
}

impl Mercator {
    /// Latitude clamps to the Mercator limit; longitude wraps into [-180, 180).
    pub fn from_degrees(lat_deg: f64, lon_deg: f64) -> Self {
        let n = f64::from(TILES_PER_AXIS);
        let lon = (lon_deg + 180.0).rem_euclid(360.0) - 180.0;
        let phi = lat_deg
            .clamp(-MAX_LATITUDE_DEG, MAX_LATITUDE_DEG)
            .to_radians();
        Mercator {
            x: (lon + 180.0) / 360.0 * n,
            y: (1.0 - phi.tan().asinh() / PI) / 2.0 * n,
        }
    }

    /// (latitude, longitude) in degrees.
    pub fn to_degrees(self) -> (f64, f64) {
        let n = f64::from(TILES_PER_AXIS);
        let lat = (PI * (1.0 - 2.0 * self.y / n)).sinh().atan().to_degrees();
        (lat, self.x / n * 360.0 - 180.0)
    }

    /// The position with each coordinate within a millionth of a step of the step lattice put
    /// on it: a point that came from the lattice (a stored vertex or source end) and went through
    /// metres returns to it exactly, so a point on a tile's north or west edge stays in its tile.
    pub fn snapped_to_lattice(self) -> Self {
        let snap = |units: f64| {
            let steps = units * STEPS_PER_TILE;
            let nearest = steps.round();
            if (steps - nearest).abs() < LATTICE_SNAP_STEPS {
                nearest / STEPS_PER_TILE
            } else {
                units
            }
        };
        Mercator {
            x: snap(self.x),
            y: snap(self.y),
        }
    }
}

/// How close to a lattice point a position counts as on it (steps; 1e-6 is about 0.3 um).
pub const LATTICE_SNAP_STEPS: f64 = 1e-6;

/// One z12 tile in the standard XYZ numbering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileId {
    pub x: u32,
    pub y: u32,
}

impl TileId {
    pub fn containing(position: Mercator) -> Self {
        let last = f64::from(TILES_PER_AXIS - 1);
        TileId {
            x: position.x.floor().rem_euclid(f64::from(TILES_PER_AXIS)) as u32,
            y: position.y.floor().clamp(0.0, last) as u32,
        }
    }

    pub fn centre(self) -> Mercator {
        Mercator {
            x: f64::from(self.x) + 0.5,
            y: f64::from(self.y) + 0.5,
        }
    }

    /// The z9 directory holding this tile.
    pub fn z9(self) -> (u32, u32) {
        (self.x >> 3, self.y >> 3)
    }

    /// Position of a tile-local int16 coordinate pair (east, south).
    pub fn to_mercator(self, local: [i16; 2]) -> Mercator {
        let centre = self.centre();
        Mercator {
            x: centre.x + f64::from(local[0]) / STEPS_PER_TILE,
            y: centre.y + f64::from(local[1]) / STEPS_PER_TILE,
        }
    }

    /// Nearest tile-local coordinate, or `None` beyond the int16 margin. `x` is taken on the
    /// short way around the antimeridian.
    pub fn to_local(self, position: Mercator) -> Option<[i16; 2]> {
        let centre = self.centre();
        let steps = |delta: f64| {
            let q = (delta * STEPS_PER_TILE).round();
            (q >= f64::from(i16::MIN) && q <= f64::from(i16::MAX)).then_some(q as i16)
        };
        Some([
            steps(wrap_x(position.x - centre.x))?,
            steps(position.y - centre.y)?,
        ])
    }

    /// The tile's centre on the global step lattice.
    pub fn centre_steps(self) -> GlobalSteps {
        GlobalSteps {
            x: i64::from(self.x) * STEPS_PER_TILE_I64 + STEPS_PER_TILE_I64 / 2,
            y: i64::from(self.y) * STEPS_PER_TILE_I64 + STEPS_PER_TILE_I64 / 2,
        }
    }

    /// The global position of a tile-local coordinate pair.
    pub fn global(self, local: [i16; 2]) -> GlobalSteps {
        let centre = self.centre_steps();
        GlobalSteps {
            x: centre.x + i64::from(local[0]),
            y: centre.y + i64::from(local[1]),
        }
    }

    /// The tile-local coordinates of a global position (the short way around the antimeridian),
    /// or `None` beyond the int16 margin.
    pub fn local(self, global: GlobalSteps) -> Option<[i16; 2]> {
        let centre = self.centre_steps();
        let dx = (global.x - centre.x + WORLD_STEPS / 2).rem_euclid(WORLD_STEPS) - WORLD_STEPS / 2;
        Some([
            i16::try_from(dx).ok()?,
            i16::try_from(global.y - centre.y).ok()?,
        ])
    }

    /// Tiles at Chebyshev distance `k` (ring 0 is the tile itself), wrapping east-west and
    /// dropping rows beyond the poles.
    pub fn ring(self, k: u32) -> Vec<TileId> {
        let k = i64::from(k);
        let n = i64::from(TILES_PER_AXIS);
        let mut tiles = Vec::new();
        for dy in -k..=k {
            let y = i64::from(self.y) + dy;
            if !(0..n).contains(&y) {
                continue;
            }
            for dx in -k..=k {
                if dx.abs() != k && dy.abs() != k {
                    continue;
                }
                let x = (i64::from(self.x) + dx).rem_euclid(n);
                let tile = TileId {
                    x: x as u32,
                    y: y as u32,
                };
                if !tiles.contains(&tile) {
                    tiles.push(tile);
                }
            }
        }
        tiles
    }

    /// Ground width of the tile in metres (east-west at its centre latitude).
    pub fn width_m(self) -> f64 {
        LocalFrame::at(self.centre()).east_m_per_unit
    }
}

/// A position on the world's int16-step lattice: `x` east and `y` south from the north-west
/// corner, 32,768 steps per tile. Tile-local coordinates are exact shifts of it, so an outline
/// stored in several tiles has bit-identical global vertices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GlobalSteps {
    pub x: i64,
    pub y: i64,
}

impl GlobalSteps {
    pub fn nearest(position: Mercator) -> Self {
        GlobalSteps {
            x: (position.x * STEPS_PER_TILE).round() as i64,
            y: (position.y * STEPS_PER_TILE).round() as i64,
        }
    }

    pub fn to_mercator(self) -> Mercator {
        Mercator {
            x: self.x as f64 / STEPS_PER_TILE,
            y: self.y as f64 / STEPS_PER_TILE,
        }
    }
}

/// `dx` in tile units taken the short way around the world.
fn wrap_x(dx: f64) -> f64 {
    let n = f64::from(TILES_PER_AXIS);
    (dx + n / 2.0).rem_euclid(n) - n / 2.0
}

/// Metres east and north of a click's origin: Mercator scaled by the WGS84 radii of curvature at
/// the origin latitude. Over a 16 km reach the Mercator scale changes by ~0.3 % (Prague), under
/// 0.03 dB of divergence.
#[derive(Clone, Copy, Debug)]
pub struct LocalFrame {
    pub origin: Mercator,
    pub east_m_per_unit: f64,
    pub north_m_per_unit: f64,
}

impl LocalFrame {
    pub fn at(origin: Mercator) -> Self {
        let (lat, _) = origin.to_degrees();
        let phi = lat.to_radians();
        let w = 1.0 - WGS84_E2 * phi.sin().powi(2);
        let prime_vertical = WGS84_A_M / w.sqrt();
        let meridional = WGS84_A_M * (1.0 - WGS84_E2) / w.powf(1.5);
        let radians_per_unit = 2.0 * PI / f64::from(TILES_PER_AXIS);
        LocalFrame {
            origin,
            east_m_per_unit: prime_vertical * phi.cos() * radians_per_unit,
            north_m_per_unit: meridional * phi.cos() * radians_per_unit,
        }
    }

    /// [east, north] metres of a position.
    pub fn to_metres(&self, position: Mercator) -> [f64; 2] {
        [
            wrap_x(position.x - self.origin.x) * self.east_m_per_unit,
            (self.origin.y - position.y) * self.north_m_per_unit,
        ]
    }

    /// [east, north] metres of a (fractional) position on the global step lattice.
    pub fn metres_of_steps(&self, steps: [f64; 2]) -> [f64; 2] {
        self.to_metres(Mercator {
            x: steps[0] / STEPS_PER_TILE,
            y: steps[1] / STEPS_PER_TILE,
        })
    }

    /// The (fractional) global step position of [east, north] metres.
    pub fn steps_of_metres(&self, metres: [f64; 2]) -> [f64; 2] {
        let position = self.to_mercator(metres);
        [position.x * STEPS_PER_TILE, position.y * STEPS_PER_TILE]
    }

    pub fn to_mercator(&self, metres: [f64; 2]) -> Mercator {
        Mercator {
            x: self.origin.x + metres[0] / self.east_m_per_unit,
            y: self.origin.y - metres[1] / self.north_m_per_unit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prague_lies_in_the_known_z9_directory() {
        let tile = TileId::containing(Mercator::from_degrees(50.07553, 14.43781));
        assert_eq!(tile.z9(), (276, 173));
        assert_eq!(tile, TileId { x: 2212, y: 1387 });
    }

    #[test]
    fn degrees_round_trip() {
        for (lat, lon) in [
            (50.07553, 14.43781),
            (-23.5505, -46.6333),
            (59.9127, 10.7461),
            (0.0, -179.99),
        ] {
            let (back_lat, back_lon) = Mercator::from_degrees(lat, lon).to_degrees();
            assert!((back_lat - lat).abs() < 1e-9 && (back_lon - lon).abs() < 1e-9);
        }
    }

    #[test]
    fn local_coordinates_cover_half_a_tile_of_margin() {
        let tile = TileId { x: 2212, y: 1387 };
        let edge = Mercator {
            x: f64::from(tile.x) - 0.49,
            y: f64::from(tile.y) + 1.49,
        };
        let local = tile.to_local(edge).expect("inside the margin");
        let back = tile.to_mercator(local);
        assert!((back.x - edge.x).abs() <= 0.5 / STEPS_PER_TILE);
        assert!((back.y - edge.y).abs() <= 0.5 / STEPS_PER_TILE);
        assert_eq!(
            tile.to_local(Mercator {
                x: f64::from(tile.x) - 0.51,
                y: tile.centre().y
            }),
            None
        );
    }

    #[test]
    fn global_steps_are_exact_shifts_between_neighbours() {
        let (west, east) = (TileId { x: 2212, y: 1387 }, TileId { x: 2213, y: 1387 });
        let point = west.global([16_000, -300]);
        assert_eq!(west.local(point), Some([16_000, -300]));
        assert_eq!(east.local(point), Some([-16_768, -300]));
        assert_eq!(east.local(west.global([-20_000, 0])), None);
        let edge = TileId {
            x: TILES_PER_AXIS - 1,
            y: 5,
        };
        assert_eq!(
            TileId { x: 0, y: 5 }.local(edge.global([100, 0])),
            Some([-32_668, 0])
        );
        let mercator = point.to_mercator();
        assert_eq!(GlobalSteps::nearest(mercator), point);
        assert_eq!(west.to_local(mercator), Some([16_000, -300]));
    }

    #[test]
    fn prague_tile_is_six_kilometres_wide_with_19_cm_steps() {
        let width = TileId { x: 2212, y: 1387 }.width_m();
        assert!((6_270.0..6_300.0).contains(&width), "{width}");
        assert!((width / STEPS_PER_TILE - 0.19).abs() < 0.005);
    }

    #[test]
    fn frame_matches_wgs84_distances() {
        // One degree of latitude at 50 N is 111,229 m and one of longitude 71,696 m (WGS84).
        let origin = Mercator::from_degrees(50.0, 14.0);
        let frame = LocalFrame::at(origin);
        let north = frame.to_metres(Mercator::from_degrees(50.005, 14.0))[1] / 0.005;
        let east = frame.to_metres(Mercator::from_degrees(50.0, 14.005))[0] / 0.005;
        assert!((north - 111_229.0).abs() < 60.0, "{north}");
        assert!((east - 71_696.0).abs() < 2.0, "{east}");
    }

    #[test]
    fn ring_wraps_the_antimeridian_and_stops_at_the_poles() {
        let tile = TileId { x: 0, y: 0 };
        let ring = tile.ring(1);
        assert_eq!(ring.len(), 5);
        assert!(ring.contains(&TileId {
            x: TILES_PER_AXIS - 1,
            y: 1
        }));
        assert_eq!(TileId { x: 100, y: 100 }.ring(2).len(), 16);
        assert_eq!(
            TileId { x: 100, y: 100 }.ring(0),
            vec![TileId { x: 100, y: 100 }]
        );
    }

    #[test]
    fn frame_takes_the_short_way_across_the_antimeridian() {
        let frame = LocalFrame::at(Mercator::from_degrees(0.0, 179.99));
        let east = frame.to_metres(Mercator::from_degrees(0.0, -179.99))[0];
        assert!((east - 2_226.4).abs() < 1.0, "{east}");
    }
}

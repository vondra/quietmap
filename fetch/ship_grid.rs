//! Native vessel-cell geometry and coverage, independent of publisher identifiers.
use super::ship_raster::Geometry;
use proj4rs::{Proj, transform::transform};
use serde::{Deserialize, Serialize};

// Publisher's geographic area convention and the acoustic placement's local metre conversions.
const AREA_METRES_PER_DEGREE: f64 = 111_320.0;
const EAST_METRES_PER_DEGREE: f64 = 111_320.0;
const NORTH_METRES_PER_DEGREE: f64 = 110_540.0;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NativeGrid {
    pub epsg: u16,
    pub width: f64,
    pub height: f64,
}

pub struct Grid {
    pub native: NativeGrid,
    projection: Proj,
    geographic: Proj,
}

impl Grid {
    pub fn new(native: NativeGrid) -> Result<Self, String> {
        if !native.width.is_finite()
            || !native.height.is_finite()
            || native.width <= 0.0
            || native.height <= 0.0
        {
            return Err("invalid ship cell dimensions".into());
        }
        let projection =
            Proj::from_user_string(&format!("EPSG:{}", native.epsg)).map_err(|e| e.to_string())?;
        if !(projection.is_latlong()
            || projection.projname() == "laea" && projection.to_meter() == 1.0)
        {
            return Err(
                "ship grids require geographic degrees or metre-based equal-area LAEA".into(),
            );
        }
        Ok(Self {
            native,
            projection,
            geographic: Proj::from_user_string("EPSG:4326").map_err(|e| e.to_string())?,
        })
    }

    pub fn degrees(&self, (x, y): (f64, f64)) -> Result<(f64, f64), String> {
        let mut point = if self.projection.is_latlong() {
            (x.to_radians(), y.to_radians(), 0.0)
        } else {
            (x, y, 0.0)
        };
        transform(&self.projection, &self.geographic, &mut point).map_err(|e| e.to_string())?;
        Ok((point.0.to_degrees(), point.1.to_degrees()))
    }

    pub fn coordinates(&self, longitude: f64, latitude: f64) -> Result<(f64, f64), String> {
        let mut point = (longitude.to_radians(), latitude.to_radians(), 0.0);
        transform(&self.geographic, &self.projection, &mut point).map_err(|e| e.to_string())?;
        Ok(if self.projection.is_latlong() {
            (point.0.to_degrees(), point.1.to_degrees())
        } else {
            (point.0, point.1)
        })
    }

    pub fn area_m2(&self, latitude: f64) -> f64 {
        if self.projection.is_latlong() {
            let [width, height] =
                [self.native.width, self.native.height].map(|d| d * AREA_METRES_PER_DEGREE);
            width * height * latitude.to_radians().cos()
        } else {
            self.native.width * self.native.height
        }
    }

    /// LAEA's spherical inverse Jacobian preserves the established acoustic placement. The
    /// projection supplies its origin; rectangular cells retain their width-to-height ratio.
    pub fn axes(&self, centre: (f64, f64), area_m2: f64) -> [[f64; 2]; 2] {
        if self.projection.is_latlong() {
            return [
                [
                    self.native.width * (EAST_METRES_PER_DEGREE * centre.0.to_radians().cos()),
                    0.0,
                ],
                [0.0, self.native.height * NORTH_METRES_PER_DEGREE],
            ];
        }
        let (phi0, lon0) = (
            self.projection.latitude_of_origin(),
            self.projection.longitude_of_origin().to_degrees(),
        );
        let project = |lat: f64, lon: f64| {
            let (phi, lambda) = (lat.to_radians(), (lon - lon0).to_radians());
            let cos_c = phi0.sin() * phi.sin() + phi0.cos() * phi.cos() * lambda.cos();
            let k = (2.0 / (1.0 + cos_c)).sqrt();
            let y = phi0.cos() * phi.sin() - phi0.sin() * phi.cos() * lambda.cos();
            [k * phi.cos() * lambda.sin(), k * y]
        };
        let metres_per_lon = EAST_METRES_PER_DEGREE * centre.0.to_radians().cos();
        let at = project(centre.0, centre.1);
        let [east, north] = [
            project(centre.0, centre.1 + 10.0 / metres_per_lon),
            project(centre.0 + 10.0 / NORTH_METRES_PER_DEGREE, centre.1),
        ]
        .map(|moved| [moved[0] - at[0], moved[1] - at[1]]);
        let det = east[0] * north[1] - north[0] * east[1];
        let steps = [
            [north[1] / det, -east[1] / det],
            [-north[0] / det, east[0] / det],
        ];
        let scale = (area_m2 * det.abs() / 100.0).sqrt();
        let aspect = (self.native.width / self.native.height).sqrt();
        [
            steps[0].map(|m| m * 10.0 * scale * aspect),
            steps[1].map(|m| m * 10.0 * scale / aspect),
        ]
    }
}

pub struct Coverage {
    pub geometry: Geometry,
    pub mask: Vec<bool>,
    pub grid: Grid,
}
impl Coverage {
    pub fn new(geometry: Geometry, mask: Vec<bool>) -> Result<Self, String> {
        if mask.len() != geometry.columns * geometry.rows {
            return Err("coverage dimensions differ".into());
        }
        let grid = Grid::new(NativeGrid {
            epsg: geometry.epsg,
            width: geometry.width,
            height: geometry.height,
        })?;
        Ok(Self {
            geometry,
            mask,
            grid,
        })
    }
    pub fn centre(&self, index: usize) -> Result<(f64, f64), String> {
        self.grid.degrees(self.geometry.centre(index))
    }
    pub fn covers(&self, longitude: f64, latitude: f64) -> bool {
        self.grid
            .coordinates(longitude, latitude)
            .ok()
            .and_then(|(x, y)| self.geometry.index(x, y))
            .is_some_and(|index| self.mask[index])
    }
}

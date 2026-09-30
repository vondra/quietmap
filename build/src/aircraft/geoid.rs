//! EGM2008 geoid undulation (WGS84 ellipsoid minus EGM2008 height) on a 10' grid, built once from
//! the PROJ 2.5' GeoTIFF by keeping every fourth node: bilinear interpolation on it errs at most
//! 0.97 m (p99 0.15 m, 2 M random points) against the full grid, far below the 25 ft step of ADS-B
//! geometric altitude. File: magic, then 1,081 rows (90 N to 90 S) of 2,160 i16 centimetres (from
//! 180 W eastwards).

use std::path::Path;

const MAGIC: &[u8; 8] = b"qmgeoid1";
/// Grid nodes per degree (10').
const NODES_PER_DEGREE: usize = 6;
const COLUMNS: usize = 360 * NODES_PER_DEGREE;
const ROWS: usize = 180 * NODES_PER_DEGREE + 1;
/// The source: PROJ's `us_nga_egm08_25.tif`, 2.5' pixels centred on the nodes.
const SOURCE_NODES_PER_DEGREE: usize = 24;

pub struct Geoid {
    centimetres: Vec<i16>,
}

impl Geoid {
    pub fn read(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
        if bytes.len() != MAGIC.len() + 2 * ROWS * COLUMNS || &bytes[..8] != MAGIC {
            return Err(format!("{}: not a geoid grid", path.display()));
        }
        let centimetres = bytes[8..]
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        Ok(Geoid { centimetres })
    }

    /// Undulation in metres at (lat, lon), bilinear between the four surrounding nodes.
    pub fn undulation_m(&self, lat: f64, lon: f64) -> f64 {
        let y = (90.0 - lat.clamp(-90.0, 90.0)) * NODES_PER_DEGREE as f64;
        let x = (lon + 180.0).rem_euclid(360.0) * NODES_PER_DEGREE as f64;
        let row = (y.floor() as usize).min(ROWS - 2);
        let column = (x.floor() as usize) % COLUMNS;
        let (fy, fx) = (y - row as f64, x - x.floor());
        let node =
            |r: usize, c: usize| f64::from(self.centimetres[r * COLUMNS + c % COLUMNS]) / 100.0;
        let north = node(row, column) * (1.0 - fx) + node(row, column + 1) * fx;
        let south = node(row + 1, column) * (1.0 - fx) + node(row + 1, column + 1) * fx;
        north * (1.0 - fy) + south * fy
    }
}

/// `qm-build geoid`: the 10' grid from the 2.5' GeoTIFF.
pub fn build(tiff_path: &Path, out: &Path) -> Result<(), String> {
    let failed = |error: tiff::TiffError| format!("{}: {error}", tiff_path.display());
    let file = std::fs::File::open(tiff_path)
        .map_err(|error| format!("{}: {error}", tiff_path.display()))?;
    let mut decoder = tiff::decoder::Decoder::new(std::io::BufReader::new(file)).map_err(failed)?;
    let (width, height) = decoder.dimensions().map_err(failed)?;
    let (width, height) = (width as usize, height as usize);
    if (width, height)
        != (
            360 * SOURCE_NODES_PER_DEGREE,
            180 * SOURCE_NODES_PER_DEGREE + 1,
        )
    {
        return Err(format!(
            "{}: {width} x {height} is not the 2.5' grid",
            tiff_path.display()
        ));
    }
    let tiff::decoder::DecodingResult::F32(values) = decoder.read_image().map_err(failed)? else {
        return Err(format!("{}: not float32", tiff_path.display()));
    };
    let step = SOURCE_NODES_PER_DEGREE / NODES_PER_DEGREE;
    let mut bytes = Vec::with_capacity(MAGIC.len() + 2 * ROWS * COLUMNS);
    bytes.extend_from_slice(MAGIC);
    for row in 0..ROWS {
        for column in 0..COLUMNS {
            let metres = values[row * step * width + column * step];
            if !(-200.0..200.0).contains(&metres) {
                return Err(format!("{}: undulation {metres} m", tiff_path.display()));
            }
            bytes.extend_from_slice(&((metres * 100.0).round() as i16).to_le_bytes());
        }
    }
    let temporary = out.with_extension("partial");
    std::fs::write(&temporary, &bytes)
        .map_err(|error| format!("{}: {error}", temporary.display()))?;
    std::fs::rename(&temporary, out).map_err(|error| format!("{}: {error}", out.display()))
}

#[cfg(test)]
impl Geoid {
    /// A grid of one value everywhere.
    pub fn uniform(metres: f64) -> Self {
        Geoid {
            centimetres: vec![(metres * 100.0).round() as i16; ROWS * COLUMNS],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolation_is_bilinear_and_wraps_the_antimeridian() {
        let mut geoid = Geoid::uniform(0.0);
        // A node at 50 N 14 E of +45 m.
        let (row, column) = (40 * NODES_PER_DEGREE, 194 * NODES_PER_DEGREE);
        geoid.centimetres[row * COLUMNS + column] = 4500;
        assert_eq!(geoid.undulation_m(50.0, 14.0), 45.0);
        assert!((geoid.undulation_m(50.0, 14.0 + 1.0 / 12.0) - 22.5).abs() < 1e-9);
        assert!((geoid.undulation_m(50.0 - 1.0 / 24.0, 14.0) - 33.75).abs() < 1e-9);
        geoid.centimetres[row * COLUMNS] = -1000;
        assert!((geoid.undulation_m(50.0, 180.0) + 10.0).abs() < 1e-9);
        assert!((geoid.undulation_m(50.0, 179.0 + 11.0 / 12.0) + 5.0).abs() < 1e-9);
        assert_eq!(geoid.undulation_m(-90.0, 0.0), 0.0);
    }
}

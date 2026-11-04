//! Decode named publisher GeoTIFF objects into a common north-up raster with explicit nodata.

use std::io::{Cursor, Read};
use std::path::{Component, Path};
use tiff::decoder::{Decoder, DecodingResult, Limits};
use tiff::tags::Tag;

pub fn relative_path(root: &Path, path: &str) -> Result<std::path::PathBuf, String> {
    let relative = Path::new(path);
    if path.is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(format!("invalid input-relative path {path:?}"));
    }
    Ok(root.join(relative))
}

pub fn bytes(path: &Path, member: &str) -> Result<Vec<u8>, String> {
    let mut archive = zip::ZipArchive::new(
        std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?,
    )
    .map_err(|e| e.to_string())?;
    let mut file = archive
        .by_name(member)
        .map_err(|e| format!("{}:{member}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Geometry {
    pub columns: usize,
    pub rows: usize,
    pub west: f64,
    pub north: f64,
    pub width: f64,
    pub height: f64,
    pub epsg: u16,
}

impl Geometry {
    pub fn centre(&self, index: usize) -> (f64, f64) {
        (
            self.width * ((index % self.columns) as f64 + 0.5) + self.west,
            self.north - self.height * ((index / self.columns) as f64 + 0.5),
        )
    }

    pub fn index(&self, x: f64, y: f64) -> Option<usize> {
        let column = ((x - self.west) / self.width).floor();
        let row = ((self.north - y) / self.height).floor();
        (column >= 0.0 && column < self.columns as f64 && row >= 0.0 && row < self.rows as f64)
            .then_some(row as usize * self.columns + column as usize)
    }
}

pub struct Raster {
    pub geometry: Geometry,
    values: DecodingResult,
    pub valid: Vec<bool>,
}

impl Raster {
    pub fn read(bytes: Vec<u8>) -> Result<Self, String> {
        let mut decoder = Decoder::new(Cursor::new(bytes))
            .map_err(|error| error.to_string())?
            .with_limits(Limits::unlimited());
        let (columns, rows) = decoder.dimensions().map_err(|error| error.to_string())?;
        let scale = decoder
            .get_tag_f64_vec(Tag::ModelPixelScaleTag)
            .map_err(|error| error.to_string())?;
        let ties = decoder
            .get_tag_f64_vec(Tag::ModelTiepointTag)
            .map_err(|error| error.to_string())?;
        let keys = decoder
            .get_tag_u16_vec(Tag::GeoKeyDirectoryTag)
            .map_err(|error| error.to_string())?;
        let key = |code| {
            keys.get(4..)
                .and_then(|keys| {
                    keys.chunks_exact(4)
                        .find(|entry| entry[0] == code && entry[1] == 0 && entry[2] == 1)
                })
                .map(|entry| entry[3])
        };
        let epsg = key(3072)
            .or_else(|| key(2048))
            .ok_or("GeoTIFF lacks its EPSG code")?;
        // GeoTIFF defaults to PixelIsArea; PixelIsPoint ties identify sample centres.
        let offset = match key(1025).unwrap_or(1) {
            1 => 0.0,
            2 => 0.5,
            _ => return Err("unsupported GeoTIFF raster type".into()),
        };
        let nodata: f64 = decoder
            .get_tag_ascii_string(Tag::Unknown(42113))
            .map_err(|error| error.to_string())?
            .trim_end_matches('\0')
            .parse()
            .map_err(|_| "invalid raster nodata")?;
        if scale.len() != 3
            || ties.len() != 6
            || scale.iter().chain(&ties).any(|value| !value.is_finite())
            || ties[0] != 0.0
            || ties[1] != 0.0
            || scale[0] <= 0.0
            || scale[1] <= 0.0
        {
            return Err("unsupported raster affine transform".into());
        }
        let geometry = Geometry {
            columns: columns as usize,
            rows: rows as usize,
            west: ties[3] - offset * scale[0],
            north: ties[4] + offset * scale[1],
            width: scale[0],
            height: scale[1],
            epsg,
        };
        let values = decoder.read_image().map_err(|error| error.to_string())?;
        let length = match &values {
            DecodingResult::F32(values) => values.len(),
            DecodingResult::I32(values) => values.len(),
            _ => return Err("expected a float32 density or int32 vessel-hour raster".into()),
        };
        if length != geometry.rows * geometry.columns {
            return Err("ship raster must have one band".into());
        }
        let mut raster = Self {
            geometry,
            values,
            valid: Vec::new(),
        };
        raster.valid = (0..length)
            .map(|index| {
                let value = raster.value(index);
                value.is_finite() && value != nodata
            })
            .collect();
        Ok(raster)
    }

    pub fn value(&self, index: usize) -> f64 {
        match &self.values {
            DecodingResult::F32(values) => f64::from(values[index]),
            DecodingResult::I32(values) => f64::from(values[index]),
            _ => unreachable!("validated single-band ship raster"),
        }
    }
}

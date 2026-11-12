//! Open one manifest-selected GDAL raster, with native-pixel masks before resampling.
use super::config::RasterSource;
use gdal::Dataset;
use std::path::PathBuf;

pub struct Source {
    pub spec: RasterSource,
    pub path: PathBuf,
    pub dataset: Dataset,
    pub observed_year: i32,
    geographic_bounds: [f64; 4],
    pub antimeridian: Option<Box<Source>>,
    /// The country label of a national ground-height model: it holds heights on no other country's land.
    pub country: Option<u16>,
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

impl Source {
    pub(super) fn reopen(&self) -> Result<Self, String> {
        let mut source = Self::open(self.spec.clone(), self.path.clone(), self.observed_year)?;
        source.country = self.country;
        source.antimeridian = self
            .antimeridian
            .as_ref()
            .map(|wrapped| wrapped.reopen().map(Box::new))
            .transpose()?;
        Ok(source)
    }

    pub fn open(spec: RasterSource, path: PathBuf, observed_year: i32) -> Result<Self, String> {
        let dataset = Dataset::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if dataset.raster_count() != 1 {
            return Err(format!("{}: expected one raster band", path.display()));
        }
        let band = dataset.rasterband(1).map_err(|e| e.to_string())?;
        let canonical = |value: f64| {
            if band.band_type() == gdal::raster::GdalDataType::Float32 {
                value as f32 as f64
            } else {
                value
            }
        };
        let same_nodata = match (spec.nodata, band.no_data_value()) {
            (None, None) => true,
            (Some(a), Some(b)) => (a.is_nan() && b.is_nan()) || canonical(a) == canonical(b),
            _ => false,
        };
        if !same_nodata {
            return Err(format!("{}: nodata differs from the manifest", spec.input));
        }
        let transform = dataset.geo_transform().map_err(|e| e.to_string())?;
        if transform[1] <= 0.0 || transform[5] >= 0.0 || transform[2] != 0.0 || transform[4] != 0.0
        {
            return Err(format!("{}: expected a north-up raster", path.display()));
        }
        let (width, height) = dataset.raster_size();
        let mut from = gdal::spatial_ref::SpatialRef::from_definition(&spec.horizontal_crs)
            .map_err(|e| e.to_string())?;
        let mut to = gdal::spatial_ref::SpatialRef::from_epsg(4326).map_err(|e| e.to_string())?;
        for crs in [&mut from, &mut to] {
            crs.set_axis_mapping_strategy(
                gdal::spatial_ref::AxisMappingStrategy::TraditionalGisOrder,
            );
        }
        let mut declared = dataset.spatial_ref().map_err(|e| e.to_string())?;
        declared
            .set_axis_mapping_strategy(gdal::spatial_ref::AxisMappingStrategy::TraditionalGisOrder);
        if declared != from {
            return Err(format!("{}: CRS differs from the manifest", spec.input));
        }
        let geographic_bounds = gdal::spatial_ref::CoordTransform::new(&from, &to)
            .and_then(|t| {
                t.transform_bounds(
                    &[
                        transform[0],
                        transform[3] + height as f64 * transform[5],
                        transform[0] + width as f64 * transform[1],
                        transform[3],
                    ],
                    21,
                )
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            spec,
            path,
            dataset,
            observed_year,
            geographic_bounds,
            antimeridian: None,
            country: None,
        })
    }

    pub fn for_window(&self, window: tiles::terrain::Window) -> &Self {
        let [west, _, east, _] = super::warp::bounds(window);
        if west < -180.0 || east > 180.0 {
            self.antimeridian.as_deref().unwrap_or(self)
        } else {
            self
        }
    }

    pub fn intersects(&self, window: tiles::terrain::Window) -> bool {
        let [west, south, east, north] = super::warp::bounds(window);
        let [left, bottom, right, top] = self.geographic_bounds;
        bottom <= north && top >= south && ((left <= east && right >= west) || left > right)
    }

    pub fn mapped(&self, lookup: &str, nodata: f64) -> Result<Dataset, String> {
        let (width, height) = self.dataset.raster_size();
        let transform = self
            .dataset
            .geo_transform()
            .map_err(|e| e.to_string())?
            .map(|v| v.to_string())
            .join(",");
        let original_nodata = self
            .spec
            .nodata
            .map(|v| format!("<NODATA>{v}</NODATA>"))
            .unwrap_or_default();
        let text = format!(
            "<VRTDataset rasterXSize=\"{width}\" rasterYSize=\"{height}\"><SRS>{}</SRS><GeoTransform>{transform}</GeoTransform><VRTRasterBand dataType=\"Byte\" band=\"1\"><NoDataValue>{nodata}</NoDataValue><ComplexSource><SourceFilename relativeToVRT=\"0\">{}</SourceFilename><SourceBand>1</SourceBand>{original_nodata}<LUT>{lookup}</LUT><SrcRect xOff=\"0\" yOff=\"0\" xSize=\"{width}\" ySize=\"{height}\"/><DstRect xOff=\"0\" yOff=\"0\" xSize=\"{width}\" ySize=\"{height}\"/></ComplexSource></VRTRasterBand></VRTDataset>",
            xml(&self.spec.horizontal_crs),
            xml(&self.path.to_string_lossy())
        );
        Dataset::open(std::path::Path::new(&text)).map_err(|e| e.to_string())
    }
}

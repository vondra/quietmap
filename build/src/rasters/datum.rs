//! Per-node vertical conversion with explicit PROJ error codes for unavailable grid cells.
use super::{source::Source, warp};
use gdal::spatial_ref::{AxisMappingStrategy, SpatialRef};
use tiles::terrain::{NODES_PER_DEGREE, Window};

struct DatumTransform(gdal_sys::OGRCoordinateTransformationH);
impl Drop for DatumTransform {
    fn drop(&mut self) {
        unsafe { gdal_sys::OCTDestroyCoordinateTransformation(self.0) }
    }
}

impl DatumTransform {
    fn new(from: &SpatialRef, to: &SpatialRef, area: Option<[f64; 4]>) -> Result<Self, String> {
        unsafe {
            let options = gdal_sys::OCTNewCoordinateTransformationOptions();
            if options.is_null() {
                return Err(warp::error());
            }
            gdal_sys::OCTCoordinateTransformationOptionsSetBallparkAllowed(options, 0);
            if let Some([west, south, east, north]) = area {
                gdal_sys::OCTCoordinateTransformationOptionsSetAreaOfInterest(
                    options, west, south, east, north,
                );
            }
            let transform = gdal_sys::OCTNewCoordinateTransformationEx(
                from.to_c_hsrs(),
                to.to_c_hsrs(),
                options,
            );
            gdal_sys::OCTDestroyCoordinateTransformationOptions(options);
            if transform.is_null() {
                return Err(warp::error());
            }
            Ok(Self(transform))
        }
    }

    fn row(
        &self,
        longitude: &mut [f64],
        latitude: &mut [f64],
        heights: &mut [f64],
    ) -> Result<(), String> {
        let mut errors = vec![-1; heights.len()];
        unsafe {
            // The per-point codes below retain errors without flooding stderr for expected grid edges.
            gdal_sys::CPLPushErrorHandler(Some(gdal_sys::CPLQuietErrorHandler));
            gdal_sys::OCTTransform4DWithErrorCodes(
                self.0,
                heights.len() as i32,
                longitude.as_mut_ptr(),
                latitude.as_mut_ptr(),
                heights.as_mut_ptr(),
                std::ptr::null_mut(),
                errors.as_mut_ptr(),
            );
            gdal_sys::CPLPopErrorHandler();
        }
        for i in 0..heights.len() {
            node_height(errors[i], longitude[i], latitude[i], &mut heights[i])?;
        }
        Ok(())
    }
}

/// One node's converted height by its PROJ code. A node beyond the datum's reach is missing, and
/// the sources below it fill it: a national model reaches past the datum's operations and grids,
/// as CA-MRDEM-30 into Alaska, RGE ALTI into Andorra and MDT05 over the sea off Galicia.
fn node_height(code: i32, longitude: f64, latitude: f64, height: &mut f64) -> Result<(), String> {
    match code {
        0 if longitude.is_finite() && latitude.is_finite() && height.is_finite() => Ok(()),
        // PROJ 9.4 can return NaN with code zero at an exactly missing grid node:
        // its nodata interpolation divides zero by zero. Only vertical NaN is missing.
        0 if longitude.is_finite() && latitude.is_finite() && height.is_nan() => Ok(()),
        // PROJ proj.h: COORD_TRANSFM (2048) + NO_OPERATION (3) / OUTSIDE_GRID (4) / GRID_AT_NODATA (5).
        2051..=2053 => {
            *height = f64::NAN;
            Ok(())
        }
        code => Err(format!(
            "datum conversion failed with PROJ code {code}: {}",
            warp::error()
        )),
    }
}

pub fn convert(values: &mut [f64], window: Window, source: &Source) -> Result<(), String> {
    let vertical = source
        .spec
        .vertical_crs
        .ok_or("ground height needs a vertical CRS")?;
    if vertical == 3855 {
        return Ok(());
    }
    let horizontal = source
        .spec
        .datum_horizontal_crs
        .as_deref()
        .unwrap_or("EPSG:4326");
    let mut from = SpatialRef::from_definition(&format!("{horizontal}+{vertical}"))
        .map_err(|e| e.to_string())?;
    let mut to = SpatialRef::from_definition("EPSG:4326+3855").map_err(|e| e.to_string())?;
    for crs in [&mut from, &mut to] {
        crs.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
    }
    let transform = DatumTransform::new(&from, &to, source.spec.datum_area)?;
    for (row, heights) in values.chunks_exact_mut(window.columns as usize).enumerate() {
        let indices: Vec<_> = heights
            .iter()
            .enumerate()
            .filter(|(_, h)| h.is_finite())
            .map(|(i, _)| i)
            .collect();
        if indices.is_empty() {
            continue;
        }
        let mut longitudes: Vec<_> = indices
            .iter()
            .map(|i| (window.west_node as f64 + *i as f64) / NODES_PER_DEGREE as f64)
            .collect();
        let mut latitudes =
            vec![(window.north_node as f64 - row as f64) / NODES_PER_DEGREE as f64; indices.len()];
        let mut elevations: Vec<_> = indices.iter().map(|i| heights[*i]).collect();
        transform
            .row(&mut longitudes, &mut latitudes, &mut elevations)
            .map_err(|error| format!("{}: {window:?}, row {row}: {error}", source.spec.input))?;
        for (i, height) in indices.into_iter().zip(elevations) {
            heights[i] = height;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_exactly_missing_vertical_node_does_not_abort_valid_neighbours() {
        use gdal::Metadata;
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("vertical.tif");
        let mut dataset = gdal::DriverManager::get_driver_by_name("GTiff")
            .unwrap()
            .create_with_band_type::<f32, _>(&path, 4, 4, 1)
            .unwrap();
        dataset
            .set_geo_transform(&[-0.5, 1.0, 0.0, 3.5, 0.0, -1.0])
            .unwrap();
        dataset.set_projection("EPSG:4326").unwrap();
        dataset
            .set_metadata_item("TYPE", "VERTICAL_OFFSET_GEOGRAPHIC_TO_VERTICAL", "")
            .unwrap();
        let mut values = vec![-32768.0f32; 16];
        values[6] = 12.0;
        let mut band = dataset.rasterband(1).unwrap();
        band.set_no_data_value(Some(-32768.0)).unwrap();
        band.write(
            (0, 0),
            (4, 4),
            &mut gdal::raster::Buffer::new((4, 4), values),
        )
        .unwrap();
        dataset.flush_cache().unwrap();
        drop(dataset);
        let mut crs = SpatialRef::from_epsg(4326).unwrap();
        crs.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
        let operation = std::ffi::CString::new(format!(
            "+proj=pipeline +step +proj=unitconvert +xy_in=deg +xy_out=rad +step +proj=vgridshift +grids={} +multiplier=1 +step +proj=unitconvert +xy_in=rad +xy_out=deg",path.display()
        )).unwrap();
        let transform = unsafe {
            let options = gdal_sys::OCTNewCoordinateTransformationOptions();
            assert!(!options.is_null());
            assert_ne!(
                gdal_sys::OCTCoordinateTransformationOptionsSetOperation(
                    options,
                    operation.as_ptr(),
                    0
                ),
                0
            );
            let handle = gdal_sys::OCTNewCoordinateTransformationEx(
                crs.to_c_hsrs(),
                crs.to_c_hsrs(),
                options,
            );
            gdal_sys::OCTDestroyCoordinateTransformationOptions(options);
            assert!(!handle.is_null());
            DatumTransform(handle)
        };
        let mut longitude = [1.0, 1.25];
        let mut latitude = [2.0, 2.0];
        let mut height = [10.0, 10.0];
        transform
            .row(&mut longitude, &mut latitude, &mut height)
            .unwrap();
        assert!(height[0].is_nan());
        assert_eq!(height[1], 22.0);
    }

    #[test]
    fn heights_beyond_the_datums_reach_are_missing_not_an_abort() {
        // CA-MRDEM-30's Alaskan pixels (no operation), RGE ALTI's in Andorra and MDT05's over the sea
        // off Galicia stopped z9 squares 55:116, 55:148, 258:189 and 241:186-189 and their whole run.
        for code in [2051, 2052, 2053] {
            let mut height = 100.0;
            assert_eq!(node_height(code, -150.0, 64.0, &mut height), Ok(()));
            assert!(height.is_nan());
        }
        assert!(node_height(2049, -150.0, 64.0, &mut 100.0).is_err());
    }

    #[test]
    fn finite_heights_do_not_hide_an_invalid_coordinate() {
        let mut from = SpatialRef::from_epsg(4326).unwrap();
        let mut to = SpatialRef::from_epsg(3857).unwrap();
        for crs in [&mut from, &mut to] {
            crs.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
        }
        let transform = DatumTransform::new(&from, &to, None).unwrap();
        let mut longitude = [0.0, 0.0];
        let mut latitude = [0.0, 100.0];
        let mut height = [12.0, 12.0];
        assert!(
            transform
                .row(&mut longitude, &mut latitude, &mut height)
                .is_err()
        );
        assert_eq!(height[0], 12.0);
    }
}

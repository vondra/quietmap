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
            match errors[i] {
                0 if longitude[i].is_finite()
                    && latitude[i].is_finite()
                    && heights[i].is_finite() => {}
                // PROJ proj.h: COORD_TRANSFM (2048) + OUTSIDE_GRID (4) / GRID_AT_NODATA (5).
                2052 | 2053 => heights[i] = f64::NAN,
                code => {
                    return Err(format!(
                        "datum conversion failed with PROJ code {code}: {}",
                        warp::error()
                    ));
                }
            }
        }
        Ok(())
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
    let mut transformed = 0;
    let had = values.iter().any(|v| v.is_finite());
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
        transform.row(&mut longitudes, &mut latitudes, &mut elevations)?;
        for (i, height) in indices.into_iter().zip(elevations) {
            heights[i] = height;
            transformed += usize::from(height.is_finite());
        }
    }
    if had && transformed == 0 {
        return Err(format!(
            "{}: datum conversion has no valid nodes",
            source.spec.input
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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

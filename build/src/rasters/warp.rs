//! GDAL warps onto explicit geographic node windows; source scale tags never alter heights.
use gdal::raster::Buffer;
use gdal::{Dataset, DriverManager};
use std::ffi::{CStr, CString};
use tiles::terrain::{NODES_PER_DEGREE, Window};

/// A VRT can keep its source handle; the borrow makes its lifetime explicit even for MEM outputs.
pub struct DerivedDataset<'a> {
    dataset: Dataset,
    _source: &'a Dataset,
}
impl std::ops::Deref for DerivedDataset<'_> {
    type Target = Dataset;
    fn deref(&self) -> &Dataset {
        &self.dataset
    }
}

pub fn error() -> String {
    unsafe { CStr::from_ptr(gdal_sys::CPLGetLastErrorMsg()) }
        .to_string_lossy()
        .into_owned()
}

fn arguments(options: &[String]) -> Result<Vec<CString>, String> {
    options
        .iter()
        .map(|s| CString::new(s.as_bytes()).map_err(|e| e.to_string()))
        .collect()
}

pub fn warp<'a>(source: &'a Dataset, options: &[String]) -> Result<DerivedDataset<'a>, String> {
    let strings = arguments(options)?;
    let mut pointers: Vec<_> = strings.iter().map(|s| s.as_ptr().cast_mut()).collect();
    pointers.push(std::ptr::null_mut());
    let mut source_handle = source.c_dataset();
    let mut usage_error = 0;
    // GDAL borrows the source and options during this call; Dataset owns the returned handle.
    unsafe {
        let options = gdal_sys::GDALWarpAppOptionsNew(pointers.as_mut_ptr(), std::ptr::null_mut());
        if options.is_null() {
            return Err(error());
        }
        let output = gdal_sys::GDALWarp(
            c"".as_ptr(),
            std::ptr::null_mut(),
            1,
            &mut source_handle,
            options,
            &mut usage_error,
        );
        gdal_sys::GDALWarpAppOptionsFree(options);
        if output.is_null() {
            return Err(error());
        }
        let dataset = Dataset::from_c_dataset(output);
        if usage_error != 0 {
            return Err(error());
        }
        Ok(DerivedDataset {
            dataset,
            _source: source,
        })
    }
}

pub fn translate<'a>(
    source: &'a Dataset,
    options: &[String],
) -> Result<DerivedDataset<'a>, String> {
    let strings = arguments(options)?;
    let mut pointers: Vec<_> = strings.iter().map(|s| s.as_ptr().cast_mut()).collect();
    pointers.push(std::ptr::null_mut());
    let mut usage_error = 0;
    unsafe {
        let options =
            gdal_sys::GDALTranslateOptionsNew(pointers.as_mut_ptr(), std::ptr::null_mut());
        if options.is_null() {
            return Err(error());
        }
        let output =
            gdal_sys::GDALTranslate(c"".as_ptr(), source.c_dataset(), options, &mut usage_error);
        gdal_sys::GDALTranslateOptionsFree(options);
        if output.is_null() {
            return Err(error());
        }
        let dataset = Dataset::from_c_dataset(output);
        if usage_error != 0 {
            return Err(error());
        }
        Ok(DerivedDataset {
            dataset,
            _source: source,
        })
    }
}

pub fn bounds(window: Window) -> [f64; 4] {
    let density = f64::from(NODES_PER_DEGREE);
    [
        (f64::from(window.west_node) - 0.5) / density,
        (f64::from(window.north_node) - f64::from(window.rows) + 0.5) / density,
        (f64::from(window.west_node) + f64::from(window.columns) - 0.5) / density,
        (f64::from(window.north_node) + 0.5) / density,
    ]
}

pub fn read(
    source: &Dataset,
    window: Window,
    crs: &str,
    nodata: (Option<f64>, f64),
    kernel: &str,
    dtype: &str,
    exact: bool,
) -> Result<Vec<f64>, String> {
    let [west, south, east, north] = bounds(window);
    let mut options: Vec<String> = [
        "-of",
        "MEM",
        "-ot",
        dtype,
        "-s_srs",
        crs,
        "-t_srs",
        "EPSG:4326",
        "-novshift",
        "-r",
        kernel,
        "-wo",
        "NUM_THREADS=1",
    ]
    .map(str::to_string)
    .to_vec();
    options.extend(["-dstnodata".into(), nodata.1.to_string(), "-te".into()]);
    options.extend([west, south, east, north].map(|v| v.to_string()));
    options.extend([
        "-ts".into(),
        window.columns.to_string(),
        window.rows.to_string(),
    ]);
    if exact {
        options.extend(["-et".into(), "0".into()]);
    }
    options.extend(["-ovr".into(), "NONE".into()]);
    if let Some(nodata) = nodata.0 {
        options.extend(["-srcnodata".into(), nodata.to_string()]);
    }
    let output = warp(source, &options)?;
    values(&output)
}

pub fn values(source: &Dataset) -> Result<Vec<f64>, String> {
    let band = source.rasterband(1).map_err(|e| e.to_string())?;
    Ok(band
        .read_as::<f64>((0, 0), source.raster_size(), source.raster_size(), None)
        .map_err(|e| e.to_string())?
        .data()
        .to_vec())
}

pub fn memory(
    width: usize,
    height: usize,
    transform: [f64; 6],
    crs: &str,
    data: Vec<f64>,
) -> Result<Dataset, String> {
    let mut dataset = DriverManager::get_driver_by_name("MEM")
        .and_then(|d| d.create_with_band_type::<f64, _>("", width, height, 1))
        .map_err(|e| e.to_string())?;
    dataset
        .set_geo_transform(&transform)
        .map_err(|e| e.to_string())?;
    dataset.set_projection(crs).map_err(|e| e.to_string())?;
    let mut band = dataset.rasterband(1).map_err(|e| e.to_string())?;
    band.set_no_data_value(Some(f64::NAN))
        .map_err(|e| e.to_string())?;
    band.write(
        (0, 0),
        (width, height),
        &mut Buffer::new((width, height), data),
    )
    .map_err(|e| e.to_string())?;
    Ok(dataset)
}
